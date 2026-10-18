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
