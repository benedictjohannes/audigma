//! ALSA audio interface modules (`Design.md §2`, `§7`, `Plan.md Phase 3`).

pub mod output;
pub mod probe;

pub use output::{AlsaAudioOutput, AlsaOutputConfig, AlsaOutputError};
pub use probe::{
    ActiveStreamStatus, AlsaCardReport, AlsaDeviceReport, AlsaProbeReport, DeviceCapabilities,
    DopSupport, NativeDsdSupport, UsbAltsetInfo, probe_alsa_devices,
};
