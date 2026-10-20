use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{DaemonControlHandle, LivePipeline};
use audigma_engine::traits::{AudioInput, AudioOutput, MockAudioInput};
use audigma_format::AudioStreamInfo;
use audigma_settings::{OutputContainer, Settings};
use std::io::Write;

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

#[test]
fn test_live_pipeline_streaming_lifecycle() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    settings.validate().unwrap();

    let input_rate = PcmRateHz::new(44100).unwrap();
    let pipeline = LivePipeline::prepare(settings, input_rate).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo {
        sample_rate: input_rate,
        original_channels: 2,
        total_frames: Some(frames as u64),
        bits_per_sample: Some(32),
    };
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);
    let ctrl_thread = control.clone();

    // Spawn runner on background thread
    let handle = std::thread::spawn(move || {
        pipeline.run_streaming(&mut mock_in, &mut mock_out, ctrl_thread)
    });

    // Let it run for a bit, then stop
    std::thread::sleep(std::time::Duration::from_millis(50));
    let telem = control.get_telemetry();
    assert_eq!(telem.input_rate_hz, 44100);

    control.set_volume(-6.0);
    assert_eq!(control.get_volume(), -6.0);

    control.stop();
    let res = handle.join().expect("Thread join");
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), audigma_engine::StreamExitReason::Stopped);

    let final_telem = control.get_telemetry();
    assert_eq!(final_telem.state, "Stopped");
}

#[test]
fn test_live_pipeline_dynamic_sample_rate_switch() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let r44 = PcmRateHz::new(44100).unwrap();
    let r96 = PcmRateHz::new(96000).unwrap();

    let mut pipeline = LivePipeline::prepare(settings.clone(), r44).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo {
        sample_rate: r44,
        original_channels: 2,
        total_frames: Some(frames as u64),
        bits_per_sample: Some(32),
    };
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(44100, "Dsd", 11289600, "balanced", 0.0);

    // Simulate an upstream rate change to 96 kHz
    mock_in.simulate_rate_change(r96);

    let exit_reason = pipeline
        .run_streaming(&mut mock_in, &mut mock_out, control.clone())
        .expect("Streaming run");

    match exit_reason {
        audigma_engine::StreamExitReason::RateChanged { new_rate, channels } => {
            assert_eq!(new_rate, r96);
            assert_eq!(channels, 2);
        }
        other => panic!("expected RateChanged, got: {other:?}"),
    }

    // Now re-prepare the pipeline for 96 kHz
    mock_in.set_active_rate(r96);
    pipeline = LivePipeline::prepare(settings, r96).unwrap();

    // Verify subsequent streaming runs cleanly with the new rate until stopped
    let ctrl2 = control.clone();
    let handle2 =
        std::thread::spawn(move || pipeline.run_streaming(&mut mock_in, &mut mock_out, ctrl2));

    std::thread::sleep(std::time::Duration::from_millis(20));
    control.stop();

    let res2 = handle2.join().expect("Thread join");
    assert_eq!(res2.unwrap(), audigma_engine::StreamExitReason::Stopped);
}

#[test]
fn test_live_pipeline_multichannel_5_1_streaming() {
    let toml_str = r#"
schema_version = 1

[input]
backend = "pipewire"
channels = 6
channel_positions = [ "FL", "FR", "FC", "LFE", "RL", "RR" ]

[output]
backend = "pipewire"

[target]
format = "pcm"
sample_format = "f32"
rate_family = "source"
pcm_rate_hz = 96000
"#;
    let settings = Settings::from_toml_str(toml_str).unwrap();
    let r48 = PcmRateHz::new(48000).unwrap();
    let pipeline = LivePipeline::prepare(settings, r48).unwrap();

    let frames = 4800;
    let ch_data = vec![vec![0.05; frames]; 6];
    let info = AudioStreamInfo {
        sample_rate: r48,
        original_channels: 6,
        total_frames: Some(frames as u64),
        bits_per_sample: Some(32),
    };
    let mut mock_in = MockAudioInput::new_multichannel(info, ch_data, false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(48000, "pcm", 96000, "balanced", 0.0);
    let ctrl_thread = control.clone();

    let handle = std::thread::spawn(move || {
        let res = pipeline.run_streaming(&mut mock_in, &mut mock_out, ctrl_thread);
        (mock_out, res)
    });

    std::thread::sleep(std::time::Duration::from_millis(30));
    control.stop();

    let (mock_out, res) = handle.join().expect("Thread join");
    assert_eq!(res.unwrap(), audigma_engine::StreamExitReason::Stopped);
    assert!(!mock_out.bytes.is_empty());
}
