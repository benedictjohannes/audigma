pub mod alsa;
pub mod cpu_topology;
#[cfg(feature = "dbus")]
pub mod dbus;
pub mod file;
#[cfg(feature = "pipewire")]
pub mod pipewire;
#[cfg(feature = "pipewire")]
pub mod pipewire_probe;
#[cfg(feature = "pipewire")]
pub mod pipewire_watch;
pub mod runtime;
#[cfg(target_os = "linux")]
pub mod sched;
pub mod stdio;

#[cfg(feature = "pipewire")]
pub use self::pipewire::{PipeWireError, PipeWirePcmOutput, PipeWireVirtualSink};
#[cfg(feature = "pipewire")]
pub use self::pipewire_probe::{
    PipeWireProbeReport, PipeWireSinkInfo, link_targets, output_link_targets, probe_pipewire_sinks,
    sink_from_props,
};
#[cfg(feature = "pipewire")]
#[cfg(feature = "pipewire")]
pub use self::pipewire_watch::PipeWireSinkWatcher;
pub use alsa::{
    AlsaAudioOutput, AlsaCardReport, AlsaDeviceReport, AlsaDeviceWatcher, AlsaHardwareMixer,
    AlsaOutputConfig, AlsaOutputError, AlsaProbeReport, MixerControlReport, is_alsa_device_present,
    probe_alsa_devices, resolve_card_index, wait_for_alsa_device,
};

#[cfg(feature = "dbus")]
pub use dbus::{DBusError, DeviceReservation, rtkit_make_realtime};
pub use file::{FileAudioInput, FileAudioOutput};
pub use stdio::{StdioAudioInput, StdioAudioOutput};

#[cfg(target_os = "linux")]
pub use sched::{AppliedPlacement, RealtimePriorityAcquisition, SchedError, apply};

pub use cpu_topology::{
    TopologyError, current_affinity, parse_cpu_list, read_cpu_topology, read_cpu_topology_from,
};
