//! Embeds the web configurator page. Never runs Bun: it copies `web/dist/index.html` when it
//! exists, otherwise the committed fallback page (see `build/embed.rs`).

#[path = "build/embed.rs"]
mod embed;

use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("out dir")).join("index.html");
    let dist = dir.join("web/dist/index.html");
    let fallback = dir.join("src/web/fallback.html");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-changed={}", fallback.display());
    println!("cargo:rerun-if-changed=build/embed.rs");
    println!("cargo:rerun-if-env-changed=AUDIGMA_REQUIRE_UI");
    let require = std::env::var("AUDIGMA_REQUIRE_UI").is_ok_and(|v| v == "1");
    match embed::choose(&dist, &fallback, require) {
        Ok(choice) => {
            let (path, built) = match choice {
                embed::Choice::Built(p) => (p, "1"),
                embed::Choice::Fallback(p) => (p, "0"),
            };
            std::fs::copy(&path, &out).expect("copy the page to OUT_DIR");
            println!("cargo:rustc-env=AUDIGMA_UI_EMBEDDED={built}");
        }
        Err(why) => {
            eprintln!("error: {why}");
            std::process::exit(1);
        }
    }
}
