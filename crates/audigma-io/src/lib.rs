//! `audigma-io`: Concrete I/O drivers, hardware capability probing, and OS audio interfaces
//! (`Design.md §2`, `§5.3`).

pub mod alsa;
pub mod file;
pub mod stdio;

pub use alsa::{
    AlsaAudioOutput, AlsaCardReport, AlsaDeviceReport, AlsaOutputConfig, AlsaOutputError,
    AlsaProbeReport, probe_alsa_devices,
};
pub use file::{FileAudioInput, FileAudioOutput};
pub use stdio::{StdioAudioInput, StdioAudioOutput};
