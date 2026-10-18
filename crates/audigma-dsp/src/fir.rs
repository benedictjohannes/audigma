//! Multistage linear-phase half-band FIR interpolation cascades and polyphase SRC (`Design.md §3.4`, `§4.2`).
//!
//! Provides:
//! - `HalfBandStage`: Zero-allocation 2x upsampler exploiting center-tap delay identity
//!   and linear-phase odd-tap symmetry (4x reduction in arithmetic operations).
//! - `HalfBandCascade`: Staged 2^K power-of-two interpolation chain (up to 256x for DSD256)
//!   using bounded tile streaming to keep intermediate buffers strictly within CPU L1 cache.
//! - `PolyphaseResampler`: Kaiser-windowed sinc polyphase resampler for cross-family rational
//!   conversion (44.1k <-> 48k via 160 phases) and sub-sample ASRC drift tracking (256 phases).

use crate::config::FilterPreset;
use crate::rates::PcmRateHz;
use crate::tables::{
    SRC_ASRC_256_PHASES, SRC_RATIONAL_160_PHASES, get_halfband_stage0_taps,
    get_halfband_stage1_taps,
};
use thiserror::Error;

/// Errors arising from FIR filtering or resampling operations.
#[derive(Debug, Error, PartialEq)]
pub enum FirError {
    #[error("Output buffer too small: required {required}, provided {provided}")]
    OutputBufferTooSmall { required: usize, provided: usize },

    #[error("Tile size {provided} exceeds maximum configured capacity {max}")]
    TileSizeExceeded { provided: usize, max: usize },

    #[error("Unsupported resampling ratio from {in_rate_hz} Hz to {out_rate_hz} Hz")]
    UnsupportedRatio { in_rate_hz: u32, out_rate_hz: u32 },
}

/// Single-stage 2x linear-phase half-band FIR interpolation stage.
///
/// Exploits half-band structural symmetry:
/// - Even non-center taps are identically 0.0.
/// - Center tap is 0.5 (scaled to 1.0 for unity interpolation gain), yielding a direct delayed path.
/// - Odd taps are symmetric ($h[-k] = h[k]$), allowing paired sample addition before multiplication.
#[derive(Debug, Clone)]
pub struct HalfBandStage {
    /// Positive-offset odd taps ($h[1], h[3], \dots$).
    taps: &'static [f64],
    /// Number of odd taps ($K$).
    num_odd_taps: usize,
    /// Circular double-buffer history line of size $4K$.
    history: Vec<f64>,
    /// Head write pointer in circular buffer ($0 \dots 2K-1$).
    head: usize,
}

impl HalfBandStage {
    /// Creates a new 2x half-band stage initialized to digital silence.
    pub fn new(taps: &'static [f64]) -> Self {
        let num_odd_taps = taps.len();
        let history_len = num_odd_taps * 4;
        Self {
            taps,
            num_odd_taps,
            history: vec![0.0; history_len],
            head: 0,
        }
    }

    /// Resets all internal delay lines to zero.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
    }

    /// Returns the group delay of this stage in input sample periods ($K$).
    #[inline]
    pub const fn group_delay_input_samples(&self) -> usize {
        self.num_odd_taps
    }

    /// Returns the group delay of this stage in 2x output sample periods ($2K - 1$).
    #[inline]
    pub const fn group_delay_output_samples(&self) -> usize {
        2 * self.num_odd_taps - 1
    }

    /// Processes a slice of input samples, writing exactly `2 * input.len()` samples into `output`.
    pub fn process_tile(&mut self, input: &[f64], output: &mut [f64]) -> Result<(), FirError> {
        let required_out = input.len() * 2;
        if output.len() < required_out {
            return Err(FirError::OutputBufferTooSmall {
                required: required_out,
                provided: output.len(),
            });
        }

        let k = self.num_odd_taps;
        let window_len = 2 * k;

        for (in_idx, &sample) in input.iter().enumerate() {
            // Insert sample into mirrored positions in double-buffer
            self.history[self.head] = sample;
            self.history[self.head + window_len] = sample;

            // Window of 2K chronological samples from oldest to newest
            let window_start = self.head + 1;
            let window = &self.history[window_start..window_start + window_len];

            // Odd branch: convolve symmetric odd taps
            // Midpoint is between window[k - 1] and window[k]
            let pre_ptr = &window[k - 1] as *const f64;
            let post_ptr = &window[k] as *const f64;
            let mut odd_acc = 0.0;
            for (i, &tap) in self.taps.iter().enumerate() {
                // Safety: i < k, so pre_ptr.sub(i) >= window[0] and post_ptr.add(i) <= window[2k - 1]
                let s_pre = unsafe { *pre_ptr.sub(i) };
                let s_post = unsafe { *post_ptr.add(i) };
                odd_acc += tap * (s_pre + s_post);
            }
            // Scale by 2.0 to restore unity DC gain for zero-stuffed interpolation
            let odd_sample = 2.0 * odd_acc;

            // Even branch: delayed center tap sample
            let even_sample = window[k - 1];

            // Interleaved output
            output[in_idx * 2] = even_sample;
            output[in_idx * 2 + 1] = odd_sample;

            // Advance head pointer
            self.head += 1;
            if self.head >= window_len {
                self.head = 0;
            }
        }

        Ok(())
    }
}

/// Multistage power-of-two ($2^K$) interpolation cascade using bounded tile streaming.
///
/// Designed to keep all intermediate upsampled buffers within the CPU L1 data cache.
/// Stage 0 uses steep transition taps, while Stage 1+ use relaxed transition taps (`Design.md §4.2`).
#[derive(Debug, Clone)]
pub struct HalfBandCascade {
    stages: Vec<HalfBandStage>,
    num_stages: usize,
    max_input_frames: usize,
    scratch_a: Vec<f64>,
    scratch_b: Vec<f64>,
}

impl HalfBandCascade {
    /// Creates a new power-of-two interpolation cascade.
    ///
    /// - `preset`: Filter preset determining Stage 0 and Stage 1+ coefficients.
    /// - `stages`: Number of 2x stages (e.g. 8 stages for 256x upsampling to DSD256).
    /// - `max_input_frames`: Maximum baseband frames processed in a single tile (e.g. 32 to 1024).
    pub fn new(preset: FilterPreset, stages: usize, max_input_frames: usize) -> Self {
        let mut stage_vec = Vec::with_capacity(stages);
        if stages > 0 {
            // Stage 0: steep transition
            stage_vec.push(HalfBandStage::new(get_halfband_stage0_taps(preset)));
            // Stage 1+: relaxed transition
            let s1_taps = get_halfband_stage1_taps(preset);
            for _ in 1..stages {
                stage_vec.push(HalfBandStage::new(s1_taps));
            }
        }

        let max_expanded_len = max_input_frames * (1 << stages);
        Self {
            stages: stage_vec,
            num_stages: stages,
            max_input_frames,
            scratch_a: vec![0.0; max_expanded_len],
            scratch_b: vec![0.0; max_expanded_len],
        }
    }

    /// Resets all stages and internal buffers to digital silence.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
        self.scratch_a.fill(0.0);
        self.scratch_b.fill(0.0);
    }

    /// Returns the total rate multiplier ($2^{\text{stages}}$).
    #[inline]
    pub const fn rate_multiplier(&self) -> usize {
        1 << self.num_stages
    }

    /// Returns the number of stages in the cascade.
    #[inline]
    pub const fn num_stages(&self) -> usize {
        self.num_stages
    }

    /// Returns the total group delay of the cascade expressed in final output samples.
    pub fn total_group_delay_output_samples(&self) -> usize {
        if self.num_stages == 0 {
            return 0;
        }
        let mut total_delay = 0;
        for (i, stage) in self.stages.iter().enumerate() {
            let scale_to_final = 1 << (self.num_stages - 1 - i);
            total_delay += stage.group_delay_output_samples() * scale_to_final;
        }
        total_delay
    }

    /// Processes an input tile of baseband frames through the full cascade.
    ///
    /// Writes `input.len() * (1 << stages)` samples into `output`.
    /// Zero heap allocation is performed during execution.
    pub fn process_tile(&mut self, input: &[f64], output: &mut [f64]) -> Result<usize, FirError> {
        if input.len() > self.max_input_frames {
            return Err(FirError::TileSizeExceeded {
                provided: input.len(),
                max: self.max_input_frames,
            });
        }
        let total_mult = 1 << self.num_stages;
        let required_out = input.len() * total_mult;
        if output.len() < required_out {
            return Err(FirError::OutputBufferTooSmall {
                required: required_out,
                provided: output.len(),
            });
        }

        if self.num_stages == 0 {
            output[..input.len()].copy_from_slice(input);
            return Ok(input.len());
        }

        if self.num_stages == 1 {
            self.stages[0].process_tile(input, &mut output[..required_out])?;
            return Ok(required_out);
        }

        // Stage 0: input -> scratch_a
        let mut cur_len = input.len() * 2;
        self.stages[0].process_tile(input, &mut self.scratch_a[..cur_len])?;

        // Stages 1 to num_stages - 2: ping-pong between scratch_a and scratch_b
        let mut in_scratch_a = true;
        for stage_idx in 1..(self.num_stages - 1) {
            let next_len = cur_len * 2;
            if in_scratch_a {
                let (src, dst) = (&self.scratch_a[..cur_len], &mut self.scratch_b[..next_len]);
                self.stages[stage_idx].process_tile(src, dst)?;
            } else {
                let (src, dst) = (&self.scratch_b[..cur_len], &mut self.scratch_a[..next_len]);
                self.stages[stage_idx].process_tile(src, dst)?;
            }
            cur_len = next_len;
            in_scratch_a = !in_scratch_a;
        }

        // Final stage: from current scratch directly into output
        let last_stage_idx = self.num_stages - 1;
        if in_scratch_a {
            self.stages[last_stage_idx]
                .process_tile(&self.scratch_a[..cur_len], &mut output[..required_out])?;
        } else {
            self.stages[last_stage_idx]
                .process_tile(&self.scratch_b[..cur_len], &mut output[..required_out])?;
        }

        Ok(required_out)
    }
}

/// Mode of operation for the polyphase resampler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolyphaseMode {
    /// Exact rational conversion between 44.1 kHz and 48 kHz rate families (160 phases).
    Rational {
        phase_step: usize,
        num_phases: usize,
    },
    /// Continuously variable fractional ASRC drift tracking (256 phases).
    Asrc { nominal_ratio: f64, drift_ppm: f64 },
}

/// Kaiser-windowed sinc polyphase resampler for cross-family conversions and ASRC drift tracking (`Design.md §4.2`).
#[derive(Debug, Clone)]
pub struct PolyphaseResampler {
    mode: PolyphaseMode,
    history: Vec<f64>,
    head: usize,
    phase_acc: f64,
    taps_per_phase: usize,
}

impl PolyphaseResampler {
    /// Creates a rational resampler for cross-family conversions (44.1k <-> 48k).
    pub fn new_rational(in_rate: PcmRateHz, out_rate: PcmRateHz) -> Result<Self, FirError> {
        let (in_hz, out_hz) = (in_rate.get(), out_rate.get());
        // Check for 44.1k <-> 48k baseband relationship
        // 44100 * 160 / 147 == 48000
        let (phase_step, num_phases) = if (in_hz == 44_100 && out_hz == 48_000)
            || (in_hz == 88_200 && out_hz == 96_000)
            || (in_hz == 176_400 && out_hz == 192_000)
            || (in_hz == 352_800 && out_hz == 384_000)
        {
            // Up-sampling by 160/147: step is 147 in 160-phase grid
            (147, 160)
        } else if (in_hz == 48_000 && out_hz == 44_100)
            || (in_hz == 96_000 && out_hz == 88_200)
            || (in_hz == 192_000 && out_hz == 176_400)
            || (in_hz == 384_000 && out_hz == 352_800)
        {
            // Down-sampling by 147/160: step is 160 in 147-phase grid
            (160, 147)
        } else {
            return Err(FirError::UnsupportedRatio {
                in_rate_hz: in_hz,
                out_rate_hz: out_hz,
            });
        };

        const TAPS: usize = 48;
        Ok(Self {
            mode: PolyphaseMode::Rational {
                phase_step,
                num_phases,
            },
            history: vec![0.0; TAPS * 2],
            head: 0,
            phase_acc: 0.0,
            taps_per_phase: TAPS,
        })
    }

    /// Creates a continuous ASRC resampler referenced to a nominal conversion ratio.
    pub fn new_asrc(in_rate: PcmRateHz, out_rate: PcmRateHz) -> Self {
        let nominal_ratio = in_rate.get() as f64 / out_rate.get() as f64;
        const TAPS: usize = 48;
        Self {
            mode: PolyphaseMode::Asrc {
                nominal_ratio,
                drift_ppm: 0.0,
            },
            history: vec![0.0; TAPS * 2],
            head: 0,
            phase_acc: 0.0,
            taps_per_phase: TAPS,
        }
    }

    /// Updates the measured clock drift in parts-per-million (PPM) for ASRC mode.
    pub fn set_drift_ppm(&mut self, ppm: f64) {
        if let PolyphaseMode::Asrc {
            ref mut drift_ppm, ..
        } = self.mode
        {
            *drift_ppm = ppm;
        }
    }

    /// Resets delay line and phase accumulators to zero.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
        self.phase_acc = 0.0;
    }

    /// Processes an input buffer through the resampler.
    ///
    /// Returns `(consumed_input_frames, produced_output_frames)`.
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> (usize, usize) {
        let taps = self.taps_per_phase;
        let mut in_idx = 0;
        let mut out_idx = 0;

        match self.mode {
            PolyphaseMode::Rational {
                phase_step,
                num_phases,
            } => {
                let step_f = phase_step as f64 / num_phases as f64;
                while in_idx < input.len() && out_idx < output.len() {
                    while self.phase_acc < 1.0 && out_idx < output.len() {
                        let phase_idx = ((self.phase_acc * 160.0).floor() as usize).min(159);
                        let filter_phase = &SRC_RATIONAL_160_PHASES[phase_idx];

                        // Convolve 48 taps with delay line history
                        let window_start = self.head + 1;
                        let window = &self.history[window_start..window_start + taps];
                        let mut sum = 0.0;
                        for i in 0..taps {
                            sum += filter_phase[i] * window[taps - 1 - i];
                        }
                        output[out_idx] = sum;
                        out_idx += 1;
                        self.phase_acc += step_f;
                    }

                    while self.phase_acc >= 1.0 && in_idx < input.len() {
                        let sample = input[in_idx];
                        in_idx += 1;
                        self.history[self.head] = sample;
                        self.history[self.head + taps] = sample;
                        self.head += 1;
                        if self.head >= taps {
                            self.head = 0;
                        }
                        self.phase_acc -= 1.0;
                    }
                }
            }
            PolyphaseMode::Asrc {
                nominal_ratio,
                drift_ppm,
            } => {
                let ratio = nominal_ratio * (1.0 + drift_ppm * 1e-6);
                while in_idx < input.len() && out_idx < output.len() {
                    while self.phase_acc < 1.0 && out_idx < output.len() {
                        let phase_idx = ((self.phase_acc * 256.0).floor() as usize).min(255);
                        let filter_phase = &SRC_ASRC_256_PHASES[phase_idx];

                        let window_start = self.head + 1;
                        let window = &self.history[window_start..window_start + taps];
                        let mut sum = 0.0;
                        for i in 0..taps {
                            sum += filter_phase[i] * window[taps - 1 - i];
                        }
                        output[out_idx] = sum;
                        out_idx += 1;
                        self.phase_acc += ratio;
                    }

                    while self.phase_acc >= 1.0 && in_idx < input.len() {
                        let sample = input[in_idx];
                        in_idx += 1;
                        self.history[self.head] = sample;
                        self.history[self.head + taps] = sample;
                        self.head += 1;
                        if self.head >= taps {
                            self.head = 0;
                        }
                        self.phase_acc -= 1.0;
                    }
                }
            }
        }

        (in_idx, out_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_halfband_stage_dc_gain_and_ramp() {
        let taps = get_halfband_stage0_taps(FilterPreset::Balanced);
        let mut stage = HalfBandStage::new(taps);

        // Constant DC input: DC gain must be exactly 1.0
        // Stage 0 balanced has 44 odd taps (2K = 88 input samples memory = 176 output samples)
        let dc_in = [1.0; 256];
        let mut dc_out = [0.0; 512];
        stage.process_tile(&dc_in, &mut dc_out).unwrap();

        // After filter delay clears (> 176 output samples), all samples should equal 1.0
        let tail = &dc_out[200..500];
        for &s in tail {
            assert!(
                (s - 1.0).abs() < 1e-6,
                "DC sample {s} deviates from unity gain 1.0"
            );
        }
    }

    #[test]
    fn test_halfband_cascade_256x_dsd256() {
        let mut cascade = HalfBandCascade::new(FilterPreset::Balanced, 8, 32);
        assert_eq!(cascade.rate_multiplier(), 256);
        assert_eq!(cascade.num_stages(), 8);

        // 16 input frames -> 16 * 256 = 4096 samples
        let input = [0.5; 16];
        let mut output = [0.0; 4096];
        let written = cascade.process_tile(&input, &mut output).unwrap();
        assert_eq!(written, 4096);
    }

    #[test]
    fn test_polyphase_rational_dc_gain() {
        let mut resampler =
            PolyphaseResampler::new_rational(PcmRateHz::PCM_44100, PcmRateHz::PCM_48000).unwrap();

        let dc_in = [1.0; 256];
        let mut dc_out = [0.0; 300];
        let (_in_read, out_written) = resampler.process(&dc_in, &mut dc_out);
        assert!(out_written > 200);

        // Discard initial warm-up tail
        let steady = &dc_out[100..out_written - 20];
        for &s in steady {
            assert!(
                (s - 1.0).abs() < 1e-4,
                "Rational SRC sample {s} deviates from unity gain"
            );
        }
    }

    #[test]
    fn test_polyphase_asrc_drift_stability() {
        let mut asrc = PolyphaseResampler::new_asrc(PcmRateHz::PCM_48000, PcmRateHz::PCM_48000);
        asrc.set_drift_ppm(200.0); // +200 ppm clock drift

        let dc_in = [0.75; 256];
        let mut dc_out = [0.0; 300];
        let (_in_read, out_written) = asrc.process(&dc_in, &mut dc_out);
        assert!(out_written > 200);

        let steady = &dc_out[100..out_written - 20];
        for &s in steady {
            assert!(
                (s - 0.75).abs() < 1e-3,
                "ASRC sample {s} deviates from DC level 0.75"
            );
        }
    }
}
