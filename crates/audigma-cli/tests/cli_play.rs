//! Integration tests for `audigma play` CLI command (`Design.md §7`, `Plan.md Phase 3`).

use std::process::Command;

#[test]
fn test_cli_play_help() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("play")
        .arg("--help")
        .output()
        .expect("failed to run audigma play --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--input"));
    assert!(stdout.contains("--device"));
    assert!(stdout.contains("--preset"));
    assert!(stdout.contains("--volume"));
    assert!(stdout.contains("--override"));
    assert!(stdout.contains("--config"));
}

#[test]
fn test_cli_play_missing_file_errors() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("play")
        .arg("-i")
        .arg("non_existent_file.wav")
        .output()
        .expect("failed to run audigma play");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("audigma error:"));
}

#[test]
fn test_cli_play_invalid_override_syntax() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("play")
        .arg("-i")
        .arg("test.wav")
        .arg("-o")
        .arg("invalid_override_format")
        .output()
        .expect("failed to run audigma play");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid override syntax"));
}

#[test]
fn test_cli_play_valid_override_parsed() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    // With non-existent file, it parses overrides first and fails at input open
    let output = Command::new(bin)
        .arg("play")
        .arg("-i")
        .arg("non_existent_file.wav")
        .arg("-o")
        .arg("output.transport=dop")
        .arg("-o")
        .arg("output.buffer_ms=25")
        .output()
        .expect("failed to run audigma play");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Overrides succeeded, failure occurred at input file
    assert!(stderr.contains("audigma error:"));
    assert!(!stderr.contains("invalid override syntax"));
}
