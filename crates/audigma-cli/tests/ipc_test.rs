use audigma_cli::ipc::{ControlClient, ControlServer, IpcRequest, IpcResponse};
use audigma_engine::DaemonControlHandle;

#[test]
fn test_ipc_socket_server_client_roundtrip() {
    let temp_dir = std::env::temp_dir();
    let socket_path = temp_dir.join(format!("audigma-test-sock-{}.sock", std::process::id()));

    let control = DaemonControlHandle::new(48000, "pcm", 192000, "balanced", -3.0);
    let server = ControlServer::start(socket_path.clone(), control.clone()).expect("Start server");

    // 1. Query initial status
    let resp = ControlClient::send(&socket_path, IpcRequest::GetStatus).expect("Get status");
    if let IpcResponse::Telemetry(telem) = resp {
        assert_eq!(telem.input_rate_hz, 48000);
        assert_eq!(telem.output_format, "pcm");
        assert_eq!(telem.preamp_db, -3.0);
        assert_eq!(telem.volume_db, 0.0);
    } else {
        panic!("Expected Telemetry response, got {:?}", resp);
    }

    // 2. Set volume
    let resp = ControlClient::send(&socket_path, IpcRequest::SetVolume { volume_db: -4.5 })
        .expect("Set volume");
    assert!(matches!(resp, IpcResponse::Ok));
    assert_eq!(control.get_volume(), -4.5);

    // 3. Set preset
    let resp = ControlClient::send(
        &socket_path,
        IpcRequest::SetPreset {
            preset: "reference".to_string(),
        },
    )
    .expect("Set preset");
    assert!(matches!(resp, IpcResponse::Ok));

    // 4. Request stop
    let resp = ControlClient::send(&socket_path, IpcRequest::Stop).expect("Stop");
    assert!(matches!(resp, IpcResponse::Ok));
    assert!(!control.is_running());

    drop(server);
    assert!(!socket_path.exists());
}
