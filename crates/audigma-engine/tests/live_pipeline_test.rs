use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{DaemonControlHandle, LivePipeline};
use audigma_engine::traits::{AudioOutput, MockAudioInput};
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

    let final_telem = control.get_telemetry();
    assert_eq!(final_telem.state, "Stopped");
}
