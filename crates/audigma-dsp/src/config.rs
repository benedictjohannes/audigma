//! DSP configuration contracts, stream specifications, and headroom math (`Design.md §3.4`, `§4.2–§4.4`).

use crate::rates::{DsdBitRateHz, PcmRateHz};
use serde::{Deserialize, Serialize};

/// Sample encoding format for PCM and packed DSD containers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SampleFormat {
    #[serde(rename = "s16le")]
    S16Le,
    #[serde(rename = "s24le")]
    S24Le,
    #[serde(rename = "s24_3le")]
    S24_3Le,
    #[serde(rename = "s32le")]
    S32Le,
    #[serde(rename = "f32")]
    F32,
    #[serde(rename = "f64")]
    F64,
    #[serde(rename = "dsd_u32_be")]
    DsdU32Be,
    #[serde(rename = "dsd_u32_le")]
    DsdU32Le,
}

/// Target signal domain (`pcm` vs `dsd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetFormat {
    Pcm,
    Dsd,
}

/// Canonical filter & modulator quality preset (`Design.md §4.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FilterPreset {
    #[default]
    #[serde(rename = "balanced")]
    Balanced,
    #[serde(rename = "reference")]
    Reference,
    #[serde(rename = "low-latency")]
    LowLatency,
}

impl FilterPreset {
    /// Strictly resolves one of the three canonical preset names (`"balanced" | "reference" | "low-latency"`).
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "balanced" => Some(Self::Balanced),
            "reference" => Some(Self::Reference),
            "low-latency" => Some(Self::LowLatency),
            _ => None,
        }
    }

    /// Returns the canonical TOML/CLI preset string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Reference => "reference",
            Self::LowLatency => "low-latency",
        }
    }
}

/// FIR impulse response phase characteristic (`Design.md §4.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterPhase {
    Linear,
    Minimum,
}

/// Pre-ringing / post-Nyquist steepness apodization control (`Design.md §4.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Apodization {
    Off,
    Gentle,
}

/// Dither topology for integer PCM quantization and 1-bit DSM (`Design.md §4.2–§4.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DitherType {
    None,
    Tpdf,
}

/// Modulator overload handling policy (`Design.md §4.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum OverloadPolicy {
    #[default]
    #[serde(rename = "mute-recover")]
    MuteRecover,
    #[serde(rename = "error")]
    Error,
}

/// Resolved target rate for a prepared DSP pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetRateSpec {
    Pcm {
        rate: PcmRateHz,
        sample_format: SampleFormat,
    },
    Dsd {
        bit_rate: DsdBitRateHz,
    },
}

/// Input/Output stream specification passed to `prepare(spec, config)` (`Design.md §3.4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamSpec {
    pub input_rate: PcmRateHz,
    pub channels: u8,
    pub target: TargetRateSpec,
}

/// Concrete, resolved DSP pipeline configuration (`Design.md §2`, `§4.2–§4.4`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DspConfiguration {
    pub preset: FilterPreset,
    pub phase: FilterPhase,
    pub apodization: Apodization,
    pub modulator_order: u8,
    pub preamp_db: f64,
    pub user_volume_db: f64,
    pub profile_headroom_db: f64,
    pub max_stable_input_dbfs: f64,
    pub safety_margin_db: f64,
    pub dither: DitherType,
    pub dither_tpdf_seed: Option<u64>,
    pub overload_policy: OverloadPolicy,
}

impl DspConfiguration {
    /// Sets the input limit `L` of the selected modulator (`modulator_order` at `bit_rate`) and the matching
    /// profile headroom `L - M`, so a 0 dBFS PCM peak reaches the modulator at `L - M` (`Design.md §4.3-§4.4`).
    pub fn apply_modulator_limits(
        &mut self,
        bit_rate: DsdBitRateHz,
    ) -> Result<(), crate::modulator::ModulatorError> {
        let limit = crate::modulator::max_stable_input_dbfs(self.modulator_order, bit_rate)?;
        self.max_stable_input_dbfs = limit;
        self.profile_headroom_db = limit - self.safety_margin_db;
        Ok(())
    }

    /// Returns the total configured static gain in dB (`preamp_db + user_volume_db + profile_headroom_db`).
    #[inline]
    pub fn total_configured_gain_db(&self) -> f64 {
        self.preamp_db + self.user_volume_db + self.profile_headroom_db
    }

    /// Computes the maximum allowable pre-modulator gain in dB for a given post-interpolation peak `P` (in dBFS):
    /// `max_gain_db = L - M - P` (`Design.md §4.4`).
    #[inline]
    pub fn max_safe_gain_db(&self, measured_peak_dbfs: f64) -> f64 {
        self.max_stable_input_dbfs - self.safety_margin_db - measured_peak_dbfs
    }

    /// Solves the two-pass offline gain equation (`Design.md §4.4`):
    /// `gain_db = min(configured_gain_db, L - M - P)`.
    #[inline]
    pub fn resolve_two_pass_gain_db(&self, measured_peak_dbfs: f64) -> f64 {
        self.total_configured_gain_db()
            .min(self.max_safe_gain_db(measured_peak_dbfs))
    }

    /// Converts a dB gain value to a linear `f64` multiplier (`10^(db / 20)`).
    #[inline]
    pub fn db_to_linear(db: f64) -> f64 {
        10.0_f64.powf(db / 20.0)
    }

    /// Returns `true` if software volume attenuation and preamp are at unity (0.0 dBFS within 1e-6 tolerance).
    ///
    /// When unity gain is satisfied and stream rates match, opportunistic bit-exact
    /// DSD passthrough is eligible (`Design.md §1.6`).
    #[inline]
    pub fn is_volume_unity(&self) -> bool {
        self.user_volume_db.abs() < 1e-6 && self.preamp_db.abs() < 1e-6
    }

    /// Computes the effective linear multiplier from a total configured gain in dB.
    ///
    /// If `is_muted` is true or `total_gain_db` is at or below the digital silence threshold
    /// (-119.0 dB), returns 0.0 (complete silence). Otherwise, returns `10^(total_gain_db / 20)`.
    #[inline]
    pub fn compute_linear_gain(total_gain_db: f64, is_muted: bool) -> f64 {
        if is_muted || total_gain_db <= -119.0 {
            0.0
        } else {
            Self::db_to_linear(total_gain_db)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::PRESET_REFERENCE;

    #[test]
    fn test_headroom_inequality_and_two_pass_gain() {
        let mut cfg = PRESET_REFERENCE.to_default_dsp_config(
            FilterPhase::Linear,
            Apodization::Off,
            0.0,
            0.0,
            OverloadPolicy::Error,
        );
        // Without a modulator (PCM) there is no headroom and the limit is full scale.
        assert_eq!(cfg.total_configured_gain_db(), 0.0);
        assert_eq!(cfg.max_stable_input_dbfs, 0.0);

        cfg.apply_modulator_limits(DsdBitRateHz::DSD64).unwrap();
        let l = crate::modulator::max_stable_input_dbfs(5, DsdBitRateHz::DSD64).unwrap();
        let m = cfg.safety_margin_db;
        assert_eq!(cfg.max_stable_input_dbfs, l);
        assert!((cfg.total_configured_gain_db() - (l - m)).abs() < 1e-12);

        // Pass 1 measures an inter-sample true peak of +1.2 dBFS: gain = L - M - P.
        let two_pass_gain = cfg.resolve_two_pass_gain_db(1.2);
        assert!((two_pass_gain - (l - m - 1.2)).abs() < 1e-12);

        // A quiet peak of -1.0 dBFS would allow more than the configured headroom, which is kept.
        let quiet_gain = cfg.resolve_two_pass_gain_db(-1.0);
        assert!((quiet_gain - (l - m)).abs() < 1e-12);
    }

    #[test]
    fn test_modulator_limits_follow_order_and_rate() {
        let mut cfg = PRESET_REFERENCE.to_default_dsp_config(
            FilterPhase::Linear,
            Apodization::Off,
            0.0,
            0.0,
            OverloadPolicy::Error,
        );
        for rate in [
            DsdBitRateHz::DSD64,
            DsdBitRateHz::DSD512,
            DsdBitRateHz::DSD256_48,
        ] {
            for order in [5u8, 7] {
                cfg.modulator_order = order;
                cfg.apply_modulator_limits(rate).unwrap();
                let l = crate::modulator::max_stable_input_dbfs(order, rate).unwrap();
                assert_eq!(cfg.max_stable_input_dbfs, l);
                assert!(l < -3.0 && l > -6.0, "implausible limit {l} dBFS");
            }
        }
        cfg.modulator_order = 6;
        assert!(cfg.apply_modulator_limits(DsdBitRateHz::DSD64).is_err());
    }

    #[test]
    fn test_volume_unity_and_linear_gain_computation() {
        let mut cfg = PRESET_REFERENCE.to_default_dsp_config(
            FilterPhase::Linear,
            Apodization::Off,
            0.0,
            0.0,
            OverloadPolicy::Error,
        );
        assert!(cfg.is_volume_unity());

        cfg.user_volume_db = -0.5;
        assert!(!cfg.is_volume_unity());

        cfg.user_volume_db = 0.0;
        cfg.preamp_db = -1.0;
        assert!(!cfg.is_volume_unity());

        cfg.preamp_db = 0.0;
        assert!(cfg.is_volume_unity());

        // compute_linear_gain tests
        assert_eq!(DspConfiguration::compute_linear_gain(0.0, false), 1.0);
        assert_eq!(DspConfiguration::compute_linear_gain(0.0, true), 0.0);
        assert_eq!(DspConfiguration::compute_linear_gain(-119.0, false), 0.0);
        assert_eq!(DspConfiguration::compute_linear_gain(-120.0, false), 0.0);
        assert!(
            (DspConfiguration::compute_linear_gain(-6.0, false) - 0.5011872336272722).abs() < 1e-9
        );
    }
}
