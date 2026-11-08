//! Built-in verified filter and modulator preset specifications (`Design.md §2`, `§4.2–§4.3`).
//!
//! `PRESET_BALANCED`, `PRESET_REFERENCE`, and `PRESET_LOW_LATENCY` define the quality targets,
//! modulator stability limits (`L dBFS`), required profile headroom, and stage metadata.
//! `tools/design/` populates the full synthesized FIR tap arrays and 5th-order CRFB NTF state vectors.

use crate::config::{
    Apodization, DitherType, DspConfiguration, FilterPhase, FilterPreset, OverloadPolicy,
};

/// Balanced 1-bit DSD silence byte (`0x69` = `0b0110_1001`, 4 ones and 4 zeros, zero DC offset).
///
/// Per `Design.md §1.3` and `§4.5`, DSD silence is never `0x00` (which represents full-scale negative DC).
pub const DSD_IDLE_BYTE: u8 = 0x69;

/// Balanced 32-bit Native `DSD_U32_BE` silence word (`0x6969_6969`).
pub const DSD_IDLE_U32_BE: u32 = 0x6969_6969;

/// Specification of a 5th-order CRFB Delta-Sigma Modulator profile (`Design.md §4.3`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulatorProfileSpec {
    /// Modulator loop order (`5` for baseline 5th-order CRFB).
    pub order: u8,
    /// Target out-of-band NTF gain (`H_inf`, typically ~`1.5`).
    pub ntf_h_inf: f64,
    /// Verified maximum stable input amplitude `L` in dBFS (`Design.md §4.3–§4.4`).
    pub max_stable_input_dbfs: f64,
    /// Default safety margin `M` in dB (`Design.md §4.4`).
    pub default_safety_margin_db: f64,
    /// Baseline profile attenuation applied when `[gain] headroom = "profile"`.
    pub baseline_headroom_db: f64,
}

/// Static specification of one of the three built-in DSP presets (`Design.md §4.2`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresetDefinition {
    pub preset: FilterPreset,
    pub name: &'static str,
    pub description: &'static str,
    /// Maximum allowed passband peak-to-peak ripple in dB (`Design.md §4.2`).
    pub passband_ripple_max_db: f64,
    /// Minimum required stopband / image rejection in dB (`Design.md §4.2`).
    pub stopband_rejection_min_db: f64,
    /// Baseline 5th-order CRFB modulator parameters.
    pub modulator: ModulatorProfileSpec,
}

impl PresetDefinition {
    /// Materializes a concrete `DspConfiguration` from this preset and user overrides.
    pub const fn to_default_dsp_config(
        &self,
        phase: FilterPhase,
        apodization: Apodization,
        preamp_db: f64,
        user_volume_db: f64,
        overload_policy: OverloadPolicy,
    ) -> DspConfiguration {
        DspConfiguration {
            preset: self.preset,
            phase,
            apodization,
            modulator_order: self.modulator.order,
            preamp_db,
            user_volume_db,
            profile_headroom_db: self.modulator.baseline_headroom_db,
            max_stable_input_dbfs: self.modulator.max_stable_input_dbfs,
            safety_margin_db: self.modulator.default_safety_margin_db,
            dither: DitherType::Tpdf,
            dither_tpdf_seed: None,
            overload_policy,
        }
    }
}

/// Default real-time playback preset: `<= 0.01 dB` passband ripple, `>= 120 dB` stopband rejection (`Design.md §4.2`).
pub const PRESET_BALANCED: PresetDefinition = PresetDefinition {
    preset: FilterPreset::Balanced,
    name: "balanced",
    description: "Default real-time playback preset (>= 120 dB stopband rejection, <= 0.01 dB ripple)",
    passband_ripple_max_db: 0.01,
    stopband_rejection_min_db: 120.0,
    modulator: ModulatorProfileSpec {
        order: 5,
        ntf_h_inf: 1.50,
        max_stable_input_dbfs: -3.5,
        default_safety_margin_db: 0.5,
        baseline_headroom_db: -4.0,
    },
};

/// Offline conversion & high-margin live upsampling preset: `<= 0.001 dB` passband ripple, `>= 140 dB` stopband rejection (`Design.md §4.2`).
pub const PRESET_REFERENCE: PresetDefinition = PresetDefinition {
    preset: FilterPreset::Reference,
    name: "reference",
    description: "Reference archival & high-margin live preset (>= 140 dB stopband rejection, <= 0.001 dB ripple)",
    passband_ripple_max_db: 0.001,
    stopband_rejection_min_db: 140.0,
    modulator: ModulatorProfileSpec {
        order: 5,
        ntf_h_inf: 1.50,
        max_stable_input_dbfs: -3.5,
        default_safety_margin_db: 0.5,
        baseline_headroom_db: -4.0,
    },
};

/// Low group-delay interactive desktop audio preset: `>= 100 dB` stopband rejection (`Design.md §4.2`).
pub const PRESET_LOW_LATENCY: PresetDefinition = PresetDefinition {
    preset: FilterPreset::LowLatency,
    name: "low-latency",
    description: "Low group-delay interactive desktop audio preset (>= 100 dB stopband rejection)",
    passband_ripple_max_db: 0.02,
    stopband_rejection_min_db: 100.0,
    modulator: ModulatorProfileSpec {
        order: 5,
        ntf_h_inf: 1.45,
        max_stable_input_dbfs: -4.0,
        default_safety_margin_db: 0.5,
        baseline_headroom_db: -4.5,
    },
};

/// All built-in DSP presets in canonical order.
pub const ALL_PRESETS: [&PresetDefinition; 3] =
    [&PRESET_BALANCED, &PRESET_REFERENCE, &PRESET_LOW_LATENCY];

/// Returns the static `PresetDefinition` for a `FilterPreset`.
#[inline]
pub const fn by_preset(preset: FilterPreset) -> &'static PresetDefinition {
    match preset {
        FilterPreset::Balanced => &PRESET_BALANCED,
        FilterPreset::Reference => &PRESET_REFERENCE,
        FilterPreset::LowLatency => &PRESET_LOW_LATENCY,
    }
}

/// Resolves a preset name (`"balanced" | "reference" | "low-latency"`) to its static `PresetDefinition`.
#[inline]
pub fn by_name(name: &str) -> Option<&'static PresetDefinition> {
    FilterPreset::from_name(name).map(by_preset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preset_lookup_and_goals() {
        assert_eq!(by_name("balanced"), Some(&PRESET_BALANCED));
        assert_eq!(by_name("reference"), Some(&PRESET_REFERENCE));
        assert_eq!(by_name("low-latency"), Some(&PRESET_LOW_LATENCY));
        assert_eq!(by_name("unknown"), None);

        const {
            assert!(PRESET_BALANCED.stopband_rejection_min_db >= 120.0);
            assert!(PRESET_REFERENCE.stopband_rejection_min_db >= 140.0);
            assert!(PRESET_LOW_LATENCY.stopband_rejection_min_db >= 100.0);
        }
        assert_eq!(DSD_IDLE_BYTE.count_ones(), 4);
        assert_eq!(DSD_IDLE_U32_BE.count_ones(), 16);
    }
}
