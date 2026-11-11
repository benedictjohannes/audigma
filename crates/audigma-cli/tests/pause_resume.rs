//! Pause/resume handshake over the control socket, with a fake supervisor that mirrors the
//! `run_run` loop (no devices involved).

use audigma_cli::ipc::{ControlClient, ControlServer, Instance, IpcRequest, IpcResponse};
use audigma_engine::{DaemonControlHandle, DaemonOutputFormat, DaemonState, PauseReason};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

struct Fixture {
    control: DaemonControlHandle,
    inst: Instance,
    _server: ControlServer,
    supervisor: Option<JoinHandle<()>>,
    sessions: Arc<AtomicUsize>,
    reload_result: Arc<Mutex<Result<(), String>>>,
}

fn fixture(tag: &str) -> Fixture {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("audigma-pr-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let control = DaemonControlHandle::new(48000, DaemonOutputFormat::Pcm, 192000, "linear", 0.0);
    let inst = Instance::with_base_dir("default", dir).unwrap();
    let lock = inst.acquire_lock().unwrap();
    let server = ControlServer::start(inst.clone(), lock, control.clone()).unwrap();

    let sessions = Arc::new(AtomicUsize::new(0));
    let reload_result = Arc::new(Mutex::new(Ok(())));
    let supervisor = {
        let control = control.clone();
        let sessions = sessions.clone();
        let reload_result = reload_result.clone();
        std::thread::spawn(move || {
            loop {
                // One "session": runs until stop or pause.
                sessions.fetch_add(1, Ordering::SeqCst);
                control.set_state(DaemonState::Running);
                while control.is_active() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if !control.is_running() || !control.pause_requested() {
                    break;
                }
                let resumed = control.park_until_resume(Duration::from_millis(5), || {
                    reload_result.lock().unwrap().clone()
                });
                if !resumed {
                    break;
                }
            }
        })
    };
    Fixture {
        control,
        inst,
        _server: server,
        supervisor: Some(supervisor),
        sessions,
        reload_result,
    }
}

fn send(f: &Fixture, req: IpcRequest) -> IpcResponse {
    ControlClient::send(&f.inst, req).unwrap()
}

fn state(f: &Fixture) -> DaemonState {
    match send(f, IpcRequest::GetStatus) {
        IpcResponse::Telemetry(t) => t.state,
        other => panic!("expected telemetry, got {other:?}"),
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.control.stop();
        if let Some(h) = self.supervisor.take() {
            let _ = h.join();
        }
    }
}

#[test]
fn pause_then_resume_rebuilds_the_session() {
    let f = fixture("cycle");
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));
    assert_eq!(state(&f), DaemonState::Paused);
    assert_eq!(f.sessions.load(Ordering::SeqCst), 1);

    assert!(matches!(send(&f, IpcRequest::Resume), IpcResponse::Ok));
    assert_ne!(state(&f), DaemonState::Paused);
    // Give the fake session a moment to start, then check a second one ran.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(f.sessions.load(Ordering::SeqCst), 2);
    assert_eq!(state(&f), DaemonState::Running);
}

#[test]
fn pause_and_resume_are_idempotent() {
    let f = fixture("idem");
    // Resume while running is a no-op success.
    assert!(matches!(send(&f, IpcRequest::Resume), IpcResponse::Ok));
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));
    assert_eq!(f.sessions.load(Ordering::SeqCst), 1);
    assert_eq!(state(&f), DaemonState::Paused);
}

#[test]
fn failed_reload_keeps_the_daemon_paused() {
    let f = fixture("fail");
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));

    *f.reload_result.lock().unwrap() = Err("bad config".to_string());
    match send(&f, IpcRequest::Resume) {
        IpcResponse::Error(msg) => assert_eq!(msg, "bad config"),
        other => panic!("expected error, got {other:?}"),
    }
    assert_eq!(state(&f), DaemonState::Paused);
    assert_eq!(f.sessions.load(Ordering::SeqCst), 1);

    // Fixing the config lets the next resume succeed.
    *f.reload_result.lock().unwrap() = Ok(());
    assert!(matches!(send(&f, IpcRequest::Resume), IpcResponse::Ok));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(f.sessions.load(Ordering::SeqCst), 2);
}

#[test]
fn stop_while_paused_ends_the_supervisor() {
    let mut f = fixture("stop");
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));
    assert!(matches!(send(&f, IpcRequest::Stop), IpcResponse::Ok));
    f.supervisor.take().unwrap().join().unwrap();
    assert!(!f.control.is_running());
}

#[test]
fn pause_after_stop_is_refused() {
    let mut f = fixture("late");
    f.control.stop();
    f.supervisor.take().unwrap().join().unwrap();
    assert!(
        f.control
            .pause_and_wait(Duration::from_millis(100))
            .is_err()
    );
}

fn reason(f: &Fixture) -> Option<PauseReason> {
    match send(f, IpcRequest::GetStatus) {
        IpcResponse::Telemetry(t) => t.pause_reason,
        other => panic!("expected telemetry, got {other:?}"),
    }
}

#[test]
fn pause_reason_is_reported_while_paused_and_cleared_on_resume() {
    let f = fixture("reason");
    assert_eq!(reason(&f), None);

    // A pause the session asks for itself (device wait expired).
    f.control.request_pause(PauseReason::DeviceWaitExpired);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while state(&f) != DaemonState::Paused {
        assert!(std::time::Instant::now() < deadline, "never paused");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reason(&f), Some(PauseReason::DeviceWaitExpired));

    assert!(matches!(send(&f, IpcRequest::Resume), IpcResponse::Ok));
    assert_eq!(reason(&f), None);

    // A client pause reports `Requested`.
    assert!(matches!(send(&f, IpcRequest::Pause), IpcResponse::Ok));
    assert_eq!(reason(&f), Some(PauseReason::Requested));
}
