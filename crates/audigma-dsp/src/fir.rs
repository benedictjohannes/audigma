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
use crate::rates::{PcmRateHz, RateFamily};
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

    #[error("Input buffer length {len} is not a multiple of decimation factor {factor}")]
    UnalignedDecimationInput { len: usize, factor: usize },
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

/// Precomputed table mapping each byte (8 bits MSB-first) to 8 `f64` samples in `{-1.0, +1.0}`.
/// Bit 1 maps to `+1.0`, bit 0 maps to `-1.0` (`Design.md §4.1`).
pub static DSD_BYTE_TO_FLOATS: [[f64; 8]; 256] = {
    let mut table = [[0.0; 8]; 256];
    let mut b = 0;
    while b < 256 {
        let mut i = 0;
        while i < 8 {
            let bit = (b >> (7 - i)) & 1;
            table[b][i] = if bit == 1 { 1.0 } else { -1.0 };
            i += 1;
        }
        b += 1;
    }
    table
};

/// Fast zero-copy conversion of chronological MSB-first DSD bytes into normalized `f64` samples.
#[inline]
pub fn unpack_dsd_bytes_to_floats(bytes: &[u8], out: &mut [f64]) -> Result<usize, FirError> {
    let required = bytes.len() * 8;
    if out.len() < required {
        return Err(FirError::OutputBufferTooSmall {
            required,
            provided: out.len(),
        });
    }
    for (i, &byte) in bytes.iter().enumerate() {
        let floats = &DSD_BYTE_TO_FLOATS[byte as usize];
        out[i * 8..(i + 1) * 8].copy_from_slice(floats);
    }
    Ok(required)
}

/// Single-stage 2x linear-phase half-band FIR decimation stage (`Design.md §1.6`, `§4.2`).
///
/// Takes $2N$ input samples at rate $2 F_s$ and produces $N$ output samples at rate $F_s$.
/// Exploits half-band structural symmetry:
/// - Even non-center taps are identically 0.0.
/// - Center tap is 0.5 (scaled by 1.0 at DC).
/// - Odd taps are symmetric ($h[-k] = h[k]$), allowing paired sample addition before multiplication.
#[derive(Debug, Clone)]
pub struct HalfBandDecimationStage {
    /// Positive-offset odd taps ($h[1], h[3], \dots$).
    taps: &'static [f64],
    /// Number of odd taps ($K$).
    num_odd_taps: usize,
    /// Circular double-buffer history line.
    history: Vec<f64>,
    /// Head write pointer in circular buffer.
    head: usize,
}

impl HalfBandDecimationStage {
    /// Creates a new 2x half-band decimation stage initialized to digital silence.
    pub fn new(taps: &'static [f64]) -> Self {
        let num_odd_taps = taps.len();
        let window_len = 4 * num_odd_taps;
        Self {
            taps,
            num_odd_taps,
            history: vec![0.0; window_len * 2 + 4],
            head: 0,
        }
    }

    /// Resets all internal delay lines to zero.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
    }

    /// Returns the group delay of this stage in input sample periods ($2K - 1$).
    #[inline]
    pub const fn group_delay_input_samples(&self) -> usize {
        2 * self.num_odd_taps - 1
    }

    /// Processes a slice of input samples, writing exactly `input.len() / 2` samples into `output`.
    pub fn process_tile(&mut self, input: &[f64], output: &mut [f64]) -> Result<usize, FirError> {
        let out_len = input.len() / 2;
        if output.len() < out_len {
            return Err(FirError::OutputBufferTooSmall {
                required: out_len,
                provided: output.len(),
            });
        }

        let k = self.num_odd_taps;
        let window_len = 4 * k;
        let center_offset = 2 * k - 1;

        for (out_idx, pair) in input.chunks_exact(2).enumerate() {
            // Push 2 chronological samples into circular double buffer
            self.history[self.head] = pair[0];
            self.history[self.head + 1] = pair[1];
            self.history[self.head + window_len] = pair[0];
            self.history[self.head + 1 + window_len] = pair[1];

            // Window of 4K samples from oldest to newest
            let window_start = self.head + 2;
            let window = &self.history[window_start..window_start + window_len];

            // Center tap is at index center_offset (2K - 1)
            let center_sample = window[center_offset];

            // Odd taps are symmetric around center_offset:
            // offsets are 1, 3, ..., 2K - 1 (step 2)
            let mut odd_acc = 0.0;
            for (i, &tap) in self.taps.iter().enumerate() {
                let delta = 2 * i + 1;
                let s_pre = window[center_offset - delta];
                let s_post = window[center_offset + delta];
                odd_acc += tap * (s_pre + s_post);
            }

            output[out_idx] = 0.5 * center_sample + odd_acc;

            self.head += 2;
            if self.head >= window_len {
                self.head = 0;
            }
        }

        Ok(out_len)
    }
}

/// Multistage power-of-two ($2^M$) decimation cascade using bounded tile streaming (`Design.md §1.6`, `§4.2`).
///
/// Early stages use relaxed transition taps (`Stage 1+`), while the final stage into baseband
/// uses steep transition taps (`Stage 0`), protecting the audio baseband from ultrasonic noise aliasing.
#[derive(Debug, Clone)]
pub struct HalfBandDecimationCascade {
    stages: Vec<HalfBandDecimationStage>,
    num_stages: usize,
    max_input_frames: usize,
    scratch_a: Vec<f64>,
    scratch_b: Vec<f64>,
    linear_gain: f64,
}

impl HalfBandDecimationCascade {
    /// Creates a new multistage decimation cascade.
    ///
    /// - `preset`: Filter preset determining Stage 0 and Stage 1+ coefficients.
    /// - `stages`: Number of 2x decimation stages (e.g. 5 stages for 32x decimation from DSD256 to 352.8k DXD).
    /// - `max_input_frames`: Maximum high-rate samples or unpacked DSD bits processed in a single tile.
    pub fn new(preset: FilterPreset, stages: usize, max_input_frames: usize) -> Self {
        let mut stage_vec = Vec::with_capacity(stages);
        if stages == 1 {
            // Single stage: steep transition
            stage_vec.push(HalfBandDecimationStage::new(get_halfband_stage0_taps(
                preset,
            )));
        } else if stages > 1 {
            // Early stages (0..stages-1): relaxed transition
            let s1_taps = get_halfband_stage1_taps(preset);
            for _ in 0..(stages - 1) {
                stage_vec.push(HalfBandDecimationStage::new(s1_taps));
            }
            // Final stage: steep transition into baseband
            stage_vec.push(HalfBandDecimationStage::new(get_halfband_stage0_taps(
                preset,
            )));
        }

        Self {
            stages: stage_vec,
            num_stages: stages,
            max_input_frames,
            scratch_a: vec![0.0; max_input_frames],
            scratch_b: vec![0.0; max_input_frames],
            linear_gain: 1.0,
        }
    }

    /// Sets the linear output scaling gain on this cascade.
    pub fn with_linear_gain(mut self, linear_gain: f64) -> Self {
        self.linear_gain = linear_gain;
        self
    }

    /// Sets the output gain in decibels on this cascade (`10^(gain_db / 20)`).
    pub fn with_gain_db(mut self, gain_db: f64) -> Self {
        self.linear_gain = 10.0_f64.powf(gain_db / 20.0);
        self
    }

    /// Sets the linear output scaling gain on this cascade in-place.
    pub fn set_linear_gain(&mut self, linear_gain: f64) {
        self.linear_gain = linear_gain;
    }

    /// Sets the output gain in decibels on this cascade in-place (`10^(gain_db / 20)`).
    pub fn set_gain_db(&mut self, gain_db: f64) {
        self.linear_gain = 10.0_f64.powf(gain_db / 20.0);
    }

    /// Returns the current linear gain multiplier.
    #[inline]
    pub fn linear_gain(&self) -> f64 {
        self.linear_gain
    }

    /// Resets all stages and internal buffers to digital silence.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
        self.scratch_a.fill(0.0);
        self.scratch_b.fill(0.0);
    }

    /// Returns the total decimation rate divider ($2^{\text{stages}}$).
    #[inline]
    pub const fn rate_divider(&self) -> usize {
        1 << self.num_stages
    }

    /// Returns the number of stages in the cascade.
    #[inline]
    pub const fn num_stages(&self) -> usize {
        self.num_stages
    }

    /// Returns the total group delay of the cascade expressed in input samples.
    pub fn total_group_delay_input_samples(&self) -> usize {
        if self.num_stages == 0 {
            return 0;
        }
        let mut total_delay = 0;
        for (i, stage) in self.stages.iter().enumerate() {
            let scale_from_input = 1 << i;
            total_delay += stage.group_delay_input_samples() * scale_from_input;
        }
        total_delay
    }

    /// Processes an input tile of high-rate samples through the full decimation cascade.
    ///
    /// Writes `input.len() / (1 << stages)` samples into `output`.
    /// Zero heap allocation is performed during execution.
    pub fn process_tile(&mut self, input: &[f64], output: &mut [f64]) -> Result<usize, FirError> {
        if input.len() > self.max_input_frames {
            return Err(FirError::TileSizeExceeded {
                provided: input.len(),
                max: self.max_input_frames,
            });
        }
        let total_divider = 1 << self.num_stages;
        if input.len() % total_divider != 0 {
            return Err(FirError::UnalignedDecimationInput {
                len: input.len(),
                factor: total_divider,
            });
        }
        let required_out = input.len() / total_divider;
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
        let mut cur_len = input.len() / 2;
        self.stages[0].process_tile(input, &mut self.scratch_a[..cur_len])?;

        // Stages 1 to num_stages - 2: ping-pong between scratch_a and scratch_b
        let mut in_scratch_a = true;
        for stage_idx in 1..(self.num_stages - 1) {
            let next_len = cur_len / 2;
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

        if (self.linear_gain - 1.0).abs() > 1e-9 {
            for s in output[..required_out].iter_mut() {
                *s *= self.linear_gain;
            }
        }

        Ok(required_out)
    }

    /// Decimates packed 1-bit DSD bytes (MSB first, 8 bits/byte) into multi-bit PCM output.
    ///
    /// The number of DSD bits (`bytes.len() * 8`) must be a multiple of the total decimation factor.
    /// Zero heap allocation is performed during execution.
    pub fn process_dsd_bytes(
        &mut self,
        bytes: &[u8],
        output: &mut [f64],
    ) -> Result<usize, FirError> {
        let total_samples = bytes.len() * 8;
        if total_samples > self.max_input_frames {
            return Err(FirError::TileSizeExceeded {
                provided: total_samples,
                max: self.max_input_frames,
            });
        }
        let total_divider = 1 << self.num_stages;
        if total_samples % total_divider != 0 {
            return Err(FirError::UnalignedDecimationInput {
                len: total_samples,
                factor: total_divider,
            });
        }
        let required_out = total_samples / total_divider;
        if output.len() < required_out {
            return Err(FirError::OutputBufferTooSmall {
                required: required_out,
                provided: output.len(),
            });
        }

        if self.num_stages == 0 {
            unpack_dsd_bytes_to_floats(bytes, &mut output[..total_samples])?;
            if (self.linear_gain - 1.0).abs() > 1e-9 {
                for s in output[..total_samples].iter_mut() {
                    *s *= self.linear_gain;
                }
            }
            return Ok(total_samples);
        }

        // Unpack into scratch_a
        unpack_dsd_bytes_to_floats(bytes, &mut self.scratch_a[..total_samples])?;

        if self.num_stages == 1 {
            self.stages[0].process_tile(
                &self.scratch_a[..total_samples],
                &mut output[..required_out],
            )?;
            if (self.linear_gain - 1.0).abs() > 1e-9 {
                for s in output[..required_out].iter_mut() {
                    *s *= self.linear_gain;
                }
            }
            return Ok(required_out);
        }

        // Stage 0: scratch_a -> scratch_b
        let mut cur_len = total_samples / 2;
        self.stages[0].process_tile(
            &self.scratch_a[..total_samples],
            &mut self.scratch_b[..cur_len],
        )?;

        // Stages 1 to num_stages - 2: ping-pong between scratch_b and scratch_a
        let mut in_scratch_b = true;
        for stage_idx in 1..(self.num_stages - 1) {
            let next_len = cur_len / 2;
            if in_scratch_b {
                let (src, dst) = (&self.scratch_b[..cur_len], &mut self.scratch_a[..next_len]);
                self.stages[stage_idx].process_tile(src, dst)?;
            } else {
                let (src, dst) = (&self.scratch_a[..cur_len], &mut self.scratch_b[..next_len]);
                self.stages[stage_idx].process_tile(src, dst)?;
            }
            cur_len = next_len;
            in_scratch_b = !in_scratch_b;
        }

        // Final stage directly into output
        let last_stage_idx = self.num_stages - 1;
        if in_scratch_b {
            self.stages[last_stage_idx]
                .process_tile(&self.scratch_b[..cur_len], &mut output[..required_out])?;
        } else {
            self.stages[last_stage_idx]
                .process_tile(&self.scratch_a[..cur_len], &mut output[..required_out])?;
        }

        if (self.linear_gain - 1.0).abs() > 1e-9 {
            for s in output[..required_out].iter_mut() {
                *s *= self.linear_gain;
            }
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
        let in_family = in_rate.rate_family();
        let out_family = out_rate.rate_family();

        let (phase_step, num_phases) = match (in_family, out_family) {
            (RateFamily::Hz44100, RateFamily::Hz48000) => {
                // Ratio out_rate / in_rate = (out_mult * 48000) / (in_mult * 44100)
                // = (out_mult / in_mult) * (160 / 147)
                // Since phase_acc advances by phase_step / num_phases = in_rate / out_rate:
                // in_rate / out_rate = (in_mult * 147) / (out_mult * 160)
                let in_mult = in_rate.rate_multiplier().unwrap_or(1);
                let out_mult = out_rate.rate_multiplier().unwrap_or(1);
                let step = (in_mult * 147) as usize;
                let phases = (out_mult * 160) as usize;
                (step, phases)
            }
            (RateFamily::Hz48000, RateFamily::Hz44100) => {
                // Ratio out_rate / in_rate = (out_mult * 44100) / (in_mult * 48000)
                // = (out_mult / in_mult) * (147 / 160)
                // phase_acc step = in_rate / out_rate = (in_mult * 160) / (out_mult * 147)
                let in_mult = in_rate.rate_multiplier().unwrap_or(1);
                let out_mult = out_rate.rate_multiplier().unwrap_or(1);
                let step = (in_mult * 160) as usize;
                let phases = (out_mult * 147) as usize;
                (step, phases)
            }
            _ => {
                return Err(FirError::UnsupportedRatio {
                    in_rate_hz: in_hz,
                    out_rate_hz: out_hz,
                });
            }
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

    #[test]
    fn test_dsd_byte_to_floats_unpacking() {
        let mut out = [0.0; 8];
        // 0xFF -> all +1.0
        unpack_dsd_bytes_to_floats(&[0xFF], &mut out).unwrap();
        assert_eq!(out, [1.0; 8]);

        // 0x00 -> all -1.0
        unpack_dsd_bytes_to_floats(&[0x00], &mut out).unwrap();
        assert_eq!(out, [-1.0; 8]);

        // 0x69 -> [ -1, 1, 1, -1, 1, -1, -1, 1 ] (balanced silence)
        unpack_dsd_bytes_to_floats(&[0x69], &mut out).unwrap();
        assert_eq!(out, [-1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
        assert_eq!(out.iter().sum::<f64>(), 0.0);
    }

    #[test]
    fn test_halfband_decimation_stage_dc_gain() {
        let taps = get_halfband_stage0_taps(FilterPreset::Balanced);
        let mut stage = HalfBandDecimationStage::new(taps);

        // Constant DC input: DC gain must be exactly 1.0
        let dc_in = [1.0; 512];
        let mut dc_out = [0.0; 256];
        let written = stage.process_tile(&dc_in, &mut dc_out).unwrap();
        assert_eq!(written, 256);

        // Discard initial warm-up delay line fill
        let steady = &dc_out[100..250];
        for &s in steady {
            assert!(
                (s - 1.0).abs() < 1e-6,
                "Decimator sample {s} deviates from unity gain 1.0"
            );
        }
    }

    #[test]
    fn test_halfband_decimation_cascade_dsd_bytes() {
        // 5 stages = 32x decimation (e.g. DSD256 at 11.2896 MHz -> 352.8 kHz DXD)
        let mut cascade = HalfBandDecimationCascade::new(FilterPreset::Balanced, 5, 8192);
        assert_eq!(cascade.rate_divider(), 32);
        assert_eq!(cascade.num_stages(), 5);

        // Group delay is around 1587 input samples (approx 50 decimated output samples)
        let delay_inputs = cascade.total_group_delay_input_samples();
        assert!(delay_inputs > 1000);

        // Feed full positive DC (0xFF bytes -> +1.0 floats)
        // 512 bytes * 8 = 4096 DSD bits -> 4096 / 32 = 128 PCM frames
        let dc_bytes = [0xFF; 512];
        let mut pcm_out = [0.0; 128];
        let written = cascade.process_dsd_bytes(&dc_bytes, &mut pcm_out).unwrap();
        assert_eq!(written, 128);

        // After filter delay clears (> 60 decimated samples), output should equal 1.0
        let tail = &pcm_out[80..128];
        for &s in tail {
            assert!(
                (s - 1.0).abs() < 1e-4,
                "Decimated DSD DC sample {s} deviates from 1.0"
            );
        }

        // Test with +6.02 dB (2.0x linear) Scarlet Book normalization gain
        let mut cascade_gain =
            HalfBandDecimationCascade::new(FilterPreset::Balanced, 5, 8192).with_linear_gain(2.0);
        assert_eq!(cascade_gain.linear_gain(), 2.0);
        let mut pcm_gain_out = [0.0; 128];
        let written_gain = cascade_gain
            .process_dsd_bytes(&dc_bytes, &mut pcm_gain_out)
            .unwrap();
        assert_eq!(written_gain, 128);
        for &s in &pcm_gain_out[80..128] {
            assert!(
                (s - 2.0).abs() < 1e-4,
                "Decimated DSD DC sample with 2.0x gain {s} deviates from 2.0"
            );
        }
    }
}
