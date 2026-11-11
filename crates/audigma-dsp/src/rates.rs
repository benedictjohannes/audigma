//! Strongly typed audio rate domains (`Design.md §1`, `§4.1`, `§5.1`).
//!
//! Raw `u32` integers must never cross module boundaries as rates.
//! Distinct domain types prevent confusing:
//! - PCM sample rates (`PcmRateHz`, e.g., `44_100`)
//! - 1-bit DSD bit rates (`DsdBitRateHz`, e.g., `11_289_600` for DSD256)
//! - Hardware container frame rates (`TransportFrameRateHz`, e.g., `176_400` for DoP64 or `88_200` for `DSD_U32`)
//! - PipeWire SPA DSD byte rates (`f_bit / 8`, e.g., `1_411_200` for DSD256)

use crate::config::TargetRateSpec;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Design audio baseband bandwidth in Hz (`B = 20 kHz`) used for Over-Sampling Ratio (`OSR`) calculations.
pub const DEFAULT_AUDIO_BASEBAND_HZ: f64 = 20_000.0;

/// Base clock family for PCM and DSD rates (`44.1 kHz` vs. `48 kHz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RateFamily {
    /// Multiples of `44,100 Hz` (`44.1k`, `88.2k`, `176.4k`, `352.8k`, `705.6k`, `DSD64=2.8224M`, etc.)
    #[serde(rename = "44100")]
    Hz44100,
    /// Multiples of `48,000 Hz` (`48k`, `96k`, `192k`, `384k`, `768k`, `DSD64/48=3.072M`, etc.)
    #[serde(rename = "48000")]
    Hz48000,
    /// Any rate not cleanly divisible by `44,100` or `48,000`.
    #[serde(rename = "other")]
    Other,
}

impl RateFamily {
    /// Returns the fundamental base PCM rate for this family (`44,100` or `48,000`), if standard.
    #[inline]
    pub const fn base_pcm_rate(self) -> Option<PcmRateHz> {
        match self {
            Self::Hz44100 => Some(PcmRateHz::PCM_44100),
            Self::Hz48000 => Some(PcmRateHz::PCM_48000),
            Self::Other => None,
        }
    }
}

/// Standard DSD rate multiplier relative to its base PCM rate (`44.1 kHz` or `48 kHz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u32)]
pub enum DsdMultiplier {
    /// `64x` (`2.8224 MHz` for 44.1k family, `3.072 MHz` for 48k family)
    Dsd64 = 64,
    /// `128x` (`5.6448 MHz` for 44.1k family, `6.144 MHz` for 48k family)
    Dsd128 = 128,
    /// `256x` (`11.2896 MHz` for 44.1k family, `12.288 MHz` for 48k family)
    Dsd256 = 256,
    /// `512x` (`22.5792 MHz` for 44.1k family, `24.576 MHz` for 48k family)
    Dsd512 = 512,
}

impl DsdMultiplier {
    /// Returns the numeric multiplier (`64`, `128`, `256`, or `512`).
    #[inline]
    pub const fn factor(self) -> u32 {
        self as u32
    }

    /// Resolves this multiplier to an explicit 1-bit `DsdBitRateHz` for the given clock family.
    #[inline]
    pub const fn to_bit_rate(self, family: RateFamily) -> Option<DsdBitRateHz> {
        DsdBitRateHz::from_family_and_multiplier(family, self)
    }
}

impl fmt::Display for DsdMultiplier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dsd64 => write!(f, "dsd64"),
            Self::Dsd128 => write!(f, "dsd128"),
            Self::Dsd256 => write!(f, "dsd256"),
            Self::Dsd512 => write!(f, "dsd512"),
        }
    }
}

/// Multi-bit PCM sample rate in Hz (samples per second per channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PcmRateHz(pub u32);

impl PcmRateHz {
    pub const PCM_44100: Self = Self(44_100);
    pub const PCM_48000: Self = Self(48_000);
    pub const PCM_88200: Self = Self(88_200);
    pub const PCM_96000: Self = Self(96_000);
    pub const PCM_176400: Self = Self(176_400);
    pub const PCM_192000: Self = Self(192_000);
    pub const PCM_352800: Self = Self(352_800);
    pub const PCM_384000: Self = Self(384_000);
    pub const PCM_705600: Self = Self(705_600);
    pub const PCM_768000: Self = Self(768_000);

    /// Constructs a `PcmRateHz`, returning `None` if `hz == 0`.
    #[inline]
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 { None } else { Some(Self(hz)) }
    }

    /// Returns the raw PCM sample rate in Hz.
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Determines whether this PCM rate belongs to the `44.1 kHz` or `48 kHz` clock family.
    #[inline]
    pub const fn rate_family(self) -> RateFamily {
        if self.0 > 0 && self.0 % 44_100 == 0 {
            RateFamily::Hz44100
        } else if self.0 > 0 && self.0 % 48_000 == 0 {
            RateFamily::Hz48000
        } else {
            RateFamily::Other
        }
    }

    /// If `target` is an exact power-of-two multiple (`2^K`) of `self`, returns `Some(K)`.
    #[inline]
    pub const fn power_of_two_stages_to(self, target: PcmRateHz) -> Option<u32> {
        if self.0 == 0 || target.0 < self.0 || target.0 % self.0 != 0 {
            return None;
        }
        let ratio = target.0 / self.0;
        if ratio.is_power_of_two() {
            Some(ratio.trailing_zeros())
        } else {
            None
        }
    }

    /// Returns the fundamental 1x base PCM rate for this rate's family (44,100 Hz or 48,000 Hz).
    #[inline]
    pub const fn base_family_rate(self) -> Option<PcmRateHz> {
        match self.rate_family() {
            RateFamily::Hz44100 => Some(PcmRateHz::PCM_44100),
            RateFamily::Hz48000 => Some(PcmRateHz::PCM_48000),
            RateFamily::Other => None,
        }
    }

    /// Returns the integer multiplier of this rate relative to its 1x base family rate (e.g. 4 for 192 kHz).
    #[inline]
    pub const fn rate_multiplier(self) -> Option<u32> {
        match self.base_family_rate() {
            Some(base) => Some(self.0 / base.0),
            None => None,
        }
    }

    /// Returns the equivalent rate in the target `RateFamily` at the same multiplier tier
    /// (e.g. 192 kHz in Hz48000 -> 176.4 kHz in Hz44100).
    #[inline]
    pub const fn corresponding_rate_in_family(
        self,
        target_family: RateFamily,
    ) -> Option<PcmRateHz> {
        let mult = match self.rate_multiplier() {
            Some(m) => m,
            None => return None,
        };
        match target_family {
            RateFamily::Hz44100 => PcmRateHz::new(44_100 * mult),
            RateFamily::Hz48000 => PcmRateHz::new(48_000 * mult),
            RateFamily::Other => None,
        }
    }
}

impl fmt::Display for PcmRateHz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} Hz (PCM)", self.0)
    }
}

/// Hardware transport frame rate in Hz (`DoP` 24/32-bit frames/s or ALSA `DSD_U32` 32-bit words/s).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransportFrameRateHz(pub u32);

impl TransportFrameRateHz {
    /// Constructs a `TransportFrameRateHz`, returning `None` if `hz == 0`.
    #[inline]
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 { None } else { Some(Self(hz)) }
    }

    /// Returns the raw frame rate in Hz.
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Converts a DoP transport frame rate into its underlying PCM carrier sample rate.
    #[inline]
    pub const fn as_pcm_carrier_rate(self) -> PcmRateHz {
        PcmRateHz(self.0)
    }
}

impl fmt::Display for TransportFrameRateHz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} frames/s", self.0)
    }
}

/// 1-bit Delta-Sigma Modulation (DSD) bit clock rate in Hz (`bits/s/channel`).
///
/// Enforces explicit conversion at transport boundaries (`Design.md §1`, `§5.1`):
/// - PipeWire SPA `audio/dsd` byte rate = `f_bit / 8` (`to_spa_dsd_byte_rate`)
/// - DoP v1.1 frame rate = `f_bit / 16` (`to_dop_frame_rate`)
/// - ALSA Native `DSD_U32` frame rate = `f_bit / 32` (`to_alsa_dsd_u32_frame_rate`)
/// - Baseband OSR = `f_bit / (2 * B)` (`baseband_osr`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DsdBitRateHz(pub u32);

impl DsdBitRateHz {
    // 44.1 kHz family DSD bit rates
    pub const DSD64: Self = Self(2_822_400);
    pub const DSD128: Self = Self(5_644_800);
    pub const DSD256: Self = Self(11_289_600);
    pub const DSD512: Self = Self(22_579_200);

    // 48 kHz family DSD bit rates (explicitly distinct per Design.md §1.1)
    pub const DSD64_48: Self = Self(3_072_000);
    pub const DSD128_48: Self = Self(6_144_000);
    pub const DSD256_48: Self = Self(12_288_000);
    pub const DSD512_48: Self = Self(24_576_000);

    /// Constructs a `DsdBitRateHz` if it is a valid non-zero multiple of 32 (required for `DSD_U32`/byte framing).
    #[inline]
    pub const fn new(hz: u32) -> Option<Self> {
        if hz == 0 || hz % 32 != 0 {
            None
        } else {
            Some(Self(hz))
        }
    }

    /// Constructs a `DsdBitRateHz` from a clock family and DSD multiplier (`64x`, `128x`, `256x`, `512x`).
    #[inline]
    pub const fn from_family_and_multiplier(
        family: RateFamily,
        multiplier: DsdMultiplier,
    ) -> Option<Self> {
        match family {
            RateFamily::Hz44100 => Some(Self(44_100 * multiplier.factor())),
            RateFamily::Hz48000 => Some(Self(48_000 * multiplier.factor())),
            RateFamily::Other => None,
        }
    }

    /// Returns the raw 1-bit clock frequency in Hz (`bits/s/channel`).
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Converts the DSD bit rate to PipeWire SPA `audio/dsd` byte rate per channel (`f_bit / 8`).
    ///
    /// Per `Design.md §1.1` & `§5.1`, PipeWire's SPA DSD format specifies rate in bytes/sec/channel
    /// (e.g., `1,411,200` for `DSD256`).
    #[inline]
    pub const fn to_spa_dsd_byte_rate(self) -> u32 {
        self.0 / 8
    }

    /// Converts the DSD bit rate to the DoP (DSD over PCM v1.1) frame rate (`f_bit / 16`).
    ///
    /// Each DoP frame carries 16 bits (2 bytes) of DSD data per channel (`176,400` for `DSD64`, `705,600` for `DSD256`).
    #[inline]
    pub const fn to_dop_frame_rate(self) -> TransportFrameRateHz {
        TransportFrameRateHz(self.0 / 16)
    }

    /// Converts the DSD bit rate to the ALSA Native `DSD_U32_BE`/`DSD_U32_LE` container frame rate (`f_bit / 32`).
    ///
    /// Each `DSD_U32` frame packs 32 consecutive 1-bit DSD samples per channel (`88,200` for `DSD64`, `352,800` for `DSD256`).
    #[inline]
    pub const fn to_alsa_dsd_u32_frame_rate(self) -> TransportFrameRateHz {
        TransportFrameRateHz(self.0 / 32)
    }

    /// Computes the true baseband Over-Sampling Ratio `OSR = f_bit / (2 * B)` (`Design.md §4.1`).
    ///
    /// Note that DSD's nominal multiplier (`64, 128, 256`) is relative to `44.1 kHz`, not `2 * B = 40 kHz`.
    /// For `DSD256` (`11.2896 MHz`) at `B = 20 kHz`, `OSR = 282.24`.
    #[inline]
    pub fn baseband_osr(self, baseband_hz: f64) -> f64 {
        assert!(baseband_hz > 0.0, "baseband_hz must be positive");
        (self.0 as f64) / (2.0 * baseband_hz)
    }

    /// Computes the baseband Over-Sampling Ratio at the standard 20 kHz audio baseband (`Design.md §4.1`).
    #[inline]
    pub fn default_baseband_osr(self) -> f64 {
        self.baseband_osr(DEFAULT_AUDIO_BASEBAND_HZ)
    }

    /// Returns the clock family (`44.1 kHz` vs `48 kHz`) of this DSD rate.
    #[inline]
    pub const fn rate_family(self) -> RateFamily {
        if self.0 > 0 && self.0 % (44_100 * 64) == 0 {
            RateFamily::Hz44100
        } else if self.0 > 0 && self.0 % (48_000 * 64) == 0 {
            RateFamily::Hz48000
        } else {
            RateFamily::Other
        }
    }

    /// Returns the `DsdMultiplier` (`Dsd64`, `Dsd128`, `Dsd256`, `Dsd512`) if this rate is a standard rate.
    #[inline]
    pub const fn dsd_multiplier(self) -> Option<DsdMultiplier> {
        let base = match self.rate_family() {
            RateFamily::Hz44100 => 44_100,
            RateFamily::Hz48000 => 48_000,
            RateFamily::Other => return None,
        };
        match self.0 / base {
            64 => Some(DsdMultiplier::Dsd64),
            128 => Some(DsdMultiplier::Dsd128),
            256 => Some(DsdMultiplier::Dsd256),
            512 => Some(DsdMultiplier::Dsd512),
            _ => None,
        }
    }

    /// If `target` PCM rate is an exact power-of-two decimation submultiple (`f_bit / 2^M`), returns `Some(M)`.
    #[inline]
    pub const fn power_of_two_decimation_stages_to(self, target: PcmRateHz) -> Option<u32> {
        if target.0 == 0 || self.0 < target.0 || self.0 % target.0 != 0 {
            return None;
        }
        let ratio = self.0 / target.0;
        if ratio.is_power_of_two() {
            Some(ratio.trailing_zeros())
        } else {
            None
        }
    }
}

/// Selects the optimal intermediate PCM rate for DSD decimation remodulation
/// according to the formula in `Design.md §1.6`:
/// `Rate_intermediate = min(Rate_in_useful, Rate_out_useful, max_intermediate_rate_hz)`.
pub fn select_dsd_intermediate_rate(
    in_rate: DsdBitRateHz,
    target_spec: Option<&TargetRateSpec>,
    max_intermediate_rate_hz: Option<PcmRateHz>,
) -> PcmRateHz {
    let in_family = in_rate.rate_family();
    let base_in_rate = in_family.base_pcm_rate().unwrap_or(PcmRateHz::PCM_44100);

    // in_useful: 176.4k/192k for DSD64, 352.8k/384k for DSD128+
    let in_useful = match in_rate.dsd_multiplier() {
        Some(DsdMultiplier::Dsd64) => base_in_rate.0 * 4, // 176.4k / 192k
        _ => base_in_rate.0 * 8,                          // 352.8k / 384k (DXD)
    };

    // out_useful: target rate if PCM, or corresponding DSD useful baseband
    let out_useful = match target_spec {
        Some(TargetRateSpec::Pcm { rate, .. }) => rate.get(),
        Some(TargetRateSpec::Dsd { bit_rate }) => {
            let out_base = bit_rate
                .rate_family()
                .base_pcm_rate()
                .unwrap_or(PcmRateHz::PCM_44100);
            match bit_rate.dsd_multiplier() {
                Some(DsdMultiplier::Dsd64) => out_base.0 * 4,
                _ => out_base.0 * 8,
            }
        }
        None => in_useful,
    };

    // max_intermediate: user limit or default 352.8k / 384k
    let default_max = base_in_rate.0 * 8;
    let max_intermediate = max_intermediate_rate_hz
        .map(|r| r.get())
        .unwrap_or(default_max);

    // Apply min formula: min(in_useful, out_useful, max_intermediate)
    let selected_hz = in_useful.min(out_useful).min(max_intermediate);

    // Candidate power-of-two rates in in_family: 4x (176.4k/192k) or 8x (352.8k/384k)
    let rate_4x = base_in_rate.0 * 4;
    let rate_8x = base_in_rate.0 * 8;

    let final_hz = if selected_hz <= rate_4x {
        rate_4x
    } else {
        rate_8x
    };

    PcmRateHz::new(final_hz).unwrap_or(PcmRateHz::PCM_352800)
}

impl DsdBitRateHz {
    /// Compact label without the bit rate: `DSD64`, `DSD256/48`, or `<hz> Hz (DSD)` when the
    /// rate is not a standard multiple.
    pub fn short_label(&self) -> String {
        match (self.rate_family(), self.dsd_multiplier()) {
            (RateFamily::Hz44100, Some(mult)) => format!("DSD{}", mult.factor()),
            (RateFamily::Hz48000, Some(mult)) => format!("DSD{}/48", mult.factor()),
            _ => format!("{} Hz (DSD)", self.0),
        }
    }
}

impl fmt::Display for DsdBitRateHz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.rate_family(), self.dsd_multiplier()) {
            (RateFamily::Hz44100, Some(mult)) => {
                write!(f, "DSD{} ({} Hz)", mult.factor(), self.0)
            }
            (RateFamily::Hz48000, Some(mult)) => {
                write!(f, "DSD{}/48 ({} Hz)", mult.factor(), self.0)
            }
            _ => write!(f, "{} Hz (DSD)", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_dsd_short_label() {
        assert_eq!(DsdBitRateHz::DSD64.short_label(), "DSD64");
        assert_eq!(
            DsdBitRateHz::new(11_289_600).unwrap().short_label(),
            "DSD256"
        );
        assert_eq!(
            DsdBitRateHz::new(12_288_000).unwrap().short_label(),
            "DSD256/48"
        );
    }

    use super::*;

    #[test]
    fn test_dsd_rate_relationships_table_44100_family() {
        // Verifies the exact table in Design.md §5.1
        struct Row {
            rate: DsdBitRateHz,
            bit_rate: u32,
            dop_frame_rate: u32,
            alsa_u32_frame_rate: u32,
            spa_byte_rate: u32,
            expected_osr_20k: f64,
        }

        let rows = [
            Row {
                rate: DsdBitRateHz::DSD64,
                bit_rate: 2_822_400,
                dop_frame_rate: 176_400,
                alsa_u32_frame_rate: 88_200,
                spa_byte_rate: 352_800,
                expected_osr_20k: 70.56,
            },
            Row {
                rate: DsdBitRateHz::DSD128,
                bit_rate: 5_644_800,
                dop_frame_rate: 352_800,
                alsa_u32_frame_rate: 176_400,
                spa_byte_rate: 705_600,
                expected_osr_20k: 141.12,
            },
            Row {
                rate: DsdBitRateHz::DSD256,
                bit_rate: 11_289_600,
                dop_frame_rate: 705_600,
                alsa_u32_frame_rate: 352_800,
                spa_byte_rate: 1_411_200,
                expected_osr_20k: 282.24,
            },
            Row {
                rate: DsdBitRateHz::DSD512,
                bit_rate: 22_579_200,
                dop_frame_rate: 1_411_200,
                alsa_u32_frame_rate: 705_600,
                spa_byte_rate: 2_822_400,
                expected_osr_20k: 564.48,
            },
        ];

        for row in rows {
            assert_eq!(row.rate.get(), row.bit_rate);
            assert_eq!(
                row.rate.to_dop_frame_rate(),
                TransportFrameRateHz(row.dop_frame_rate)
            );
            assert_eq!(
                row.rate.to_alsa_dsd_u32_frame_rate(),
                TransportFrameRateHz(row.alsa_u32_frame_rate)
            );
            assert_eq!(row.rate.to_spa_dsd_byte_rate(), row.spa_byte_rate);
            assert!((row.rate.default_baseband_osr() - row.expected_osr_20k).abs() < 1e-9);
            assert_eq!(row.rate.rate_family(), RateFamily::Hz44100);
        }
    }

    #[test]
    fn test_dsd_48khz_family_distinctness() {
        assert_eq!(DsdBitRateHz::DSD64_48.get(), 3_072_000);
        assert_eq!(DsdBitRateHz::DSD256_48.get(), 12_288_000);
        assert_eq!(DsdBitRateHz::DSD256_48.rate_family(), RateFamily::Hz48000);
        assert_eq!(
            DsdBitRateHz::DSD256_48.dsd_multiplier(),
            Some(DsdMultiplier::Dsd256)
        );
        assert_eq!(DsdBitRateHz::DSD256_48.to_spa_dsd_byte_rate(), 1_536_000);
        assert_eq!(
            DsdBitRateHz::DSD256_48.to_dop_frame_rate(),
            TransportFrameRateHz(768_000)
        );
        assert_eq!(
            DsdBitRateHz::DSD256_48.to_alsa_dsd_u32_frame_rate(),
            TransportFrameRateHz(384_000)
        );
        assert!((DsdBitRateHz::DSD256_48.default_baseband_osr() - 307.2).abs() < 1e-9);
    }

    #[test]
    fn test_pcm_rate_families_and_power_of_two() {
        assert_eq!(PcmRateHz::PCM_44100.rate_family(), RateFamily::Hz44100);
        assert_eq!(PcmRateHz::PCM_705600.rate_family(), RateFamily::Hz44100);
        assert_eq!(PcmRateHz::PCM_48000.rate_family(), RateFamily::Hz48000);
        assert_eq!(PcmRateHz::PCM_768000.rate_family(), RateFamily::Hz48000);

        assert_eq!(
            PcmRateHz::PCM_44100.power_of_two_stages_to(PcmRateHz::PCM_705600),
            Some(4) // 44.1k * 2^4 = 705.6k
        );
        assert_eq!(
            PcmRateHz::PCM_44100.power_of_two_stages_to(PcmRateHz::PCM_48000),
            None
        );

        // Test DsdBitRateHz decimation stages
        assert_eq!(
            DsdBitRateHz::DSD64.power_of_two_decimation_stages_to(PcmRateHz::PCM_176400),
            Some(4) // 2.8224M / 176.4k = 16 = 2^4
        );
        assert_eq!(
            DsdBitRateHz::DSD256.power_of_two_decimation_stages_to(PcmRateHz::PCM_352800),
            Some(5) // 11.2896M / 352.8k = 32 = 2^5
        );
        assert_eq!(
            DsdBitRateHz::DSD256.power_of_two_decimation_stages_to(PcmRateHz::PCM_176400),
            Some(6) // 11.2896M / 176.4k = 64 = 2^6
        );
        assert_eq!(
            DsdBitRateHz::DSD256.power_of_two_decimation_stages_to(PcmRateHz::PCM_48000),
            None
        );

        // Test intermediate rate selection formula
        let dsd256_spec = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256,
        };
        let pcm96_spec = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_96000,
            sample_format: crate::config::SampleFormat::F32,
        };

        // DSD64 -> DSD256: in_useful is 176.4k -> selects 176.4k
        assert_eq!(
            select_dsd_intermediate_rate(DsdBitRateHz::DSD64, Some(&dsd256_spec), None),
            PcmRateHz::PCM_176400
        );
        // DSD256 -> DSD256: in_useful=352.8k, out_useful=352.8k -> selects 352.8k
        assert_eq!(
            select_dsd_intermediate_rate(DsdBitRateHz::DSD256, Some(&dsd256_spec), None),
            PcmRateHz::PCM_352800
        );
        // DSD256 -> PCM 96k: out_useful=96k <= 176.4k -> selects 176.4k
        assert_eq!(
            select_dsd_intermediate_rate(DsdBitRateHz::DSD256, Some(&pcm96_spec), None),
            PcmRateHz::PCM_176400
        );
        // DSD256 with max_intermediate=176.4k -> selects 176.4k
        assert_eq!(
            select_dsd_intermediate_rate(
                DsdBitRateHz::DSD256,
                Some(&dsd256_spec),
                Some(PcmRateHz::PCM_176400)
            ),
            PcmRateHz::PCM_176400
        );
    }
}
