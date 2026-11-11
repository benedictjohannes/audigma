//! Field-level assists: probes, the capability check and the benchmark.
//!
//! `meta/assists.toml` declares each assist (id, kind, label, the keys it fills); this module
//! holds the implementations. [`IMPLEMENTED`] lists the ids that have one, and a test requires
//! every catalogue id to appear in it, so a declared assist cannot ship without code.

pub mod benchmark;
pub mod capability;
pub mod probe;

/// Assist ids with an implementation in this crate. `quality-starting-point` is a pure data
/// preset (applied to the draft by the frontends, Session 4+), so it has no code here.
pub const IMPLEMENTED: &[&str] = &[
    "alsa-detect",
    "pipewire-sinks",
    "capability-check",
    "benchmark-discover",
    "benchmark-validate",
    "quality-starting-point",
];

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_settings::meta::Catalog;

    #[test]
    fn every_catalogue_assist_is_implemented() {
        let catalog = Catalog::load().expect("catalog loads");
        for a in catalog.assists() {
            assert!(
                IMPLEMENTED.contains(&a.id.as_str()),
                "assist `{}` is declared in assists.toml but not listed as implemented",
                a.id
            );
        }
        for id in IMPLEMENTED {
            assert!(
                catalog.assist(id).is_some(),
                "`{id}` is not in assists.toml"
            );
        }
    }
}
