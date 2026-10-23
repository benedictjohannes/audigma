//! Integration tests for Unified Volume Strategy & Hardware Mixer Bridge (`Design.md §1.8`, `Phase 5 Step 5.5`).

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{DaemonControlHandle, LivePipeline};
use audigma_engine::traits::{
    AudioOutput, EngineError, HardwareVolumeBridge, MockAudioInput, VolumeUpdate,
};
use audigma_format::AudioStreamInfo;
use audigma_settings::{OutputContainer, Settings};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct TestOutputSink {
    pub bytes: Vec<u8>,
}

impl Write for TestOutputSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AudioOutput for TestOutputSink {}

#[derive(Default, Clone)]
struct MockHardwareBridge {
    pub volumes: Arc<Mutex<Vec<f64>>>,
    pub mutes: Arc<Mutex<Vec<bool>>>,
    pub db_range: Option<(f64, f64)>,
}

impl HardwareVolumeBridge for MockHardwareBridge {
    fn set_volume_db(&mut self, db: f64) -> Result<(), EngineError> {
        self.volumes.lock().unwrap().push(db);
        Ok(())
    }

    fn set_mute(&mut self, muted: bool) -> Result<(), EngineError> {
        self.mutes.lock().unwrap().push(muted);
        Ok(())
    }

    fn get_db_range(&self) -> Option<(f64, f64)> {
        self.db_range
    }
}

#[test]
fn test_volume_hardware_mode_routes_to_bridge() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    control.set_volume_mode("hardware");
    let ctrl_thread = control.clone();

    let mock_bridge = MockHardwareBridge::default();
    let bridge_vols = mock_bridge.volumes.clone();
    let bridge_mutes = mock_bridge.mutes.clone();

    let handle = std::thread::spawn(move || {
        let mut b = mock_bridge;
        pipeline.run_streaming_with_bridge(&mut mock_in, &mut mock_out, ctrl_thread, Some(&mut b))
    });

    std::thread::sleep(Duration::from_millis(50));
    control.set_volume(-6.0);
    std::thread::sleep(Duration::from_millis(50));
    control.set_muted(true);
    std::thread::sleep(Duration::from_millis(50));

    control.stop();
    let _ = handle.join().unwrap();

    let vols = bridge_vols.lock().unwrap().clone();
    let mutes = bridge_mutes.lock().unwrap().clone();

    assert!(
        vols.contains(&-6.0),
        "bridge should receive -6.0 dB in hardware mode"
    );
    assert!(
        mutes.contains(&true),
        "bridge should receive mute=true in hardware mode"
    );
}

#[test]
fn test_dynamic_volume_from_input_driver_interception() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    control.set_volume_mode("hardware");
    let ctrl_thread = control.clone();

    let mock_bridge = MockHardwareBridge::default();
    let bridge_vols = mock_bridge.volumes.clone();

    // Enqueue a volume update from upstream (simulating PipeWire props slider)
    mock_in.simulate_volume_change(VolumeUpdate {
        volume_db: Some(-12.5),
        volume_percent: None,
        is_muted: Some(false),
    });

    let mut in_driver = mock_in;
    let handle = std::thread::spawn(move || {
        let mut b = mock_bridge;
        pipeline.run_streaming_with_bridge(&mut in_driver, &mut mock_out, ctrl_thread, Some(&mut b))
    });

    std::thread::sleep(Duration::from_millis(60));

    // Verify control volume was updated by the driver event
    assert_eq!(control.get_volume(), -12.5);

    control.stop();
    let _ = handle.join().unwrap();

    let vols = bridge_vols.lock().unwrap().clone();
    assert!(
        vols.contains(&-12.5),
        "bridge should receive -12.5 dB from input event"
    );
}

#[test]
fn test_software_volume_mode_does_not_call_hardware_bridge() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    control.set_volume_mode("software");
    let ctrl_thread = control.clone();

    let mock_bridge = MockHardwareBridge::default();
    let bridge_vols = mock_bridge.volumes.clone();

    let handle = std::thread::spawn(move || {
        let mut b = mock_bridge;
        pipeline.run_streaming_with_bridge(&mut mock_in, &mut mock_out, ctrl_thread, Some(&mut b))
    });

    std::thread::sleep(Duration::from_millis(50));
    control.set_volume(-9.0);
    std::thread::sleep(Duration::from_millis(50));

    control.stop();
    let _ = handle.join().unwrap();

    let vols = bridge_vols.lock().unwrap().clone();
    assert!(
        vols.is_empty(),
        "bridge should NOT be called when volume_mode is 'software'"
    );
}

#[test]
fn test_none_volume_mode_ignores_volume() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    control.set_volume_mode("none");
    let ctrl_thread = control.clone();

    let mock_bridge = MockHardwareBridge::default();
    let bridge_vols = mock_bridge.volumes.clone();

    let handle = std::thread::spawn(move || {
        let mut b = mock_bridge;
        pipeline.run_streaming_with_bridge(&mut mock_in, &mut mock_out, ctrl_thread, Some(&mut b))
    });

    std::thread::sleep(Duration::from_millis(50));
    control.set_volume(-15.0);
    std::thread::sleep(Duration::from_millis(50));

    control.stop();
    let _ = handle.join().unwrap();

    let vols = bridge_vols.lock().unwrap().clone();
    assert!(
        vols.is_empty(),
        "bridge should NOT be called when volume_mode is 'none'"
    );
}

#[test]
fn test_volume_percentage_alsamixer_curve_on_hardware_bridge() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    control.set_volume_mode("hardware");
    let ctrl_thread = control.clone();

    let mock_bridge = MockHardwareBridge {
        db_range: Some((-50.0, 0.0)), // JA11 hardware limits
        ..Default::default()
    };
    let bridge_vols = mock_bridge.volumes.clone();

    // Upstream PipeWire sends 53% slider
    mock_in.simulate_volume_change(VolumeUpdate {
        volume_db: None,
        volume_percent: Some(52.6),
        is_muted: Some(false),
    });

    let mut in_driver = mock_in;
    let handle = std::thread::spawn(move || {
        let mut b = mock_bridge;
        pipeline.run_streaming_with_bridge(&mut in_driver, &mut mock_out, ctrl_thread, Some(&mut b))
    });

    std::thread::sleep(Duration::from_millis(60));

    // 53% slider on JA11 should map to -13.50 dB
    let vols = bridge_vols.lock().unwrap().clone();
    assert!(
        vols.iter().any(|&v| (v - (-13.50)).abs() < 0.1),
        "expected -13.5 dB on hardware bridge for 53%, got {:?}",
        vols
    );

    // Now set 76% slider via control handle -> should map to -6.00 dB
    control.set_volume_percent(75.9);
    std::thread::sleep(Duration::from_millis(60));

    let vols = bridge_vols.lock().unwrap().clone();
    assert!(
        vols.iter().any(|&v| (v - (-6.00)).abs() < 0.1),
        "expected -6.0 dB on hardware bridge for 76%, got {:?}",
        vols
    );

    control.stop();
    let _ = handle.join().unwrap();
}

#[test]
fn test_daemon_set_volume_clamps_to_supported_range() {
    let control = DaemonControlHandle::new(44_100, "pcm", 192_000, "balanced", 0.0);

    control.set_volume(1_000.0);
    assert_eq!(control.get_volume(), audigma_dsp::MAX_VOLUME_DB);

    control.set_volume(-1_000.0);
    assert_eq!(control.get_volume(), audigma_dsp::MIN_VOLUME_DB);

    // A non-finite dB value must never reach the DSP gain stage.
    control.set_volume(f64::NAN);
    assert_eq!(control.get_volume(), audigma_dsp::MIN_VOLUME_DB);

    // In-range values pass through untouched.
    control.set_volume(-6.0);
    assert_eq!(control.get_volume(), -6.0);
}
