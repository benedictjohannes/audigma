//! `audigma-io`: Concrete I/O drivers, hardware capability probing, and OS audio interfaces
//! (`Design.md §2`, `§5.3`).

pub mod file;
pub mod stdio;

pub use file::{FileAudioInput, FileAudioOutput};
pub use stdio::{StdioAudioInput, StdioAudioOutput};
