//! Worker-local input DSP. Each channel retains its own decimation residue,
//! FIR history, and SRC phase; the coordinator supplies shared frame controls.

use crate::decimator::ChannelDecimatorPool;
use crate::lifecycle::FrameGain;
use crate::traits::{DsdDecodeConfig, EngineError};
use audigma_dsp::FilterPreset;
use audigma_dsp::fir::{HalfBandDecimationCascade, PolyphaseResampler};
use audigma_dsp::rates::PcmRateHz;

/// Resampling after input decimation, before the output interpolation cascade.
#[derive(Debug, Clone, Copy)]
pub enum InputResampling {
    None,
    Rational {
        input_rate: PcmRateHz,
        output_rate: PcmRateHz,
    },
    Adaptive {
        input_rate: PcmRateHz,
        output_rate: PcmRateHz,
        max_correction_ppm: f64,
    },
}

/// Optional input stages run by the existing persistent channel workers.
#[derive(Debug, Clone, Copy)]
pub struct InputDspConfig {
    pub pcm_decimation_stages: usize,
    pub dsd: Option<DsdDecodeConfig>,
    pub resampling: InputResampling,
}

/// Immutable controls shared by every channel for one input block.
#[derive(Debug, Clone, Copy)]
pub struct InputTileControls<'a> {
    pub frame_gains: &'a [FrameGain],
    pub linear_gain: f64,
    pub drift_ppm: f64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct InputBounds {
    pub pcm_frames: usize,
    pub dsd_bytes: usize,
    pub output_frames: usize,
    pub pcm_divider: usize,
    pub dsd_divider: usize,
}

fn invalid(message: &str) -> EngineError {
    EngineError::InvalidTopology {
        message: message.into(),
    }
}

fn divider(stages: usize) -> Result<usize, EngineError> {
    let shift =
        u32::try_from(stages).map_err(|_| invalid("input decimation stage count overflow"))?;
    1usize
        .checked_shl(shift)
        .ok_or_else(|| invalid("input decimation stage count overflow"))
}

impl InputDspConfig {
    pub(super) fn bounds(self, baseband_frames: usize) -> Result<InputBounds, EngineError> {
        let pcm_divider = divider(self.pcm_decimation_stages)?;
        let pcm_frames = baseband_frames
            .checked_mul(pcm_divider)
            .ok_or_else(|| invalid("PCM input tile capacity overflow"))?;
        let dsd_divider = self
            .dsd
            .map(|dsd| divider(dsd.decimation_stages))
            .transpose()?
            .unwrap_or(1);
        let dsd_bytes = if self.dsd.is_some() {
            let bits = pcm_frames
                .checked_mul(dsd_divider)
                .ok_or_else(|| invalid("DSD input tile capacity overflow"))?;
            if bits < 8 {
                return Err(invalid(
                    "input tile must hold at least one complete DSD byte",
                ));
            }
            bits / 8
        } else {
            0
        };

        let output_frames = match self.resampling {
            InputResampling::None => baseband_frames,
            InputResampling::Rational {
                input_rate,
                output_rate,
            } => Self::resampled_capacity(baseband_frames, input_rate, output_rate, 0.0)?,
            InputResampling::Adaptive {
                input_rate,
                output_rate,
                max_correction_ppm,
            } => {
                if !max_correction_ppm.is_finite()
                    || !(0.0..1_000_000.0).contains(&max_correction_ppm)
                {
                    return Err(invalid(
                        "ASRC correction limit must be finite and below 1000000 ppm",
                    ));
                }
                Self::resampled_capacity(
                    baseband_frames,
                    input_rate,
                    output_rate,
                    max_correction_ppm,
                )?
            }
        };

        Ok(InputBounds {
            pcm_frames,
            dsd_bytes,
            output_frames,
            pcm_divider,
            dsd_divider,
        })
    }

    fn resampled_capacity(
        frames: usize,
        input_rate: PcmRateHz,
        output_rate: PcmRateHz,
        max_ppm: f64,
    ) -> Result<usize, EngineError> {
        // Reserve the worst negative correction plus one phase-boundary frame.
        // The whole SRC result stays in one worker dispatch, including short tails
        // that previously required a second interpolation/modulation barrier.
        let capacity = (frames as f64 * output_rate.get() as f64
            / (input_rate.get() as f64 * (1.0 - max_ppm * 1e-6)))
            .ceil();
        if !capacity.is_finite() || capacity >= usize::MAX as f64 {
            return Err(invalid("resampled tile capacity overflow"));
        }
        (capacity as usize)
            .checked_add(1)
            .ok_or_else(|| invalid("resampled tile capacity overflow"))
    }

    pub(super) fn validate_controls(
        self,
        controls: InputTileControls<'_>,
    ) -> Result<(), EngineError> {
        if let InputResampling::Adaptive {
            max_correction_ppm, ..
        } = self.resampling
        {
            if !controls.drift_ppm.is_finite() || controls.drift_ppm.abs() > max_correction_ppm {
                return Err(invalid("ASRC correction exceeds the prepared worker limit"));
            }
        }
        Ok(())
    }
}

pub(super) struct InputProcessor {
    pcm_decimator: Option<ChannelDecimatorPool>,
    dsd_decimator: Option<HalfBandDecimationCascade>,
    dsd_alignment: usize,
    dsd_residue: Vec<u8>,
    dsd_scratch: Vec<u8>,
    dsd_pcm: Vec<f64>,
    baseband: Vec<f64>,
    resampler: Option<PolyphaseResampler>,
    resampled: Vec<f64>,
}

impl InputProcessor {
    pub(super) fn new(
        preset: FilterPreset,
        frames: usize,
        config: InputDspConfig,
        bounds: InputBounds,
    ) -> Result<Self, EngineError> {
        let pcm_decimator = (config.pcm_decimation_stages > 0).then(|| {
            ChannelDecimatorPool::new(preset, config.pcm_decimation_stages, 1, bounds.pcm_frames)
        });
        let dsd_decimator = config.dsd.map(|dsd| {
            HalfBandDecimationCascade::new(dsd.preset, dsd.decimation_stages, bounds.dsd_bytes * 8)
                .with_gain_db(dsd.gain_db)
        });
        let dsd_alignment = bounds.dsd_divider.div_ceil(8);
        let dsd_scratch_bytes = if config.dsd.is_some() {
            bounds
                .dsd_bytes
                .checked_add(dsd_alignment - 1)
                .ok_or_else(|| invalid("DSD residue capacity overflow"))?
        } else {
            0
        };
        let resampler = match config.resampling {
            InputResampling::None => None,
            InputResampling::Rational {
                input_rate,
                output_rate,
            } => Some(PolyphaseResampler::new_rational(input_rate, output_rate)?),
            InputResampling::Adaptive {
                input_rate,
                output_rate,
                ..
            } => Some(PolyphaseResampler::new_asrc(input_rate, output_rate)),
        };
        let resampled_frames = if resampler.is_some() {
            bounds.output_frames
        } else {
            0
        };
        let dsd_pcm_frames = if config.dsd.is_some() && pcm_decimator.is_some() {
            bounds.pcm_frames
        } else {
            0
        };
        Ok(Self {
            pcm_decimator,
            dsd_decimator,
            dsd_alignment,
            dsd_residue: Vec::with_capacity(if config.dsd.is_some() {
                dsd_alignment
            } else {
                0
            }),
            dsd_scratch: vec![0; dsd_scratch_bytes],
            dsd_pcm: vec![0.0; dsd_pcm_frames],
            baseband: vec![0.0; frames],
            resampler,
            resampled: vec![0.0; resampled_frames],
        })
    }

    pub(super) fn reset(&mut self) {
        if let Some(decimator) = self.pcm_decimator.as_mut() {
            decimator.reset();
        }
        if let Some(decimator) = self.dsd_decimator.as_mut() {
            decimator.reset();
        }
        self.dsd_residue.clear();
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }

    pub(super) fn process_pcm(
        &mut self,
        input: &[f64],
        controls: InputTileControls<'_>,
    ) -> Result<&[f64], EngineError> {
        let frames = if let Some(decimator) = self.pcm_decimator.as_mut() {
            decimator.process(&[input], &mut [&mut self.baseband])?
        } else {
            self.baseband[..input.len()].copy_from_slice(input);
            input.len()
        };
        self.finish(frames, controls)
    }

    pub(super) fn process_dsd(
        &mut self,
        input: &[u8],
        controls: InputTileControls<'_>,
    ) -> Result<&[f64], EngineError> {
        let decimator = self
            .dsd_decimator
            .as_mut()
            .ok_or_else(|| invalid("channel worker has no raw DSD input configuration"))?;
        let residue = self.dsd_residue.len();
        self.dsd_scratch[..residue].copy_from_slice(&self.dsd_residue);
        self.dsd_scratch[residue..residue + input.len()].copy_from_slice(input);
        let available = residue + input.len();
        let aligned = available / self.dsd_alignment * self.dsd_alignment;
        self.dsd_residue.clear();
        self.dsd_residue
            .extend_from_slice(&self.dsd_scratch[aligned..available]);
        let frames = if let Some(pcm_decimator) = self.pcm_decimator.as_mut() {
            // Preserve the two distinct FIR cascades: the DSD decoder protects
            // its advertised PCM rate, then PCM downsampling protects the target.
            let decoded =
                decimator.process_dsd_bytes(&self.dsd_scratch[..aligned], &mut self.dsd_pcm)?;
            pcm_decimator.process(&[&self.dsd_pcm[..decoded]], &mut [&mut self.baseband])?
        } else {
            decimator.process_dsd_bytes(&self.dsd_scratch[..aligned], &mut self.baseband)?
        };
        self.finish(frames, controls)
    }

    fn finish(
        &mut self,
        frames: usize,
        controls: InputTileControls<'_>,
    ) -> Result<&[f64], EngineError> {
        if controls.frame_gains.len() != frames {
            return Err(invalid(
                "channel worker's decimation residue lost alignment",
            ));
        }
        for (sample, gain) in self.baseband[..frames].iter_mut().zip(controls.frame_gains) {
            // Keep these as separate operations, in the original order. In
            // particular, lifecycle silence must discard nonfinite input first.
            *sample = gain.apply(*sample);
            *sample *= controls.linear_gain;
        }
        if frames == 0 {
            return Ok(&self.baseband[..0]);
        }
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.set_drift_ppm(controls.drift_ppm);
            let (consumed, produced) =
                resampler.process(&self.baseband[..frames], &mut self.resampled);
            if consumed != frames {
                return Err(invalid(
                    "channel worker's SRC output exceeded its prepared capacity",
                ));
            }
            Ok(&self.resampled[..produced])
        } else {
            Ok(&self.baseband[..frames])
        }
    }
}
