use audigma_engine::traits::{AudioInput, AudioOutput};
use audigma_io::stdio::{StdioAudioInput, StdioAudioOutput};
use std::io::Write;

#[test]
fn test_stdio_input_and_output_basics() {
    let out = StdioAudioOutput::new();
    assert!(!out.can_seek());

    // Input metadata
    let in_stream = StdioAudioInput::new(
        audigma_dsp::config::SampleFormat::S16Le,
        audigma_dsp::PcmRateHz::PCM_44100,
        2,
    );
    assert!(!in_stream.can_rewind());
    assert_eq!(
        in_stream.info().sample_rate,
        audigma_dsp::PcmRateHz::PCM_44100
    );
    assert_eq!(in_stream.info().original_channels, 2);
}

#[test]
fn test_file_input_and_output_seekability() {
    let temp_dir = std::env::temp_dir();
    let test_file = temp_dir.join("audigma_test_out.raw");

    let mut out = audigma_io::file::FileAudioOutput::create(&test_file).unwrap();
    assert!(out.can_seek());
    out.write_all(b"test").unwrap();
    out.flush().unwrap();

    let _ = std::fs::remove_file(test_file);
}

#[test]
fn test_file_audio_input_dsf_reading_and_decimation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let dsf_path = temp_dir.path().join("test_in.dsf");

    let bit_rate = audigma_dsp::DsdBitRateHz::DSD128;
    let num_bytes = 8192; // 2 blocks of 4096
    let left_bytes: Vec<u8> = (0..num_bytes).map(|i| (i % 256) as u8).collect();
    let right_bytes: Vec<u8> = (0..num_bytes).map(|i| ((i + 77) % 256) as u8).collect();

    let dsf_file_bytes = audigma_format::dsf::pack_stereo_dsf_file(
        bit_rate,
        &left_bytes,
        &right_bytes,
        audigma_format::dsf::DsfBitOrder::LsbFirst,
        audigma_format::dsf::DsfPadMode::SpecZero,
    )
    .expect("pack dsf file");
    std::fs::write(&dsf_path, &dsf_file_bytes).unwrap();

    let mut file_in = audigma_io::file::FileAudioInput::open(&dsf_path).expect("open dsf input");
    assert!(file_in.info().is_dsd());
    assert_eq!(file_in.info().dsd_bit_rate, Some(bit_rate));
    assert!(file_in.can_rewind());

    // 1. Read raw DSD bytes
    let mut read_l = vec![0u8; 4096];
    let mut read_r = vec![0u8; 4096];
    let bytes_read = file_in
        .read_dsd_bytes(&mut [&mut read_l, &mut read_r])
        .expect("read dsd bytes");
    assert_eq!(bytes_read, 4096);
    assert_eq!(&read_l[..], &left_bytes[..4096]);
    assert_eq!(&read_r[..], &right_bytes[..4096]);

    // 2. Rewind
    file_in.rewind().expect("rewind");

    // 3. Read PCM decimated frames (with default +6.0 dB Scarlet Book gain, abs <= 2.0)
    let mut pcm_l = vec![0.0f64; 256];
    let mut pcm_r = vec![0.0f64; 256];
    let pcm_read = file_in
        .read_stereo(&mut pcm_l, &mut pcm_r)
        .expect("read stereo decimated");
    assert_eq!(pcm_read, 256);
    for &s in &pcm_l {
        assert!(s.is_finite() && s.abs() <= 2.0);
    }

    // 4. Also verify unity (0.0 dB) gain with open_with_dsd_gain
    let mut file_in_unity = audigma_io::file::FileAudioInput::open_with_dsd_gain(&dsf_path, 0.0)
        .expect("open dsf unity");
    let mut pcm_u_l = vec![0.0f64; 256];
    let mut pcm_u_r = vec![0.0f64; 256];
    let read_u = file_in_unity
        .read_stereo(&mut pcm_u_l, &mut pcm_u_r)
        .expect("read stereo unity");
    assert_eq!(read_u, 256);
    for &s in &pcm_u_l {
        assert!(s.is_finite() && s.abs() <= 1.0);
    }
}

#[test]
fn test_offline_pipeline_dsf_to_dsf_passthrough() {
    let temp_dir = tempfile::tempdir().unwrap();
    let in_dsf_path = temp_dir.path().join("in_dsd256.dsf");
    let out_dsf_path = temp_dir.path().join("out_dsd256.dsf");

    let bit_rate = audigma_dsp::DsdBitRateHz::DSD256;
    let num_bytes = 8192; // 2 blocks
    let left_bytes: Vec<u8> = (0..num_bytes).map(|i| (i % 256) as u8).collect();
    let right_bytes: Vec<u8> = (0..num_bytes).map(|i| ((i + 13) % 256) as u8).collect();

    let dsf_file_bytes = audigma_format::dsf::pack_stereo_dsf_file(
        bit_rate,
        &left_bytes,
        &right_bytes,
        audigma_format::dsf::DsfBitOrder::LsbFirst,
        audigma_format::dsf::DsfPadMode::SpecZero,
    )
    .expect("pack dsf file");
    std::fs::write(&in_dsf_path, &dsf_file_bytes).unwrap();

    let mut file_in = audigma_io::file::FileAudioInput::open(&in_dsf_path).expect("open dsf");
    let file_out = audigma_io::file::FileAudioOutput::create(&out_dsf_path).expect("create dsf");

    let settings =
        audigma_settings::Settings::default_offline(audigma_settings::OutputContainer::Dsf);
    let pipeline = audigma_engine::OfflinePipeline::prepare(settings, file_in.info().sample_rate)
        .expect("prepare pipeline");

    let report = pipeline
        .run(&mut file_in, Box::new(file_out))
        .expect("run passthrough");

    assert_eq!(report.target_format, audigma_dsp::config::TargetFormat::Dsd);
    assert_eq!(report.total_input_frames, (num_bytes * 8) as u64);
    assert_eq!(report.overload_count, 0);

    // Read back output DSF file
    let out_bytes = std::fs::read(&out_dsf_path).expect("read out dsf");
    let (header, out_l, out_r) =
        audigma_format::dsf::parse_stereo_dsf_file(&out_bytes).expect("valid out dsf");
    assert_eq!(header.bit_rate, bit_rate);
    assert_eq!(out_l, left_bytes);
    assert_eq!(out_r, right_bytes);
}

#[test]
fn test_offline_pipeline_dsf_remodulation_dsd64_to_dsd256() {
    let temp_dir = tempfile::tempdir().unwrap();
    let in_dsf_path = temp_dir.path().join("in_dsd64.dsf");
    let out_dsf_path = temp_dir.path().join("out_dsd256.dsf");

    let bit_rate_in = audigma_dsp::DsdBitRateHz::DSD64;
    let num_bytes = 4096; // 1 block
    let left_bytes: Vec<u8> = vec![0x69; num_bytes]; // DSD silence pattern
    let right_bytes: Vec<u8> = vec![0x69; num_bytes];

    let dsf_file_bytes = audigma_format::dsf::pack_stereo_dsf_file(
        bit_rate_in,
        &left_bytes,
        &right_bytes,
        audigma_format::dsf::DsfBitOrder::LsbFirst,
        audigma_format::dsf::DsfPadMode::SpecZero,
    )
    .expect("pack dsf file");
    std::fs::write(&in_dsf_path, &dsf_file_bytes).unwrap();

    let mut file_in = audigma_io::file::FileAudioInput::open(&in_dsf_path).expect("open dsf");
    let file_out = audigma_io::file::FileAudioOutput::create(&out_dsf_path).expect("create dsf");

    // Target is DSD256
    let settings =
        audigma_settings::Settings::default_offline(audigma_settings::OutputContainer::Dsf);
    let pipeline = audigma_engine::OfflinePipeline::prepare(settings, file_in.info().sample_rate)
        .expect("prepare pipeline");

    let report = pipeline
        .run(&mut file_in, Box::new(file_out))
        .expect("run remodulation");

    assert_eq!(report.target_format, audigma_dsp::config::TargetFormat::Dsd);
    assert_eq!(report.overload_count, 0);

    // Read back output DSF file
    let out_bytes = std::fs::read(&out_dsf_path).expect("read out dsf");
    let (header, _, _) =
        audigma_format::dsf::parse_stereo_dsf_file(&out_bytes).expect("valid out dsf");
    assert_eq!(header.bit_rate.get(), 11_289_600); // DSD256
}

#[test]
fn test_offline_pipeline_dsf_decimation_to_wav() {
    let temp_dir = tempfile::tempdir().unwrap();
    let in_dsf_path = temp_dir.path().join("in_dsd64.dsf");
    let out_wav_path = temp_dir.path().join("out.wav");

    let bit_rate_in = audigma_dsp::DsdBitRateHz::DSD64;
    let num_bytes = 4096;
    let left_bytes: Vec<u8> = vec![0x69; num_bytes];
    let right_bytes: Vec<u8> = vec![0x69; num_bytes];

    let dsf_file_bytes = audigma_format::dsf::pack_stereo_dsf_file(
        bit_rate_in,
        &left_bytes,
        &right_bytes,
        audigma_format::dsf::DsfBitOrder::LsbFirst,
        audigma_format::dsf::DsfPadMode::SpecZero,
    )
    .expect("pack dsf file");
    std::fs::write(&in_dsf_path, &dsf_file_bytes).unwrap();

    let mut file_in = audigma_io::file::FileAudioInput::open(&in_dsf_path).expect("open dsf");
    let file_out = audigma_io::file::FileAudioOutput::create(&out_wav_path).expect("create wav");

    let settings =
        audigma_settings::Settings::default_offline(audigma_settings::OutputContainer::Wav);
    let pipeline = audigma_engine::OfflinePipeline::prepare(settings, file_in.info().sample_rate)
        .expect("prepare pipeline");

    let report = pipeline
        .run(&mut file_in, Box::new(file_out))
        .expect("run wav decimation");

    assert_eq!(report.target_format, audigma_dsp::config::TargetFormat::Pcm);

    // Read back with hound
    let reader = hound::WavReader::open(&out_wav_path).expect("valid wav header");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 192000);
}
