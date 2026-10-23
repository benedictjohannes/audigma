//! Integration tests for output device resilience and hotplug lifecycle transitions (`Plan.md Step 5.2`).

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{DaemonControlHandle, DaemonOutputFormat, DaemonState, LivePipeline};
use audigma_engine::traits::{AudioOutput, EngineError, MockAudioInput};
use audigma_format::AudioStreamInfo;
use audigma_io::alsa::{AlsaDeviceWatcher, wait_for_alsa_device};
use audigma_settings::{OutputContainer, Settings};

use std::io::{ErrorKind, Result as IoResult, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Mock output sink that simulates a hardware device disconnection after writing N bytes.
struct DisconnectingOutputSink {
    bytes_before_disconnect: usize,
    bytes_written: usize,
    disconnected: bool,
}

impl DisconnectingOutputSink {
    fn new(bytes_before_disconnect: usize) -> Self {
        Self {
            bytes_before_disconnect,
            bytes_written: 0,
            disconnected: false,
        }
    }
}

impl Write for DisconnectingOutputSink {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        if self.disconnected || self.bytes_written >= self.bytes_before_disconnect {
            self.disconnected = true;
            return Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "mock ALSA device disconnected: ENODEV",
            ));
        }

        let allowable = (self.bytes_before_disconnect - self.bytes_written).min(buf.len());
        self.bytes_written += allowable;
        if allowable < buf.len() {
            self.disconnected = true;
        }
        Ok(allowable)
    }

    fn flush(&mut self) -> IoResult<()> {
        if self.disconnected {
            return Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "mock ALSA device disconnected: ENODEV",
            ));
        }
        Ok(())
    }
}

impl AudioOutput for DisconnectingOutputSink {}

#[test]
fn test_disconnecting_output_sink_triggers_device_disconnected_error() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);

    // Output will disconnect after ~2048 bytes
    let mut mock_out = DisconnectingOutputSink::new(2048);
    let control =
        DaemonControlHandle::new(44100, DaemonOutputFormat::Dsd, 11289600, "balanced", 0.0);

    let res = pipeline.run_streaming(&mut mock_in, &mut mock_out, control);
    assert!(res.is_err());
    match res.unwrap_err() {
        EngineError::DeviceDisconnected(msg) => {
            assert!(msg.contains("ENODEV") || msg.contains("disconnected"));
        }
        other => panic!("expected EngineError::DeviceDisconnected, got: {other:?}"),
    }
}

#[test]
fn test_inotify_device_watcher_lifecycle_event_trigger() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = AlsaDeviceWatcher::with_path(temp.path()).expect("watcher init");

    // Initially nothing
    assert!(
        !watcher
            .wait_for_event(Some(Duration::from_millis(50)))
            .unwrap()
    );

    // Create a new control device node in tempdir
    let node_path = temp.path().join("controlC2");
    std::fs::write(&node_path, b"test-card").unwrap();

    // Event should fire
    assert!(
        watcher
            .wait_for_event(Some(Duration::from_millis(500)))
            .unwrap()
    );

    // Next wait should be quiet again
    assert!(
        !watcher
            .wait_for_event(Some(Duration::from_millis(50)))
            .unwrap()
    );
}

#[test]
fn test_wait_for_alsa_device_cancel_flag() {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();

    // Spawn thread to cancel after 50ms
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        cancel_clone.store(true, Ordering::SeqCst);
    });

    // Device that does not exist with wait indefinitely (-1)
    let res = wait_for_alsa_device("hw:CARD=NonExistentDevice999", -1, &cancel);
    assert!(!res.unwrap());
}

#[test]
fn test_wait_for_alsa_device_timeout() {
    let cancel = AtomicBool::new(false);
    // Timeout after 100ms
    let res = wait_for_alsa_device("hw:CARD=NonExistentDevice999", 100, &cancel);
    assert!(!res.unwrap());
}

#[test]
fn test_telemetry_extended_fields_serializable() {
    let control = DaemonControlHandle::new(48000, DaemonOutputFormat::Pcm, 96000, "balanced", 0.0);
    control.set_state(DaemonState::WaitingForDevice);
    control.set_disconnect_info(Some("hw:CARD=JA11,DEV=0".to_string()), Some(1500));

    let telem = control.get_telemetry();
    assert_eq!(telem.state, DaemonState::WaitingForDevice);
    assert_eq!(telem.device_target.as_deref(), Some("hw:CARD=JA11,DEV=0"));
    assert_eq!(telem.disconnect_duration_ms, Some(1500));

    let json_str = serde_json::to_string(&telem).unwrap();
    assert!(json_str.contains("\"state\":\"WaitingForDevice\""));
    assert!(json_str.contains("\"output_format\":\"pcm\""));
    assert!(json_str.contains("\"volume_mode\":\"software\""));
    assert!(json_str.contains("\"device_target\":\"hw:CARD=JA11,DEV=0\""));
    assert!(json_str.contains("\"disconnect_duration_ms\":1500"));
}
