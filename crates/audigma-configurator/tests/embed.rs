//! The embedded page: which file `build.rs` picks, and that `GET /` serves it.

#[path = "../build/embed.rs"]
mod embed;

use std::path::Path;

use embed::{Choice, choose};

#[test]
fn missing_dist_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("index.html");
    let fallback = Path::new("fallback.html");
    assert_eq!(
        choose(&dist, fallback, false),
        Ok(Choice::Fallback(fallback.to_path_buf()))
    );
}

#[test]
fn present_dist_is_embedded() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("index.html");
    std::fs::write(&dist, "<html></html>").unwrap();
    for require in [false, true] {
        assert_eq!(
            choose(&dist, Path::new("fallback.html"), require),
            Ok(Choice::Built(dist.clone()))
        );
    }
}

#[test]
fn require_ui_without_dist_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = choose(
        &dir.path().join("index.html"),
        Path::new("fallback.html"),
        true,
    )
    .unwrap_err();
    assert!(err.contains("AUDIGMA_REQUIRE_UI=1"), "{err}");
    assert!(err.contains("bun run build"), "{err}");
}

/// Compiles and runs the real `build.rs` against a fake crate directory without `web/dist`.
#[test]
fn build_script_fails_without_dist_when_required() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let work = tempfile::tempdir().unwrap();
    let exe = work.path().join("build_script");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let built = std::process::Command::new(rustc)
        .args(["--edition", "2024", "--crate-name", "build_script", "-o"])
        .arg(&exe)
        .arg(manifest.join("build.rs"))
        .output()
        .expect("run rustc");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );

    // A crate directory with the fallback page but no `web/dist`.
    let krate = work.path().join("crate");
    std::fs::create_dir_all(krate.join("src/web")).unwrap();
    std::fs::write(krate.join("src/web/fallback.html"), "<html>fallback</html>").unwrap();
    let out = work.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let run = |require: Option<&str>| {
        let mut cmd = std::process::Command::new(&exe);
        cmd.env("CARGO_MANIFEST_DIR", &krate).env("OUT_DIR", &out);
        cmd.env_remove("AUDIGMA_REQUIRE_UI");
        if let Some(v) = require {
            cmd.env("AUDIGMA_REQUIRE_UI", v);
        }
        cmd.output().expect("run build script")
    };

    let strict = run(Some("1"));
    assert!(!strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("AUDIGMA_REQUIRE_UI=1"));

    let lax = run(None);
    assert!(lax.status.success());
    assert!(String::from_utf8_lossy(&lax.stdout).contains("AUDIGMA_UI_EMBEDDED=0"));
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<html>fallback</html>"
    );

    // With a built page present, the built page wins even when it is required.
    std::fs::create_dir_all(krate.join("web/dist")).unwrap();
    std::fs::write(krate.join("web/dist/index.html"), "<html>built</html>").unwrap();
    let strict = run(Some("1"));
    assert!(strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stdout).contains("AUDIGMA_UI_EMBEDDED=1"));
    assert_eq!(
        std::fs::read_to_string(out.join("index.html")).unwrap(),
        "<html>built</html>"
    );
}
