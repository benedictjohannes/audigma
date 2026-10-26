//! Integration tests for structured logging and CLI flags.

use std::process::Command;

#[test]
fn test_cli_logging_flags_and_exit_code() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    // 1. Valid command with log flags succeeds
    let output = Command::new(bin)
        .arg("--log-format")
        .arg("text")
        .arg("--log-level")
        .arg("info")
        .arg("presets")
        .arg("show")
        .arg("balanced")
        .output()
        .expect("failed to run audigma presets show balanced");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Preset: balanced"));

    // 2. Refactored run_presets returns error without abrupt abort
    let output = Command::new(bin)
        .arg("--log-format")
        .arg("json")
        .arg("presets")
        .arg("show")
        .arg("nonexistent_preset")
        .output()
        .expect("failed to run audigma presets show");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown preset 'nonexistent_preset'"));

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

    let output = Command::new(bin)
        .env("AUDIGMA_LOG_FORMAT", "json")
        .env("AUDIGMA_LOG", "debug")
        .arg("presets")
        .arg("list")
        .output()
        .expect("failed to run audigma presets list");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("balanced"));
    assert!(stdout.contains("reference"));
}
