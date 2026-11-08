//! Integration tests for structured logging and CLI flags.

use std::process::Command;

#[test]
fn test_cli_logging_flags_and_exit_code() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let cfg = write_config(
        "logging_flags",
        "schema_version = 1\n[target]\nformat = \"dsd\"\ndsd_rate = \"dsd64\"\n",
    );

    // 1. Valid command with log flags succeeds
    let output = Command::new(bin)
        .arg("--log-format")
        .arg("text")
        .arg("--log-level")
        .arg("info")
        .arg("config")
        .arg("explain")
        .arg("-c")
        .arg(&cfg)
        .output()
        .expect("failed to run audigma config explain");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Filter:        linear, apodization off"));

    // 2. A configuration error is reported without an abrupt abort (removed `preset` key)
    let bad = write_config(
        "logging_bad",
        "schema_version = 1\n[target]\nformat = \"dsd\"\ndsd_rate = \"dsd64\"\n[resampler]\npreset = \"balanced\"\n",
    );
    let output = Command::new(bin)
        .arg("--log-format")
        .arg("json")
        .arg("config")
        .arg("check")
        .arg("-c")
        .arg(&bad)
        .output()
        .expect("failed to run audigma config check");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("preset"), "stderr: {stderr}");

    // 3. audigma run missing config returns failure cleanly
    let output = Command::new(bin)
        .arg("--log-format")
        .arg("json")
        .arg("run")
        .arg("-c")
        .arg("/nonexistent/audigma/config.toml")
        .output()
        .expect("failed to run audigma run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("configuration file not found"));
}

#[test]
fn test_cli_env_var_logging() {
    let bin = env!("CARGO_BIN_EXE_audigma");
    let cfg = write_config(
        "env_logging",
        "schema_version = 1\n[target]\nformat = \"dsd\"\ndsd_rate = \"dsd64\"\n[resampler]\napodization = 18500\n",
    );

    let output = Command::new(bin)
        .env("AUDIGMA_LOG_FORMAT", "json")
        .env("AUDIGMA_LOG", "debug")
        .arg("config")
        .arg("explain")
        .arg("-c")
        .arg(&cfg)
        .output()
        .expect("failed to run audigma config explain");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("apodization 18500 Hz"));
}

fn write_config(name: &str, toml: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "audigma_cli_logging_{name}_{}.toml",
        std::process::id()
    ));
    std::fs::write(&path, toml).unwrap();
    path
}
