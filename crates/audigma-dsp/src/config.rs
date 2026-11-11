//! DSP configuration contracts, stream specifications, and headroom math (`Design.md §3.4`, `§4.2–§4.4`).

use crate::rates::{DsdBitRateHz, PcmRateHz};
use serde::{Deserialize, Serialize};

/// Sample encoding format for PCM and packed DSD containers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetFormat {
    Pcm,
    Dsd,
}

/// FIR impulse response phase characteristic (`Design.md §4.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterPhase {
    Linear,
    Minimum,
}

/// Stage-0 apodization (`Design.md §4.2`): `Off` uses the brick-wall half-band (pass 20 kHz, stop 24.1 kHz);
/// a pass edge selects an apodizing low-pass from that edge to a 22.05 kHz stopband (the source Nyquist).
///
/// Edges are for 44.1 kHz-family sources; the 48 kHz family runs the same tables, so they scale by 48/44.1.
/// TOML: `"off"`, `18000`, `18500`, `19000`, `20000`, or `"gentle"` (alias for `19000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Apodization {
    #[default]
    Off,
    PassEdge18000,
    PassEdge18500,
    PassEdge19000,
    PassEdge20000,
}

impl Apodization {
    /// Every apodizing pass edge in Hz, in ascending order.
    pub const PASS_EDGES_HZ: [u32; 4] = [18_000, 18_500, 19_000, 20_000];

    /// Every setting, `Off` first.
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::PassEdge18000,
        Self::PassEdge18500,
        Self::PassEdge19000,
        Self::PassEdge20000,
    ];

    /// The pass edge in Hz (44.1 kHz family), or `None` for `Off`.
    pub const fn pass_edge_hz(self) -> Option<u32> {
        match self {
            Self::Off => None,
            Self::PassEdge18000 => Some(18_000),
            Self::PassEdge18500 => Some(18_500),
            Self::PassEdge19000 => Some(19_000),
            Self::PassEdge20000 => Some(20_000),
        }
    }

    /// Resolves a pass edge in Hz; only the four supported edges are accepted.
    pub const fn from_pass_edge_hz(hz: u32) -> Option<Self> {
        match hz {
            18_000 => Some(Self::PassEdge18000),
            18_500 => Some(Self::PassEdge18500),
            19_000 => Some(Self::PassEdge19000),
            20_000 => Some(Self::PassEdge20000),
            _ => None,
        }
    }

    /// Resolves `"off"`, `"gentle"` (alias for 19000) or a supported pass edge written as a number.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "gentle" => Some(Self::PassEdge19000),
            other => other.parse().ok().and_then(Self::from_pass_edge_hz),
        }
    }

    /// Canonical name: `"off"` or the pass edge in Hz.
    pub fn name(self) -> String {
        self.pass_edge_hz()
            .map_or_else(|| "off".to_string(), |hz| hz.to_string())
    }
}

impl std::fmt::Display for Apodization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name())
    }
}

/// Accepted TOML values (`"gentle"` is an input alias for `19000`); mirrors the `Deserialize` impl.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Apodization {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Apodization".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "enum": ["off", "gentle", 18000, 18500, 19000, 20000] })
    }
}

impl Serialize for Apodization {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.pass_edge_hz() {
            None => serializer.serialize_str("off"),
            Some(hz) => serializer.serialize_u32(hz),
        }
    }
}

impl<'de> Deserialize<'de> for Apodization {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ApodizationVisitor;

        const EXPECTED: &str = r#""off", "gentle", 18000, 18500, 19000 or 20000"#;

        impl serde::de::Visitor<'_> for ApodizationVisitor {
            type Value = Apodization;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(EXPECTED)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Apodization, E> {
                match v {
                    "off" => Ok(Apodization::Off),
                    "gentle" => Ok(Apodization::PassEdge19000),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(v), &EXPECTED)),
                }
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Apodization, E> {
                u32::try_from(v)
                    .ok()
                    .and_then(Apodization::from_pass_edge_hz)
                    .ok_or_else(|| E::invalid_value(serde::de::Unexpected::Unsigned(v), &EXPECTED))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Apodization, E> {
                u64::try_from(v)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(v), &EXPECTED))
                    .and_then(|v| self.visit_u64(v))
            }
        }

        deserializer.deserialize_any(ApodizationVisitor)
    }
}

/// Dither topology for integer PCM quantization and 1-bit DSM (`Design.md §4.2–§4.3`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DitherType {
    None,
    Tpdf,
}

/// Modulator overload handling policy (`Design.md §4.4`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// A configuration with the given filter and gain settings and defaults for the rest.
    ///
    /// It has no modulator limits (PCM: full scale, no headroom); a DSD target applies them with
    /// `apply_modulator_limits` once its order and bit rate are known.
    pub const fn new(
        phase: FilterPhase,
        apodization: Apodization,
        preamp_db: f64,
        user_volume_db: f64,
        overload_policy: OverloadPolicy,
    ) -> Self {
        Self {
            phase,
            apodization,
            modulator_order: crate::defaults::DEFAULT_MODULATOR_ORDER,
            preamp_db,
            user_volume_db,
            profile_headroom_db: 0.0,
            max_stable_input_dbfs: 0.0,
            safety_margin_db: 0.0,
            dither: DitherType::Tpdf,
            dither_tpdf_seed: None,
            overload_policy,
        }
    }

    /// Sets the input limit `L` of the selected modulator (`modulator_order` at `bit_rate`) and the matching
    /// profile headroom `L - M` (`M` = `safety_margin_db`, DSD only; PCM keeps `M = 0`), so a 0 dBFS PCM peak reaches the modulator at `L - M` (`Design.md §4.3-§4.4`).
    pub fn apply_modulator_limits(
        &mut self,
        bit_rate: DsdBitRateHz,
        safety_margin_db: f64,
    ) -> Result<(), crate::modulator::ModulatorError> {
        let limit = crate::modulator::max_stable_input_dbfs(self.modulator_order, bit_rate)?;
        self.max_stable_input_dbfs = limit;
        self.safety_margin_db = safety_margin_db;
        self.profile_headroom_db = limit - safety_margin_db;
        Ok(())
    }

    /// Human-readable filter summary for logs and telemetry, e.g. `"linear, apodization off"`
    /// or `"minimum, apodization 19000 Hz"`.
    pub fn filter_description(&self) -> String {
        let phase = match self.phase {
            FilterPhase::Linear => "linear",
            FilterPhase::Minimum => "minimum",
        };
        match self.apodization.pass_edge_hz() {
            None => format!("{phase}, apodization off"),
            Some(hz) => format!("{phase}, apodization {hz} Hz"),
        }
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

    #[test]
    fn test_headroom_inequality_and_two_pass_gain() {
        let mut cfg = DspConfiguration::new(
            FilterPhase::Linear,
            Apodization::Off,
            0.0,
            0.0,
            OverloadPolicy::Error,
        );
        // Without a modulator (PCM) there is no headroom and the limit is full scale.
        assert_eq!(cfg.total_configured_gain_db(), 0.0);
        assert_eq!(cfg.max_stable_input_dbfs, 0.0);
        assert_eq!(cfg.safety_margin_db, 0.0);

        cfg.apply_modulator_limits(DsdBitRateHz::DSD64, 0.25)
            .unwrap();
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
        let mut cfg = DspConfiguration::new(
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
                cfg.apply_modulator_limits(rate, 0.25).unwrap();
                let l = crate::modulator::max_stable_input_dbfs(order, rate).unwrap();
                assert_eq!(cfg.max_stable_input_dbfs, l);
                assert!(l < -3.0 && l > -6.0, "implausible limit {l} dBFS");
            }
        }
        cfg.modulator_order = 6;
        assert!(
            cfg.apply_modulator_limits(DsdBitRateHz::DSD64, 0.25)
                .is_err()
        );
    }

    #[test]
    fn test_apodization_names_and_serde() {
        #[derive(Deserialize, Serialize)]
        struct Wrap {
            apodization: Apodization,
        }
        let parse =
            |v: &str| toml::from_str::<Wrap>(&format!("apodization = {v}")).map(|w| w.apodization);
        assert_eq!(parse(r#""off""#).unwrap(), Apodization::Off);
        assert_eq!(parse(r#""gentle""#).unwrap(), Apodization::PassEdge19000);
        assert_eq!(parse("18000").unwrap(), Apodization::PassEdge18000);
        assert_eq!(parse("18500").unwrap(), Apodization::PassEdge18500);
        assert_eq!(parse("20000").unwrap(), Apodization::PassEdge20000);
        for bad in ["17500", "19500", "-1", r#""19000""#, r#""on""#, "19000.0"] {
            assert!(parse(bad).is_err(), "{bad} must be rejected");
        }
        for a in Apodization::ALL {
            let text = toml::to_string(&Wrap { apodization: a }).unwrap();
            assert_eq!(toml::from_str::<Wrap>(&text).unwrap().apodization, a);
            assert_eq!(Apodization::from_name(&a.name()), Some(a));
        }
        assert_eq!(
            Apodization::from_name("gentle"),
            Some(Apodization::PassEdge19000)
        );
        assert_eq!(Apodization::from_name("17500"), None);
    }

    #[test]
    fn test_volume_unity_and_linear_gain_computation() {
        let mut cfg = DspConfiguration::new(
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
