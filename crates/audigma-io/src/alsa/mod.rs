//! ALSA audio interface modules (`Design.md §2`, `§7`, `Plan.md Phase 3`).

pub mod output;
pub mod probe;
pub mod watcher;

pub use output::{AlsaAudioOutput, AlsaOutputConfig, AlsaOutputError};
pub use probe::{
    ActiveStreamStatus, AlsaCardReport, AlsaDeviceReport, AlsaProbeReport, DeviceCapabilities,
    DopSupport, NativeDsdSupport, UsbAltsetInfo, probe_alsa_devices,
};
pub use watcher::{
    AlsaDeviceWatcher, is_alsa_device_present, resolve_card_index, wait_for_alsa_device,
};
