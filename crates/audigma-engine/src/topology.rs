//! Pipeline topology resolution, rate planning, and passthrough eligibility (`Design.md §1.3`, `§1.6`).

use audigma_dsp::config::{DspConfiguration, TargetRateSpec};
use audigma_dsp::rates::PcmRateHz;

use crate::traits::EngineError;
use crate::worker::TargetDsdConfig;

/// Concrete rate planning and filter cascade topology resolved from input rate and target specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineTopology {
    /// Input PCM rate (or decimated intermediate PCM rate if input was DSD).
    pub input_rate: PcmRateHz,
    /// Intermediate baseband rate after any rational SRC and before the power-of-two half-band cascade.
    pub intermediate_rate: PcmRateHz,
    /// Number of 2x power-of-two half-band FIR upsampling stages.
    pub cascade_stages: usize,
    /// Number of 2x power-of-two half-band FIR decimation stages.
    pub decimation_stages: usize,
    /// Effective rate multiplier of the half-band cascade (`1 << cascade_stages`).
    pub rate_multiplier: usize,
    /// Whether polyphase rational SRC is required between input rate and intermediate rate.
    pub requires_rational_src: bool,
    /// Resolved target output rate specification.
    pub target_spec: TargetRateSpec,
}

impl PipelineTopology {
    /// Resolves intermediate baseband rate and power-of-two half-band stages.
    pub fn resolve(
        input_rate: PcmRateHz,
        target_spec: TargetRateSpec,
    ) -> Result<Self, EngineError> {
        let (intermediate_rate, stages, dec_stages) = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                let dsd_family = bit_rate.rate_family();
                let in_family = input_rate.rate_family();
                if in_family == dsd_family {
                    if bit_rate.get() >= input_rate.get() {
                        let mult = bit_rate.get() / input_rate.get();
                        let stages = (mult as f64).log2().round() as usize;
                        (input_rate, stages, 0)
                    } else {
                        return Err(EngineError::InvalidTopology {
                            message: format!(
                                "input rate {input_rate} exceeds DSD bit rate {bit_rate}"
                            ),
                        });
                    }
                } else {
                    // Cross-family conversion (e.g. 192k 48-family -> 44.1-family DSD):
                    // Resample at matched tier (192k -> 176.4k) to preserve ultrasonic content,
                    // then upsample via power-of-two half-band FIR stages to target DSD bit rate.
                    let matched_tier_rate = input_rate
                        .corresponding_rate_in_family(dsd_family)
                        .filter(|r| bit_rate.get() >= r.get())
                        .or_else(|| dsd_family.base_pcm_rate())
                        .ok_or_else(|| EngineError::InvalidTopology {
                            message: format!("invalid rate family for DSD bit rate {bit_rate}"),
                        })?;
                    let mult = bit_rate.get() / matched_tier_rate.get();
                    let stages = (mult as f64).log2().round() as usize;
                    (matched_tier_rate, stages, 0)
                }
            }
            TargetRateSpec::Pcm { rate, .. } => {
                let target_family = rate.rate_family();
                let in_family = input_rate.rate_family();
                if in_family == target_family {
                    if rate.get() >= input_rate.get() {
                        let mult = rate.get() / input_rate.get();
                        let stages = if mult >= 1 {
                            (mult as f64).log2().round() as usize
                        } else {
                            0
                        };
                        (input_rate, stages, 0)
                    } else {
                        let div = input_rate.get() / rate.get();
                        if div > 0 && div.is_power_of_two() {
                            let dec = div.trailing_zeros() as usize;
                            (rate, 0, dec)
                        } else {
                            return Err(EngineError::InvalidTopology {
                                message: format!(
                                    "cannot downsample {input_rate} to {rate}: ratio {div} is not a power of two"
                                ),
                            });
                        }
                    }
                } else {
                    // Cross-family PCM conversion:
                    // If target rate allows, resample at matched tier first, then power-of-two stages
                    let matched_tier_rate = input_rate
                        .corresponding_rate_in_family(target_family)
                        .filter(|r| rate.get() >= r.get())
                        .or_else(|| target_family.base_pcm_rate())
                        .ok_or_else(|| EngineError::InvalidTopology {
                            message: format!("invalid target rate family for {rate}"),
                        })?;
                    let mult = rate.get() / matched_tier_rate.get();
                    let stages = if mult >= 1 {
                        (mult as f64).log2().round() as usize
                    } else {
                        0
                    };
                    (matched_tier_rate, stages, 0)
                }
            }
        };

        let rate_multiplier = 1usize << stages;
        let requires_rational_src = input_rate != intermediate_rate && dec_stages == 0;

        Ok(Self {
            input_rate,
            intermediate_rate,
            cascade_stages: stages,
            decimation_stages: dec_stages,
            rate_multiplier,
            requires_rational_src,
            target_spec,
        })
    }

    /// Returns the effective decimation divider (`1 << decimation_stages`).
    #[inline]
    pub const fn decimation_divider(&self) -> usize {
        1 << self.decimation_stages
    }

    /// Returns `true` if the target is a 1-bit DSD stream.
    #[inline]
    pub fn is_dsd(&self) -> bool {
        matches!(self.target_spec, TargetRateSpec::Dsd { .. })
    }

    /// Resolves optional modulator target configuration for `ChannelWorkerPool`.
    pub fn target_dsd_config(&self, dsp_config: &DspConfiguration) -> Option<TargetDsdConfig> {
        match self.target_spec {
            TargetRateSpec::Dsd { bit_rate } => Some(TargetDsdConfig {
                bit_rate,
                dither: dsp_config.dither,
                overload_policy: dsp_config.overload_policy,
                seed_base: dsp_config.dither_tpdf_seed.unwrap_or(12345),
            }),
            TargetRateSpec::Pcm { .. } => None,
        }
    }
}

pub use crate::transport::is_dsd_passthrough_eligible;

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_dsp::config::{Apodization, FilterPhase, OverloadPolicy, SampleFormat};
    use audigma_dsp::presets::PRESET_REFERENCE;
    use audigma_dsp::rates::{DsdBitRateHz, PcmRateHz};
    use audigma_format::decoder::AudioStreamInfo;

    #[test]
    fn test_topology_same_family_dsd() {
        let in_rate = PcmRateHz::PCM_44100;
        let target = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256, // 11.2896 MHz = 256x 44.1k
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, in_rate);
        assert_eq!(topo.cascade_stages, 8); // 2^8 = 256
        assert_eq!(topo.rate_multiplier, 256);
        assert!(!topo.requires_rational_src);
        assert!(topo.is_dsd());
    }

    #[test]
    fn test_topology_cross_family_dsd_matched_tier() {
        // 192k (48-family) -> DSD256 (44.1-family)
        let in_rate = PcmRateHz::PCM_192000;
        let target = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256, // 11.2896 MHz
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, PcmRateHz::PCM_176400); // matched tier
        assert_eq!(topo.cascade_stages, 6); // 11.2896M / 176.4k = 64 = 2^6
        assert_eq!(topo.rate_multiplier, 64);
        assert!(topo.requires_rational_src);
    }

    #[test]
    fn test_topology_same_family_pcm() {
        let in_rate = PcmRateHz::PCM_44100;
        let target = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_352800, // 8x
            sample_format: SampleFormat::S32Le,
            valid_bits: None,
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, in_rate);
        assert_eq!(topo.cascade_stages, 3); // 2^3 = 8
        assert_eq!(topo.decimation_stages, 0);
        assert_eq!(topo.rate_multiplier, 8);
        assert_eq!(topo.decimation_divider(), 1);
        assert!(!topo.requires_rational_src);
        assert!(!topo.is_dsd());
    }

    #[test]
    fn test_topology_same_family_pcm_downsample_176k_to_44k() {
        let in_rate = PcmRateHz::PCM_176400;
        let target = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_44100,
            sample_format: SampleFormat::S32Le,
            valid_bits: None,
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, PcmRateHz::PCM_44100);
        assert_eq!(topo.cascade_stages, 0);
        assert_eq!(topo.decimation_stages, 2); // 176.4k / 44.1k = 4 = 2^2
        assert_eq!(topo.rate_multiplier, 1);
        assert_eq!(topo.decimation_divider(), 4);
        assert!(!topo.requires_rational_src);
        assert!(!topo.is_dsd());
    }

    #[test]
    fn test_topology_same_family_pcm_downsample_192k_to_48k() {
        let in_rate = PcmRateHz::PCM_192000;
        let target = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_48000,
            sample_format: SampleFormat::S32Le,
            valid_bits: None,
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, PcmRateHz::PCM_48000);
        assert_eq!(topo.cascade_stages, 0);
        assert_eq!(topo.decimation_stages, 2); // 192k / 48k = 4 = 2^2
        assert_eq!(topo.rate_multiplier, 1);
        assert_eq!(topo.decimation_divider(), 4);
        assert!(!topo.requires_rational_src);
    }

    #[test]
    fn test_topology_same_family_pcm_downsample_96k_to_48k() {
        let in_rate = PcmRateHz::PCM_96000;
        let target = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_48000,
            sample_format: SampleFormat::S32Le,
            valid_bits: None,
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, PcmRateHz::PCM_48000);
        assert_eq!(topo.cascade_stages, 0);
        assert_eq!(topo.decimation_stages, 1); // 96k / 48k = 2 = 2^1
        assert_eq!(topo.rate_multiplier, 1);
        assert_eq!(topo.decimation_divider(), 2);
        assert!(!topo.requires_rational_src);
    }

    #[test]
    fn test_topology_cross_family_pcm_downsample_176k_to_48k() {
        let in_rate = PcmRateHz::PCM_176400;
        let target = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_48000,
            sample_format: SampleFormat::S32Le,
            valid_bits: None,
        };
        let topo = PipelineTopology::resolve(in_rate, target).unwrap();
        assert_eq!(topo.intermediate_rate, PcmRateHz::PCM_48000);
        assert_eq!(topo.cascade_stages, 0);
        assert_eq!(topo.decimation_stages, 0);
        assert_eq!(topo.rate_multiplier, 1);
        assert!(topo.requires_rational_src);
    }

    #[test]
    fn test_topology_rejects_input_rate_exceeding_dsd_bit_rate() {
        // High PCM rate in 44.1k family exceeding DSD64 bit rate
        let in_rate = PcmRateHz::new(5_644_800).unwrap();
        let target = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD64, // 2.8224 MHz
        };
        let err = PipelineTopology::resolve(in_rate, target).unwrap_err();
        assert!(err.to_string().contains("exceeds DSD bit rate"));
    }

    #[test]
    fn test_dsd_passthrough_eligibility() {
        let bit_rate = DsdBitRateHz::DSD64;
        let target = TargetRateSpec::Dsd { bit_rate };
        let dsd_info = AudioStreamInfo::new_dsd(bit_rate, 2, None);
        let pcm_info = AudioStreamInfo::new_pcm(PcmRateHz::PCM_44100, 2, None, Some(16));

        let mut dsp_cfg = PRESET_REFERENCE.to_default_dsp_config(
            FilterPhase::Linear,
            Apodization::Off,
            0.0,
            0.0,
            OverloadPolicy::Error,
        );

        // Matching DSD at unity volume
        assert!(is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));

        // PCM input is not eligible
        assert!(!is_dsd_passthrough_eligible(&pcm_info, &target, &dsp_cfg));

        // Non-unity user volume disables passthrough
        dsp_cfg.user_volume_db = -3.0;
        assert!(!is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));

        // Non-unity preamp disables passthrough
        dsp_cfg.user_volume_db = 0.0;
        dsp_cfg.preamp_db = -1.0;
        assert!(!is_dsd_passthrough_eligible(&dsd_info, &target, &dsp_cfg));

        // Rate mismatch disables passthrough
        dsp_cfg.preamp_db = 0.0;
        let other_target = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD128,
        };
        assert!(!is_dsd_passthrough_eligible(
            &dsd_info,
            &other_target,
            &dsp_cfg
        ));
    }

    #[test]
    fn test_topology_plan_rate_error_returns_invalid_topology() {
        let high_pcm_rate = PcmRateHz::new(5_644_800).unwrap();
        let target = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD64,
        };
        let err = PipelineTopology::resolve(high_pcm_rate, target).unwrap_err();
        match err {
            EngineError::InvalidTopology { message } => {
                assert!(message.contains("exceeds DSD bit rate"));
            }
            _ => panic!("expected InvalidTopology"),
        }
    }
}
