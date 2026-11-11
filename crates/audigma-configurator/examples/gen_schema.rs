//! Regenerates `web/src/generated/`: the two JSON Schemas from the Rust types, then `types.ts`
//! and `zod.ts` from them (`bun run gen`, needs `bun install` once in `web/`).
//!
//! ```text
//! cargo run -p audigma-configurator --example gen_schema
//! ```

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web");
    let out = web.join("src/generated");
    std::fs::create_dir_all(&out).expect("create generated dir");
    for (name, text) in audigma_configurator::schema::generated_files() {
        std::fs::write(out.join(name), text).expect("write schema");
        println!("wrote {}", out.join(name).display());
    }
    let status = Command::new("bun")
        .args(["run", "gen"])
        .current_dir(&web)
        .status()
        .expect("run bun (install it, then `bun install` in crates/audigma-configurator/web)");
    assert!(status.success(), "bun run gen failed");
}
