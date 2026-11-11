//! Integration tests for audigma-cli commands (`Design.md §5.3`, `§7`).

use std::fs::File;
use std::io::Write;
use std::process::Command;

#[test]
fn test_cli_presets_subcommand_is_gone() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let output = Command::new(bin)
        .arg("presets")
        .arg("list")
        .output()
        .expect("failed to run audigma presets list");
    assert!(!output.status.success());
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
phase = "minimum"
apodization = "gentle"
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
    assert!(explain_json.contains("\"phase\": \"minimum\""));
    assert!(explain_json.contains("\"apodization\": 19000"));
    assert!(explain_json.contains("\"input_rate_hz\": 44100"));

    let _ = std::fs::remove_file(cfg_path);
}

#[test]
fn test_cli_config_check_prints_warnings() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let cfg_path = std::env::temp_dir().join("test_audigma_cli_warn_cfg.toml");
    let toml = r#"
schema_version = 1

[output]
backend = "pipewire"

[target]
format = "pcm"
pcm_rate_hz = 192000

[resampler]
drift_correction = "on"
"#;
    File::create(&cfg_path)
        .unwrap()
        .write_all(toml.as_bytes())
        .unwrap();
    let output = Command::new(bin)
        .args(["config", "check", "-c"])
        .arg(&cfg_path)
        .output()
        .expect("failed to run audigma config check");
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning:") && stderr.contains("drift_correction"));
    let _ = std::fs::remove_file(cfg_path);
}

/// The output extension is the only container selector: an unknown one is an error, a DSD
/// target cannot go to `.wav`/`.flac`, and the removed `[output] container` key is rejected.
#[test]
fn test_cli_convert_container_comes_from_the_extension() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let dir = std::env::temp_dir();
    let cfg_dsd = dir.join("audigma_test_ext_dsd.toml");
    let cfg_old = dir.join("audigma_test_ext_old_key.toml");
    std::fs::write(
        &cfg_dsd,
        "schema_version = 1\n[target]\nformat = \"dsd\"\nrate_family = \"44100\"\ndsd_rate = \"dsd256\"\n",
    )
    .unwrap();
    std::fs::write(
        &cfg_old,
        "schema_version = 1\n[output]\ncontainer = \"wav\"\n",
    )
    .unwrap();

    let run = |cfg: &std::path::Path, out: &str| {
        let o = Command::new(bin)
            .args(["convert", "-i", "-", "-o", out, "-c"])
            .arg(cfg)
            .output()
            .expect("failed to run audigma convert");
        assert!(!o.status.success(), "{out} must fail");
        String::from_utf8_lossy(&o.stderr).to_string()
    };
    let dir_s = |n: &str| dir.join(n).to_string_lossy().to_string();

    let err = run(&cfg_dsd, &dir_s("audigma_test_ext.bin"));
    assert!(err.contains("cannot tell the output container"), "{err}");
    let err = run(&cfg_dsd, &dir_s("audigma_test_ext.wav"));
    assert!(
        err.contains("target.format") && err.contains(".wav"),
        "{err}"
    );
    let err = run(&cfg_dsd, &dir_s("audigma_test_ext.flac"));
    assert!(
        err.contains("target.format") && err.contains(".flac"),
        "{err}"
    );
    let err = run(&cfg_old, &dir_s("audigma_test_ext.wav"));
    assert!(err.contains("container"), "{err}");
    // Writing a header container to stdout is not possible: `-o -` is a raw stream.
    let err = run(&cfg_dsd, "-");
    assert!(err.contains("error"), "{err}");

    let _ = std::fs::remove_file(cfg_dsd);
    let _ = std::fs::remove_file(cfg_old);
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

/// FLAC output writes the documented default `s32le` as 32 bit (no silent narrowing), and an
/// explicit `s16le` as 16 bit.
#[test]
fn test_cli_convert_flac_bit_depth_follows_sample_format() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let dir = std::env::temp_dir();
    let in_wav = dir.join("audigma_test_in_flac_depth.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&in_wav, spec).unwrap();
    for i in 0..4410 {
        let t = i as f64 / 44100.0;
        let s = (0.3 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin() * 32767.0) as i16;
        writer.write_sample(s).unwrap();
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();

    for (name, sample_format, expected_bits) in [
        ("default", None, "32"),
        ("s24le", Some("s24le"), "24"),
        ("s16le", Some("s16le"), "16"),
    ] {
        let out_flac = dir.join(format!("audigma_test_out_flac_depth_{name}.flac"));
        let cfg = dir.join(format!("audigma_test_flac_depth_{name}.toml"));
        let format_line = sample_format
            .map(|f| format!("sample_format = \"{f}\"\n"))
            .unwrap_or_default();
        std::fs::write(
            &cfg,
            format!(
                "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 44100\n{format_line}"
            ),
        )
        .unwrap();

        let conv = Command::new(bin)
            .arg("convert")
            .arg("-i")
            .arg(&in_wav)
            .arg("-o")
            .arg(&out_flac)
            .arg("-c")
            .arg(&cfg)
            .output()
            .expect("failed to run audigma convert");
        assert!(
            conv.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&conv.stderr)
        );

        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=bits_per_raw_sample",
                "-of",
                "csv=p=0",
            ])
            .arg(&out_flac)
            .output()
            .expect("failed to run ffprobe");
        assert!(probe.status.success(), "{name}: ffprobe failed");
        assert_eq!(
            String::from_utf8_lossy(&probe.stdout).trim(),
            expected_bits,
            "{name}"
        );
        let _ = std::fs::remove_file(out_flac);
        let _ = std::fs::remove_file(cfg);
    }
    let _ = std::fs::remove_file(in_wav);
}
