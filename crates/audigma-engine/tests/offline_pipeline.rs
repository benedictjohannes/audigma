use audigma_dsp::PcmRateHz;
use audigma_dsp::config::TargetFormat;
use audigma_engine::traits::{MockAudioInput, SeekableSink, StreamSink};
use audigma_engine::{EngineError, OfflinePipeline};
use audigma_format::AudioStreamInfo;
use audigma_settings::{OutputContainer, PeakAnalysisMode, Settings};
use std::f64::consts::PI;
use std::io::Cursor;

fn generate_synthetic_stereo_sine(
    sample_rate: PcmRateHz,
    duration_secs: f64,
    frequency_hz: f64,
    amplitude: f64,
) -> (Vec<f64>, Vec<f64>) {
    let num_samples = (sample_rate.get() as f64 * duration_secs).round() as usize;
    let mut left = Vec::with_capacity(num_samples);
    let mut right = Vec::with_capacity(num_samples);

    let dt = 1.0 / sample_rate.get() as f64;
    for i in 0..num_samples {
        let t = i as f64 * dt;
        let s_l = amplitude * (2.0 * PI * frequency_hz * t).sin();
        let s_r = amplitude * (2.0 * PI * frequency_hz * t + PI * 0.5).sin(); // 90 degree phase shift
        left.push(s_l);
        right.push(s_r);
    }

    (left, right)
}

#[test]
fn test_offline_pipeline_dsf_conversion_trim_to_duration() {
    let in_rate = PcmRateHz::PCM_44100;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.1, 1000.0, 0.5);
    let total_frames = left.len() as u64;

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(total_frames),
        bits_per_sample: Some(16),
    };

    let mut input = MockAudioInput::new(info, left, right, true);
    let mut out_buffer = Cursor::new(Vec::new());

    let settings = Settings::default_offline(OutputContainer::Dsf);
    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");

    let report = pipeline
        .run(&mut input, Box::new(SeekableSink(&mut out_buffer)))
        .expect("conversion run failed");

    assert_eq!(report.input_rate, in_rate);
    assert_eq!(report.target_format, TargetFormat::Dsd);
    assert_eq!(report.total_input_frames, total_frames);
    assert_eq!(report.overload_count, 0);

    // Verify DSF container bytes
    let dsf_bytes = out_buffer.into_inner();
    assert!(dsf_bytes.len() > 92);
    assert_eq!(&dsf_bytes[0..4], b"DSD ");

    // Parse DSF header via audigma_format
    let (header, _, _) =
        audigma_format::dsf::parse_stereo_dsf_file(&dsf_bytes).expect("valid DSF file");
    assert_eq!(header.channels, 2);
    assert_eq!(header.bit_rate.get(), 11_289_600); // DSD256

    // In TrimToDuration, exact sample count matches input duration (total_frames * 256)
    let expected_dsd_samples = total_frames * 256;
    assert_eq!(header.sample_count_per_channel, expected_dsd_samples);
}

#[test]
fn test_offline_pipeline_wav_conversion_two_pass() {
    let in_rate = PcmRateHz::PCM_44100;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.05, 1000.0, 0.95);
    let total_frames = left.len() as u64;

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(total_frames),
        bits_per_sample: Some(16),
    };

    let mut input = MockAudioInput::new(info, left, right, true);
    let mut out_buffer = Cursor::new(Vec::new());

    let mut settings = Settings::default_offline(OutputContainer::Wav);
    // Configure two-pass peak analysis
    if let Some(ref mut gain) = settings.gain {
        gain.peak_analysis = PeakAnalysisMode::TwoPass;
        gain.preamp_db = 0.0;
    }

    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");
    let report = pipeline
        .run(&mut input, Box::new(SeekableSink(&mut out_buffer)))
        .expect("conversion failed");

    assert_eq!(report.total_input_frames, total_frames);
    assert!(report.measured_peak_dbfs.is_some());
    let measured_peak = report.measured_peak_dbfs.unwrap();
    // Inter-sample transient peak of abruptly started sine wave is captured accurately around +0.57 dBFS
    assert!((measured_peak - 0.57).abs() < 0.1);
    // Two-pass gain staging automatically applied attenuation to ensure L - M - P headroom
    assert!(report.applied_gain_db < -4.0);

    // Read back with hound
    let wav_bytes = out_buffer.into_inner();
    let reader = hound::WavReader::new(Cursor::new(wav_bytes)).expect("valid WAV header");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 192000);
    assert_eq!(spec.channels, 2);
}

#[test]
fn test_offline_pipeline_flac_conversion() {
    let in_rate = PcmRateHz::PCM_44100;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.05, 440.0, 0.7);
    let total_frames = left.len() as u64;

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(total_frames),
        bits_per_sample: Some(16),
    };

    let mut input = MockAudioInput::new(info, left, right, true);
    let mut out_buffer = Cursor::new(Vec::new());

    let settings = Settings::default_offline(OutputContainer::Flac);
    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");
    let report = pipeline
        .run(&mut input, Box::new(SeekableSink(&mut out_buffer)))
        .expect("conversion failed");

    assert_eq!(report.total_input_frames, total_frames);

    let flac_bytes = out_buffer.into_inner();
    assert!(flac_bytes.len() > 4);
    assert_eq!(&flac_bytes[0..4], b"fLaC");
}

#[test]
fn test_offline_pipeline_rejects_nonseekable_output_for_dsf() {
    let in_rate = PcmRateHz::PCM_44100;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.01, 1000.0, 0.5);

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(left.len() as u64),
        bits_per_sample: Some(16),
    };

    let mut input = MockAudioInput::new(info, left, right, true);
    let dummy_sink = StreamSink(Vec::new()); // Non-seekable stream (e.g. stdout)

    let settings = Settings::default_offline(OutputContainer::Dsf);
    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");

    let err = pipeline
        .run(&mut input, Box::new(dummy_sink))
        .expect_err("must reject non-seekable output for DSF");

    match err {
        EngineError::SeekableOutputRequired { container } => {
            assert_eq!(container, "dsf");
        }
        other => panic!("expected SeekableOutputRequired, got: {other:?}"),
    }
}

#[test]
fn test_offline_pipeline_rejects_unseekable_input_for_two_pass() {
    let in_rate = PcmRateHz::PCM_44100;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.01, 1000.0, 0.5);

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(left.len() as u64),
        bits_per_sample: Some(16),
    };

    // allow_rewind = false (simulates unseekable stdin stream)
    let mut input = MockAudioInput::new(info, left, right, false);
    let mut out_buffer = Cursor::new(Vec::new());

    let mut settings = Settings::default_offline(OutputContainer::Wav);
    if let Some(ref mut gain) = settings.gain {
        gain.peak_analysis = PeakAnalysisMode::TwoPass;
    }

    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");
    let err = pipeline
        .run(&mut input, Box::new(SeekableSink(&mut out_buffer)))
        .expect_err("must reject unseekable input in two-pass mode");

    match err {
        EngineError::TwoPassRequiresSeekableInput => {}
        other => panic!("expected TwoPassRequiresSeekableInput, got: {other:?}"),
    }
}

#[test]
fn test_offline_pipeline_cross_family_48k_to_dsd256() {
    let in_rate = PcmRateHz::PCM_48000;
    let (left, right) = generate_synthetic_stereo_sine(in_rate, 0.05, 1000.0, 0.5);
    let total_frames = left.len() as u64;

    let info = AudioStreamInfo {
        sample_rate: in_rate,
        original_channels: 2,
        total_frames: Some(total_frames),
        bits_per_sample: Some(16),
    };

    let mut input = MockAudioInput::new(info, left, right, true);
    let mut out_buffer = Cursor::new(Vec::new());

    // Target is DSD256 (44.1k family: 11.2896 MHz), requiring 48k -> 44.1k rational SRC
    let settings = Settings::default_offline(OutputContainer::Dsf);
    let pipeline = OfflinePipeline::prepare(settings, in_rate).expect("prepare failed");

    let report = pipeline
        .run(&mut input, Box::new(SeekableSink(&mut out_buffer)))
        .expect("cross-family conversion failed");

    assert_eq!(report.input_rate, in_rate);
    assert_eq!(report.target_format, TargetFormat::Dsd);
    assert_eq!(report.overload_count, 0);

    let dsf_bytes = out_buffer.into_inner();
    let (header, _, _) =
        audigma_format::dsf::parse_stereo_dsf_file(&dsf_bytes).expect("valid DSF file");
    assert_eq!(header.bit_rate.get(), 11_289_600);
}
