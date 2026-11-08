use audigma_cli::ipc::{
    ControlClient, ControlServer, Instance, IpcError, IpcRequest, IpcResponse, list_live_instances,
};
use audigma_engine::{DaemonControlHandle, DaemonOutputFormat};
use std::path::{Path, PathBuf};

/// Fresh, private runtime dir per test so tests never touch the real `$XDG_RUNTIME_DIR`.
fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("audigma-ipc-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn handle(input_rate: u32) -> DaemonControlHandle {
    DaemonControlHandle::new(
        input_rate,
        DaemonOutputFormat::Pcm,
        192000,
        "linear, apodization off",
        -3.0,
    )
}

fn start(name: &str, dir: &Path, control: &DaemonControlHandle) -> ControlServer {
    let inst = Instance::with_base_dir(name, dir.to_path_buf()).unwrap();
    let lock = inst.acquire_lock().expect("acquire lock");
    ControlServer::start(inst, lock, control.clone()).expect("start server")
}

#[test]
fn test_ipc_roundtrip() {
    let dir = test_dir("roundtrip");
    let control = handle(48000);
    let server = start("default", &dir, &control);
    let inst = Instance::with_base_dir("default", dir.clone()).unwrap();

    let resp = ControlClient::send(&inst, IpcRequest::GetStatus).expect("status");
    let IpcResponse::Telemetry(telem) = resp else {
        panic!("expected telemetry");
    };
    assert_eq!(telem.input_rate_hz, 48000);
    assert_eq!(telem.preamp_db, -3.0);
    assert_eq!(telem.filter, "linear, apodization off");

    let resp = ControlClient::send(&inst, IpcRequest::SetVolume { volume_db: -4.5 }).unwrap();
    assert!(matches!(resp, IpcResponse::Ok));
    assert_eq!(control.get_volume(), -4.5);

    let resp = ControlClient::send(&inst, IpcRequest::Stop).unwrap();
    assert!(matches!(resp, IpcResponse::Ok));
    assert!(!control.is_running());

    drop(server);
    assert!(list_live_instances(Some(&dir)).is_empty());
    assert!(matches!(
        ControlClient::send(&inst, IpcRequest::GetStatus),
        Err(IpcError::DaemonNotRunning(_))
    ));
}

#[test]
fn test_active_instance_collision_rejection() {
    let dir = test_dir("collision");
    let control = handle(48000);
    let _server = start("test-collision", &dir, &control);

    let inst = Instance::with_base_dir("test-collision", dir.clone()).unwrap();
    match inst.acquire_lock() {
        Err(IpcError::InstanceAlreadyRunning(name)) => assert_eq!(name, "test-collision"),
        other => panic!("expected InstanceAlreadyRunning, got {other:?}"),
    }

    // The first server must be untouched and still responsive.
    let resp = ControlClient::send(&inst, IpcRequest::GetStatus).expect("first still alive");
    assert!(matches!(resp, IpcResponse::Telemetry(_)));
}

#[test]
fn test_multi_instance_concurrency() {
    let dir = test_dir("multi");
    let ctrl_a = handle(44100);
    let ctrl_b = handle(96000);
    let _a = start("test-a", &dir, &ctrl_a);
    let _b = start("test-b", &dir, &ctrl_b);

    let inst_a = Instance::with_base_dir("test-a", dir.clone()).unwrap();
    let inst_b = Instance::with_base_dir("test-b", dir.clone()).unwrap();

    let rate = |inst: &Instance| match ControlClient::send(inst, IpcRequest::GetStatus).unwrap() {
        IpcResponse::Telemetry(t) => t.input_rate_hz,
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(rate(&inst_a), 44100);
    assert_eq!(rate(&inst_b), 96000);

    // Commands are isolated per instance.
    ControlClient::send(&inst_a, IpcRequest::SetVolume { volume_db: -10.0 }).unwrap();
    assert_eq!(ctrl_a.get_volume(), -10.0);
    assert_eq!(ctrl_b.get_volume(), 0.0);

    assert_eq!(
        list_live_instances(Some(&dir)),
        vec!["test-a".to_string(), "test-b".to_string()]
    );
}

#[test]
fn test_orphan_socket_recovery() {
    let dir = test_dir("orphan");
    let control = handle(48000);

    // Simulate a SIGKILLed daemon: a bound socket whose listener is gone but whose file remains.
    std::fs::create_dir_all(&dir).unwrap();
    let orphan = dir.join("test-orphan.sock");
    let listener = std::os::unix::net::UnixListener::bind(&orphan).unwrap();
    drop(listener);
    assert!(orphan.exists());

    let inst = Instance::with_base_dir("test-orphan", dir.clone()).unwrap();
    // Stale socket must read as "not running" rather than a raw connection error.
    assert!(matches!(
        ControlClient::send(&inst, IpcRequest::GetStatus),
        Err(IpcError::DaemonNotRunning(_))
    ));
    assert!(list_live_instances(Some(&dir)).is_empty());

    let _server = start("test-orphan", &dir, &control);
    assert!(matches!(
        ControlClient::send(&inst, IpcRequest::GetStatus),
        Ok(IpcResponse::Telemetry(_))
    ));
}

#[test]
fn test_lock_released_after_shutdown() {
    let dir = test_dir("relock");
    let control = handle(48000);
    let server = start("test-relock", &dir, &control);
    drop(server);

    let inst = Instance::with_base_dir("test-relock", dir).unwrap();
    inst.acquire_lock().expect("lock free after server drop");
}

#[test]
fn test_client_clean_failure_when_stopped() {
    let dir = test_dir("dead");
    let inst = Instance::with_base_dir("test-dead", dir).unwrap();
    assert!(matches!(
        ControlClient::send(&inst, IpcRequest::GetStatus),
        Err(IpcError::DaemonNotRunning(name)) if name == "test-dead"
    ));
}

#[test]
fn test_instance_name_validation() {
    for good in ["default", "k17", "ja11", "a_b-C9", &"x".repeat(32)] {
        assert!(Instance::new(good).is_ok(), "{good} should be valid");
    }
    for bad in ["", "a/b", "..", "a b", "k17.sock", &"x".repeat(33), "é"] {
        assert!(
            matches!(Instance::new(bad), Err(IpcError::InvalidInstanceName(_))),
            "{bad:?} should be rejected"
        );
    }
}
