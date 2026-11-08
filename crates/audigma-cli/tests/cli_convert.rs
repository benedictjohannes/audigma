//! Integration tests for audigma-cli commands (`Design.md §5.3`, `§7`).

use std::fs::File;
use std::io::Write;
use std::process::Command;

#[test]
fn test_cli_presets_list_and_show() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output_list = Command::new(bin)
        .arg("presets")
        .arg("list")
        .output()
        .expect("failed to run audigma presets list");
    assert!(output_list.status.success());
    let list_str = String::from_utf8_lossy(&output_list.stdout);
    assert!(list_str.contains("balanced"));
    assert!(list_str.contains("reference"));
    assert!(list_str.contains("low-latency"));

    let output_show = Command::new(bin)
        .arg("presets")
        .arg("show")
        .arg("reference")
        .output()
        .expect("failed to run audigma presets show reference");
    assert!(output_show.status.success());
    let show_str = String::from_utf8_lossy(&output_show.stdout);
    assert!(show_str.contains("Preset: reference"));
    assert!(show_str.contains("140.0 dB"));
}

#[test]
fn test_cli_config_check_and_explain() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let temp_dir = std::env::temp_dir();
    let cfg_path = temp_dir.join("test_audigma_cli_cfg.toml");

    let toml = r#"
schema_version = 1

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"

[resampler]
preset = "reference"
"#;
    let mut f = File::create(&cfg_path).unwrap();
    f.write_all(toml.as_bytes()).unwrap();

    // Check
    let output_check = Command::new(bin)
        .arg("config")
        .arg("check")
        .arg("-c")
        .arg(&cfg_path)
        .output()
        .expect("failed to run audigma config check");
    assert!(output_check.status.success());

    // Explain JSON
    let output_explain = Command::new(bin)
        .arg("config")
        .arg("explain")
        .arg("-c")
        .arg(&cfg_path)
        .arg("--input-rate")
        .arg("44100")
        .arg("--json")
        .output()
        .expect("failed to run audigma config explain");
    assert!(output_explain.status.success());
    let explain_json = String::from_utf8_lossy(&output_explain.stdout);
    assert!(explain_json.contains("\"preset\": \"reference\""));
    assert!(explain_json.contains("\"input_rate_hz\": 44100"));

    let _ = std::fs::remove_file(cfg_path);
}

#[test]
fn test_cli_convert_rejects_non_seekable_stdout_for_dsf() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let temp_dir = std::env::temp_dir();
    let cfg_path = temp_dir.join("test_audigma_stdout_dsf.toml");

    let toml = r#"
schema_version = 1

[output]
container = "dsf"

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"
"#;
    let mut f = File::create(&cfg_path).unwrap();
    f.write_all(toml.as_bytes()).unwrap();

    let output = Command::new(bin)
        .arg("convert")
        .arg("-i")
        .arg("-")
        .arg("-o")
        .arg("-")
        .arg("-c")
        .arg(&cfg_path)
        .output()
        .expect("failed to run audigma convert");

    assert!(!output.status.success());
    let err_str = String::from_utf8_lossy(&output.stderr);
    assert!(err_str.contains("error") || err_str.contains("cannot write container = \"dsf\""));

    let _ = std::fs::remove_file(cfg_path);
}

#[test]
fn test_cli_convert_synthetic_wav_to_dsf_and_ffprobe_verify() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let temp_dir = std::env::temp_dir();
    let in_wav_path = temp_dir.join("audigma_test_in.wav");
    let out_dsf_path = temp_dir.join("audigma_test_out.dsf");
    let out_flac_path = temp_dir.join("audigma_test_out.flac");

    // 1. Generate clean 44.1 kHz 16-bit stereo WAV with 1 second of audio
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&in_wav_path, spec).unwrap();
    for i in 0..44100 {
        let t = i as f64 / 44100.0;
        let s = (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 32767.0) as i16;
        writer.write_sample(s).unwrap();
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();

    // 2. Convert to DSF
    let conv_dsf = Command::new(bin)
        .arg("convert")
        .arg("-i")
        .arg(&in_wav_path)
        .arg("-o")
        .arg(&out_dsf_path)
        .output()
        .expect("failed to run audigma convert to dsf");
    assert!(
        conv_dsf.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&conv_dsf.stderr)
    );

    // Verify DSF with ffprobe
    let probe_dsf = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("stream=codec_name,sample_rate,channels")
        .arg("-of")
        .arg("json")
        .arg(&out_dsf_path)
        .output()
        .expect("failed to run ffprobe on dsf");
    assert!(probe_dsf.status.success());
    let probe_dsf_str = String::from_utf8_lossy(&probe_dsf.stdout);
    assert!(probe_dsf_str.contains("dsd_lsbf") || probe_dsf_str.contains("dsd"));
    assert!(probe_dsf_str.contains("\"channels\": 2"));

    // 3. Convert to FLAC
    let conv_flac = Command::new(bin)
        .arg("convert")
        .arg("-i")
        .arg(&in_wav_path)
        .arg("-o")
        .arg(&out_flac_path)
        .output()
        .expect("failed to run audigma convert to flac");
    assert!(
        conv_flac.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&conv_flac.stderr)
    );

    // Verify FLAC with ffprobe
    let probe_flac = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("stream=codec_name,sample_rate,channels")
        .arg("-of")
        .arg("json")
        .arg(&out_flac_path)
        .output()
        .expect("failed to run ffprobe on flac");
    assert!(probe_flac.status.success());
    let probe_flac_str = String::from_utf8_lossy(&probe_flac.stdout);
    assert!(probe_flac_str.contains("\"codec_name\": \"flac\""));
    assert!(probe_flac_str.contains("\"channels\": 2"));

    let _ = std::fs::remove_file(in_wav_path);
    let _ = std::fs::remove_file(out_dsf_path);
    let _ = std::fs::remove_file(out_flac_path);
}

#[test]
fn test_cli_convert_respects_config_write_report() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let temp_dir = std::env::temp_dir();
    let in_wav_path = temp_dir.join("audigma_test_in_report.wav");
    let out_wav_path = temp_dir.join("audigma_test_out_report.wav");
    let cfg_path = temp_dir.join("audigma_test_report.toml");

    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&in_wav_path, spec).unwrap();
    for i in 0..4410 {
        let t = i as f64 / 44100.0;
        let s = (0.2 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 32767.0) as i16;
        writer.write_sample(s).unwrap();
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();

    let toml = r#"
schema_version = 1

[target]
format = "pcm"
pcm_rate_hz = 44100
sample_format = "s16le"

[output]
container = "wav"

[offline]
write_report = true
"#;
    std::fs::write(&cfg_path, toml).unwrap();

    let output = Command::new(bin)
        .arg("convert")
        .arg("-i")
        .arg(&in_wav_path)
        .arg("-o")
        .arg(&out_wav_path)
        .arg("-c")
        .arg(&cfg_path)
        .output()
        .expect("failed to run audigma convert");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Should be valid JSON with total_input_frames
    assert!(stderr.contains("\"total_input_frames\": 4410"));
    assert!(stderr.contains("\"applied_gain_db\":"));

    let _ = std::fs::remove_file(in_wav_path);
    let _ = std::fs::remove_file(out_wav_path);
    let _ = std::fs::remove_file(cfg_path);
}
