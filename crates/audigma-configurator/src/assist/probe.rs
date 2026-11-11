//! Probe assists: ALSA device detection and the PipeWire sink list.
//!
//! Thin, serialisable wrappers over `audigma-io`. The wrapper joins a PipeWire sink to the ALSA
//! card behind it (`alsa.card` = card index) so a frontend can show one list.

use audigma_io::{AlsaProbeReport, PipeWireProbeReport, PipeWireSinkInfo};
use serde::Serialize;

/// Both probes in one report. A probe that failed is reported as an error string, not a failure
/// of the other one: a machine without PipeWire can still be probed over ALSA.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub alsa: Result<AlsaProbeReport, String>,
    pub pipewire: Result<PipeWireProbeReport, String>,
}

/// Runs the ALSA probe (`alsa-detect`).
pub fn probe_alsa() -> Result<AlsaProbeReport, String> {
    audigma_io::probe_alsa_devices()
}

/// Runs the PipeWire registry probe (`pipewire-sinks`).
pub fn probe_pipewire() -> Result<PipeWireProbeReport, String> {
    audigma_io::probe_pipewire_sinks()
}

/// Runs both probes.
pub fn probe_all() -> ProbeReport {
    ProbeReport {
        alsa: probe_alsa(),
        pipewire: probe_pipewire(),
    }
}

/// The PipeWire sinks backed by ALSA card `card_index`.
pub fn sinks_for_card(report: &PipeWireProbeReport, card_index: i32) -> Vec<&PipeWireSinkInfo> {
    report
        .sinks
        .iter()
        .filter(|s| {
            s.alsa_card
                .is_some_and(|c| i64::from(c) == i64::from(card_index))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(name: &str, card: Option<u32>) -> PipeWireSinkInfo {
        PipeWireSinkInfo {
            id: 1,
            node_name: name.into(),
            description: None,
            device_api: Some("alsa".into()),
            alsa_card: card,
            alsa_card_name: None,
            alsa_path: None,
        }
    }

    #[test]
    fn sinks_join_on_alsa_card() {
        let report = PipeWireProbeReport {
            sinks: vec![sink("a", Some(2)), sink("b", Some(0)), sink("c", None)],
        };
        let found = sinks_for_card(&report, 2);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].node_name, "a");
        assert!(sinks_for_card(&report, 5).is_empty());
    }
}
