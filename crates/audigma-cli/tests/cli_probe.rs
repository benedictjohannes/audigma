//! Integration tests for `audigma probe` (`Design.md §7`).

use std::process::Command;

#[test]
fn test_cli_probe_alsa_text() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("probe")
        .arg("--backend")
        .arg("alsa")
        .output()
        .expect("failed to run audigma probe");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ALSA Hardware Capability Probe Report"));
    assert!(stdout.contains("sound card(s)"));
}

#[test]
fn test_cli_probe_alsa_json() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("probe")
        .arg("--backend")
        .arg("alsa")
        .arg("--json")
        .output()
        .expect("failed to run audigma probe --json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(parsed.get("cards").is_some());
    let cards = parsed["cards"].as_array().expect("cards must be an array");
    assert!(!cards.is_empty());
}

#[test]
fn test_cli_probe_pipewire() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("probe")
        .arg("--backend")
        .arg("pipewire")
        .arg("--json")
        .output()
        .expect("failed to run audigma probe --backend pipewire --json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert_eq!(parsed["backend"], "pipewire");
}

#[test]
fn test_cli_probe_unknown_backend() {
    let bin = env!("CARGO_BIN_EXE_audigma");

    let output = Command::new(bin)
        .arg("probe")
        .arg("--backend")
        .arg("unsupported_backend")
        .output()
        .expect("failed to run audigma probe");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown audio backend"));
}
