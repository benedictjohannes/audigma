//! ALSA audio interface modules (`Design.md §2`, `§7`).

pub mod mixer;
pub mod output;
pub mod probe;
pub mod watcher;

pub use mixer::{AlsaHardwareMixer, CANDIDATE_MIXER_CONTROLS, resolve_card_mixer_name};
pub use output::{AlsaAudioOutput, AlsaFormatRequest, AlsaOutputConfig, AlsaOutputError};
pub use probe::{
    ActiveStreamStatus, AlsaCardReport, AlsaDeviceReport, AlsaProbeReport, DeviceCapabilities,
    DopSupport, MixerControlReport, NativeDsdSupport, UsbAltsetInfo, probe_alsa_devices,
};
pub use watcher::{
    AlsaDeviceWatcher, is_alsa_device_present, resolve_card_index, wait_for_alsa_device,
};
