//! Which page `GET /` embeds. Shared by `build.rs` and `tests/embed.rs` (via `#[path]`), so the
//! decision is tested without running a build.

use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Choice {
    /// The Vite build output.
    Built(PathBuf),
    /// The committed page that says this build has no UI.
    Fallback(PathBuf),
}

/// `dist` is `web/dist/index.html`, `fallback` the committed page. With `require` (the release
/// build, `AUDIGMA_REQUIRE_UI=1`) a missing `dist` is an error instead of a silent fallback.
pub fn choose(dist: &Path, fallback: &Path, require: bool) -> Result<Choice, String> {
    if dist.is_file() {
        Ok(Choice::Built(dist.to_path_buf()))
    } else if require {
        Err(format!(
            "AUDIGMA_REQUIRE_UI=1 but {} does not exist; run `bun install --frozen-lockfile && \
             bun run build` in crates/audigma-configurator/web first",
            dist.display()
        ))
    } else {
        Ok(Choice::Fallback(fallback.to_path_buf()))
    }
}
