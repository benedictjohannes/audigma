use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live_pipeline::{
    DaemonControlHandle, DaemonOutputFormat, DaemonState, LivePipeline,
};
use audigma_engine::traits::{
    AudioInput, AudioOutput, DsdDecodeConfig, EngineError, MockAudioInput,
};
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
    let info = AudioStreamInfo::new_pcm(input_rate, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(
        44100,
        DaemonOutputFormat::Dsd,
        11289600,
        "linear, apodization off",
        0.0,
    );
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
    assert_eq!(final_telem.state, DaemonState::Stopped);
}

#[test]
fn test_live_pipeline_dynamic_sample_rate_switch() {
    let settings = Settings::default_offline(OutputContainer::Raw);
    let r44 = PcmRateHz::new(44100).unwrap();
    let r96 = PcmRateHz::new(96000).unwrap();

    let mut pipeline = LivePipeline::prepare(settings.clone(), r44).unwrap();

    let frames = 44100;
    let info = AudioStreamInfo::new_pcm(r44, 2, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(
        44100,
        DaemonOutputFormat::Dsd,
        11289600,
        "linear, apodization off",
        0.0,
    );

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
    let info = AudioStreamInfo::new_pcm(r48, 6, Some(frames as u64), Some(32));
    let mut mock_in = MockAudioInput::new_multichannel(info, ch_data, false);
    let mut mock_out = TestOutputSink::default();

    let control = DaemonControlHandle::new(
        48000,
        DaemonOutputFormat::Pcm,
        96000,
        "linear, apodization off",
        0.0,
    );
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

/// Raw-only input proves that live conversion does not call the serial decoded
/// read path. Playback stops deterministically after the final fragment.
struct RawOnlyDsdInput {
    info: AudioStreamInfo,
    bytes: Vec<u8>,
    pos: usize,
    fragment_bytes: usize,
    reads: usize,
    switch_rate: bool,
    control: DaemonControlHandle,
}

impl AudioInput for RawOnlyDsdInput {
    fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    fn read_stereo(&mut self, _left: &mut [f64], _right: &mut [f64]) -> Result<usize, EngineError> {
        panic!("live DSD conversion must not decode on the input/coordinator thread");
    }

    fn dsd_decode_config(&self) -> Option<DsdDecodeConfig> {
        Some(DsdDecodeConfig {
            decimation_stages: 4,
            gain_db: 0.0,
        })
    }

    fn read_dsd_bytes(&mut self, channels: &mut [&mut [u8]]) -> Result<usize, EngineError> {
        let bytes = channels
            .iter()
            .map(|channel| channel.len())
            .min()
            .unwrap_or(0)
            .min(self.fragment_bytes)
            .min(self.bytes.len() - self.pos);
        for channel in channels {
            channel[..bytes].copy_from_slice(&self.bytes[self.pos..self.pos + bytes]);
        }
        self.pos += bytes;
        self.reads += 1;
        if self.pos == self.bytes.len() && !self.switch_rate {
            self.control.stop();
        }
        Ok(bytes)
    }

    fn check_rate_change(&self) -> Option<PcmRateHz> {
        (self.switch_rate && self.reads > 0).then_some(PcmRateHz::PCM_192000)
    }

    fn buffer_fill_secs(&self) -> Option<f64> {
        Some(0.060)
    }
    fn can_rewind(&self) -> bool {
        false
    }
    fn rewind(&mut self) -> Result<(), EngineError> {
        Err(EngineError::TwoPassRequiresSeekableInput)
    }
}

fn raw_dsd_settings(target: &str) -> Settings {
    Settings::from_toml_str(&format!(
        r#"
schema_version = 1
[input]
backend = "pipewire"
channels = 2
allow_dsd = true
[output]
backend = "alsa"
device = "null"
transport = "native"
[target]
{target}
[gain]
overload_policy = "mute-recover"
[clock]
mode = "adaptive-input"
max_correction_ppm = 500
"#
    ))
    .unwrap()
}

fn raw_only_input(
    control: DaemonControlHandle,
    bytes: usize,
    fragment_bytes: usize,
    switch_rate: bool,
) -> RawOnlyDsdInput {
    RawOnlyDsdInput {
        info: AudioStreamInfo::new_dsd(audigma_dsp::DsdBitRateHz::DSD64, 2, Some(bytes as u64 * 8)),
        bytes: vec![0x69; bytes],
        pos: 0,
        fragment_bytes,
        reads: 0,
        switch_rate,
        control,
    }
}

#[test]
fn live_raw_dsd512_conversion_preserves_fragmented_input_without_serial_decode() {
    let settings =
        raw_dsd_settings("format = \"dsd\"\nrate_family = \"source\"\ndsd_rate = \"dsd512\"");
    let pipeline = LivePipeline::prepare(settings, PcmRateHz::PCM_176400).unwrap();
    let control = DaemonControlHandle::new(
        176400,
        DaemonOutputFormat::Dsd,
        22579200,
        "linear, apodization off",
        0.0,
    );
    let mut input = raw_only_input(control.clone(), 513, 13, false);
    let mut output = TestOutputSink::default();
    assert_eq!(
        pipeline
            .run_streaming(&mut input, &mut output, control.clone())
            .unwrap(),
        audigma_engine::StreamExitReason::Stopped
    );
    assert!(input.reads > 1);
    assert_eq!(control.get_telemetry().total_input_frames, 256);
    // 256 decoded frames * 128 DSD bits/frame/channel, plus the existing idle tail.
    assert_eq!(output.bytes.len(), 256 * 128 / 8 * 2 + 1024 * 8);
}

#[test]
fn live_raw_dsd_pcm_downsampling_keeps_decoded_input_and_output_counts() {
    let settings = raw_dsd_settings(
        "format = \"pcm\"\nrate_family = \"source\"\npcm_rate_hz = 44100\nsample_format = \"f64\"",
    );
    let pipeline = LivePipeline::prepare(settings, PcmRateHz::PCM_176400).unwrap();
    let control = DaemonControlHandle::new(
        176400,
        DaemonOutputFormat::Pcm,
        44100,
        "linear, apodization off",
        0.0,
    );
    let mut input = raw_only_input(control.clone(), 2049, 31, false);
    let mut output = TestOutputSink::default();
    assert_eq!(
        pipeline
            .run_streaming(&mut input, &mut output, control.clone())
            .unwrap(),
        audigma_engine::StreamExitReason::Stopped
    );
    let telemetry = control.get_telemetry();
    assert_eq!(telemetry.total_input_frames, 1024);
    assert_eq!(telemetry.total_output_frames, 256);
    assert_eq!(output.bytes.len(), 256 * 2 * 8 + 1024 * 2 * 4);
}

#[test]
fn live_raw_dsd_rate_switch_drains_with_pcm_zeros_without_decoded_reads() {
    let settings =
        raw_dsd_settings("format = \"dsd\"\nrate_family = \"source\"\ndsd_rate = \"dsd512\"");
    let pipeline = LivePipeline::prepare(settings, PcmRateHz::PCM_176400).unwrap();
    let control = DaemonControlHandle::new(
        176400,
        DaemonOutputFormat::Dsd,
        22579200,
        "linear, apodization off",
        0.0,
    );
    let mut input = raw_only_input(control.clone(), 512, 512, true);
    let mut output = TestOutputSink::default();
    assert_eq!(
        pipeline
            .run_streaming(&mut input, &mut output, control)
            .unwrap(),
        audigma_engine::StreamExitReason::RateChanged {
            new_rate: PcmRateHz::PCM_192000,
            channels: 2
        }
    );
    assert_eq!(input.reads, 1);
    assert!(!output.bytes.is_empty());
}

#[test]
fn test_dsp_session_keeps_workers_across_streams_and_rates() {
    use audigma_engine::live_pipeline::DspSession;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let settings = Settings::default_offline(OutputContainer::Raw);
    let started = Arc::new(AtomicUsize::new(0));
    let hook: audigma_engine::ThreadInitHook = {
        let started = Arc::clone(&started);
        Arc::new(move |_, _| {
            started.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    let setup = audigma_engine::WorkerSetup {
        placements: Vec::new(),
        hook: Some(hook),
        ..Default::default()
    };

    let mut session = DspSession::new();
    assert!(!session.is_active());

    for hz in [44100u32, 96000, 44100] {
        let rate = PcmRateHz::new(hz).unwrap();
        let pipeline =
            LivePipeline::prepare_with_workers(settings.clone(), rate, Some(setup.clone()))
                .unwrap();
        let frames = hz as usize / 4;
        let info = AudioStreamInfo::new_pcm(rate, 2, Some(frames as u64), Some(32));
        let mut mock_in = MockAudioInput::new(info, vec![0.1; frames], vec![0.1; frames], false);
        let mut mock_out = TestOutputSink::default();
        let control = DaemonControlHandle::new(
            hz,
            DaemonOutputFormat::Dsd,
            11289600,
            "linear, apodization off",
            0.0,
        );
        let ctrl = control.clone();
        std::thread::scope(|scope| {
            let h = scope.spawn(|| {
                pipeline.run_streaming_with_session(
                    &mut session,
                    &mut mock_in,
                    &mut mock_out,
                    ctrl,
                    None,
                )
            });
            std::thread::sleep(std::time::Duration::from_millis(80));
            control.stop();
            assert_eq!(
                h.join().unwrap().unwrap(),
                audigma_engine::StreamExitReason::Stopped
            );
        });
        assert!(session.is_active());
        assert!(
            !mock_out.bytes.is_empty(),
            "stream at {hz} Hz produced no output"
        );
    }

    // Two channel workers, spawned once for three streams.
    assert_eq!(started.load(Ordering::SeqCst), 2);
}
