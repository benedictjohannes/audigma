//! Multistage linear-phase half-band FIR interpolation cascades and polyphase SRC (`Design.md §3.4`, `§4.2`).
//!
//! Provides:
//! - `HalfBandStage`: Zero-allocation 2x upsampler exploiting center-tap delay identity
//!   and linear-phase odd-tap symmetry (4x reduction in arithmetic operations).
//! - `HalfBandCascade`: Staged 2^K power-of-two interpolation chain (up to 512x for DSD512)
//!   using bounded tile streaming to limit intermediate-buffer memory.
//! - `PolyphaseResampler`: Kaiser-windowed sinc polyphase resampler for exact rational conversion
//!   (44.1k <-> 48k at any tier, anti-aliased when downsampling) and ASRC drift tracking (256 phases,
//!   cubic interpolation between them). Banks are built from a closed-form prototype at construction.

use crate::config::{Apodization, FilterPhase, FilterPreset};
use crate::rates::PcmRateHz;
use crate::tables::{
    HalfBandTaps, SRC_ASRC_PHASES, SRC_CUTOFF, SRC_KAISER_BETA, SRC_TAPS_UNITY,
    get_halfband_stage0_filter, get_halfband_stage0_taps, get_halfband_stage1_filter,
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
        // The high-rate stages use these short kernels. Dispatch once per tile so
        // LLVM can unroll their tap loops without changing accumulation order.
        match self.num_odd_taps {
            6 => self.process_tile_kernel::<6>(input, output),
            7 => self.process_tile_kernel::<7>(input, output),
            8 => self.process_tile_kernel::<8>(input, output),
            _ => self.process_tile_kernel::<0>(input, output),
        }
    }

    fn process_tile_kernel<const K: usize>(
        &mut self,
        input: &[f64],
        output: &mut [f64],
    ) -> Result<(), FirError> {
        let required_out = input.len() * 2;
        if output.len() < required_out {
            return Err(FirError::OutputBufferTooSmall {
                required: required_out,
                provided: output.len(),
            });
        }

        let k = if K == 0 { self.num_odd_taps } else { K };
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
            for i in 0..k {
                let tap = self.taps[i];
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

/// Generic 2x polyphase FIR interpolation stage for asymmetric / non-half-band prototypes.
///
/// The full-length prototype $h[n]$ is split into the two polyphase branches
/// $E_0(z) = \sum_m h[2m] z^{-m}$ and $E_1(z) = \sum_m h[2m+1] z^{-m}$.
/// Both branches are scaled by 2.0 to restore unity interpolation gain after
/// zero-stuffing. This supports minimum-phase filters (no pre-ringing, causal
/// energy) as well as linear-phase apodized filters that are not exact half-bands.
#[derive(Debug, Clone)]
pub struct PolyphaseInterpolationStage {
    /// Even polyphase branch taps ($2 h[2m]$).
    even_taps: Vec<f64>,
    /// Odd polyphase branch taps ($2 h[2m+1]$).
    odd_taps: Vec<f64>,
    /// Number of taps per polyphase branch ($M = \lceil L / 2 \rceil$).
    taps_per_phase: usize,
    /// Circular double-buffer history line of size $2M$.
    history: Vec<f64>,
    /// Head write pointer in the circular buffer.
    head: usize,
    /// Impulse-response peak index, used as the alignment delay in output samples.
    group_delay_output_samples: usize,
}

impl PolyphaseInterpolationStage {
    /// Creates a new 2x polyphase stage from a full-length FIR prototype.
    pub fn new(taps: &'static [f64]) -> Self {
        assert!(!taps.is_empty(), "polyphase prototype must not be empty");
        let taps_per_phase = taps.len().div_ceil(2);

        let mut even_taps = Vec::with_capacity(taps_per_phase);
        let mut odd_taps = Vec::with_capacity(taps_per_phase);
        for m in 0..taps_per_phase {
            even_taps.push(2.0 * taps.get(2 * m).copied().unwrap_or(0.0));
            odd_taps.push(2.0 * taps.get(2 * m + 1).copied().unwrap_or(0.0));
        }

        let mut peak_index = 0;
        let mut peak_magnitude = 0.0_f64;
        for (idx, &tap) in taps.iter().enumerate() {
            let magnitude = tap.abs();
            if magnitude > peak_magnitude {
                peak_magnitude = magnitude;
                peak_index = idx;
            }
        }

        Self {
            even_taps,
            odd_taps,
            taps_per_phase,
            history: vec![0.0; taps_per_phase * 2],
            head: 0,
            group_delay_output_samples: peak_index,
        }
    }

    /// Resets all internal delay lines to zero.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
    }

    /// Returns the alignment delay of this stage in 2x output sample periods.
    #[inline]
    pub const fn group_delay_output_samples(&self) -> usize {
        self.group_delay_output_samples
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

        let m = self.taps_per_phase;
        for (in_idx, &sample) in input.iter().enumerate() {
            // Insert sample into mirrored positions in the double-buffer.
            self.history[self.head] = sample;
            self.history[self.head + m] = sample;

            // Window of M chronological samples from oldest to newest.
            let window_start = self.head + 1;
            let window = &self.history[window_start..window_start + m];

            let mut even_acc = 0.0;
            let mut odd_acc = 0.0;
            for (idx, (even_tap, odd_tap)) in
                self.even_taps.iter().zip(self.odd_taps.iter()).enumerate()
            {
                let delayed = window[m - 1 - idx];
                even_acc += even_tap * delayed;
                odd_acc += odd_tap * delayed;
            }

            output[in_idx * 2] = even_acc;
            output[in_idx * 2 + 1] = odd_acc;

            self.head += 1;
            if self.head >= m {
                self.head = 0;
            }
        }

        Ok(())
    }
}

/// One 2x interpolation stage selected by the configured filter phase/apodization.
#[derive(Debug, Clone)]
enum CascadeStage {
    /// Linear-phase exact half-band prototype (folded symmetric kernel).
    HalfBand(HalfBandStage),
    /// General full-length prototype (asymmetric / apodized).
    Polyphase(PolyphaseInterpolationStage),
}

impl CascadeStage {
    fn from_taps(taps: HalfBandTaps) -> Self {
        match taps {
            HalfBandTaps::Symmetric(odd_taps) => Self::HalfBand(HalfBandStage::new(odd_taps)),
            HalfBandTaps::General(full_taps) => {
                Self::Polyphase(PolyphaseInterpolationStage::new(full_taps))
            }
        }
    }

    fn reset(&mut self) {
        match self {
            Self::HalfBand(stage) => stage.reset(),
            Self::Polyphase(stage) => stage.reset(),
        }
    }

    fn group_delay_output_samples(&self) -> usize {
        match self {
            Self::HalfBand(stage) => stage.group_delay_output_samples(),
            Self::Polyphase(stage) => stage.group_delay_output_samples(),
        }
    }

    fn process_tile(&mut self, input: &[f64], output: &mut [f64]) -> Result<(), FirError> {
        match self {
            Self::HalfBand(stage) => stage.process_tile(input, output),
            Self::Polyphase(stage) => stage.process_tile(input, output),
        }
    }
}

/// Multistage power-of-two ($2^K$) interpolation cascade using bounded tile streaming.
///
/// Intermediate buffer sizes are bounded by the configured input tile size and
/// rate multiplier; cache residency depends on those sizes and the host CPU.
/// Stage 0 uses steep transition taps, while Stage 1+ use relaxed transition taps (`Design.md §4.2`).
#[derive(Debug, Clone)]
pub struct HalfBandCascade {
    stages: Vec<CascadeStage>,
    num_stages: usize,
    max_input_frames: usize,
    scratch_a: Vec<f64>,
    scratch_b: Vec<f64>,
}

impl HalfBandCascade {
    /// Creates a new power-of-two interpolation cascade.
    ///
    /// - `preset`: Filter preset determining Stage 0 and Stage 1+ coefficients.
    /// - `phase`: Linear- or minimum-phase coefficient family.
    /// - `apodization`: Off (brick-wall) or gentle Stage 0 roll-off.
    /// - `stages`: Number of 2x stages (e.g. 8 stages for 256x upsampling to DSD256, 9 stages for 512x to DSD512).
    /// - `max_input_frames`: Maximum baseband frames processed in a single tile (e.g. 32 to 1024).
    pub fn new(
        preset: FilterPreset,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        max_input_frames: usize,
    ) -> Self {
        let mut stage_vec = Vec::with_capacity(stages);
        if stages > 0 {
            // Stage 0: steep transition, selected by phase and apodization.
            stage_vec.push(CascadeStage::from_taps(get_halfband_stage0_filter(
                preset,
                phase,
                apodization,
            )));
            // Stage 1+: relaxed transition. Apodization only affects Stage 0.
            let s1_taps = get_halfband_stage1_filter(preset, phase);
            for _ in 1..stages {
                stage_vec.push(CascadeStage::from_taps(s1_taps));
            }
        }

        // The final stage writes directly to the caller's output. Size each
        // ping-pong buffer only for the largest intermediate it actually holds.
        let mut scratch_a_len = 0;
        let mut scratch_b_len = 0;
        for stage_idx in 0..stages.saturating_sub(1) {
            let len = max_input_frames * (1 << (stage_idx + 1));
            if stage_idx % 2 == 0 {
                scratch_a_len = len;
            } else {
                scratch_b_len = len;
            }
        }
        Self {
            stages: stage_vec,
            num_stages: stages,
            max_input_frames,
            scratch_a: vec![0.0; scratch_a_len],
            scratch_b: vec![0.0; scratch_b_len],
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

/// Modified Bessel function of the first kind, order 0 (power series; same summation as `design_src.py`).
fn bessel_i0(x: f64) -> f64 {
    let half = 0.5 * x;
    let mut term = 1.0;
    let mut total = 1.0;
    let mut k = 1.0;
    loop {
        let r = half / k;
        term *= r * r;
        total += term;
        if term < total * 1e-17 {
            return total;
        }
        k += 1.0;
    }
}

/// Band-limit scale of a converter: `min(1, out/in)`, so the prototype's band edges sit on the lower rate.
pub fn src_band_limit_scale(in_hz: f64, out_hz: f64) -> f64 {
    (out_hz / in_hz).min(1.0)
}

/// Taps per output sample for band-limit scale `scale` (`design_src.taps_for_scale`).
pub fn src_taps_for_scale(scale: f64) -> usize {
    2 * (SRC_TAPS_UNITY as f64 / (2.0 * scale) - 1e-9).ceil() as usize
}

/// Closed-form SRC prototype `h(t)`, `t` in input samples (`design_src.prototype`, `Design.md §4.2`).
///
/// Kaiser-windowed sinc with cutoff `SRC_CUTOFF * scale` and window support `taps` input samples.
pub fn src_prototype(t: f64, scale: f64, taps: usize) -> f64 {
    src_prototype_with_norm(t, scale, taps, bessel_i0(SRC_KAISER_BETA))
}

/// `src_prototype` with the window normalisation `I0(SRC_KAISER_BETA)` precomputed (bank construction).
fn src_prototype_with_norm(t: f64, scale: f64, taps: usize, i0_beta: f64) -> f64 {
    let fc = SRC_CUTOFF * scale;
    let half = taps as f64 / 2.0;
    let r = 1.0 - (t / half) * (t / half);
    if r < 0.0 {
        return 0.0;
    }
    let window = bessel_i0(SRC_KAISER_BETA * r.sqrt()) / i0_beta;
    let x = 2.0 * fc * t;
    let sinc = if x == 0.0 {
        1.0
    } else {
        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
    };
    2.0 * fc * sinc * window
}

/// Appends one bank row for fractional position `phi` (`design_src.bank_row`), unity DC gain.
///
/// Stored reversed (oldest input first), so the convolution walks the delay line forwards.
fn push_bank_row(bank: &mut Vec<f64>, phi: f64, scale: f64, taps: usize) {
    let start = bank.len();
    let half = taps as f64 / 2.0;
    let i0_beta = bessel_i0(SRC_KAISER_BETA);
    for j in 0..taps {
        let i = taps - 1 - j;
        bank.push(src_prototype_with_norm(
            i as f64 - half + phi,
            scale,
            taps,
            i0_beta,
        ));
    }
    let sum: f64 = bank[start..].iter().sum();
    for c in &mut bank[start..] {
        *c /= sum;
    }
}

/// Accumulator lanes of the SRC dot products: independent add chains the compiler can vectorise.
const SRC_LANES: usize = 4;

#[inline]
fn lane_sum(acc: [f64; SRC_LANES]) -> f64 {
    (acc[0] + acc[1]) + (acc[2] + acc[3])
}

/// `sum(row[i] * w[i])` over `SRC_LANES` interleaved accumulators.
#[inline]
fn dot(row: &[f64], w: &[f64]) -> f64 {
    let mut acc = [0.0; SRC_LANES];
    let (rc, wc) = (row.chunks_exact(SRC_LANES), w.chunks_exact(SRC_LANES));
    let tail: f64 = rc
        .remainder()
        .iter()
        .zip(wc.remainder())
        .map(|(c, v)| c * v)
        .sum();
    for (c, v) in rc.zip(wc) {
        for l in 0..SRC_LANES {
            acc[l] += c[l] * v[l];
        }
    }
    lane_sum(acc) + tail
}

/// Four dot products of `w` with four rows in one pass over the delay line.
#[inline]
fn dot4_rows(rows: [&[f64]; 4], w: &[f64]) -> (f64, f64, f64, f64) {
    let (mut a0, mut a1, mut a2, mut a3) = (
        [0.0; SRC_LANES],
        [0.0; SRC_LANES],
        [0.0; SRC_LANES],
        [0.0; SRC_LANES],
    );
    let chunks = rows[0]
        .chunks_exact(SRC_LANES)
        .zip(rows[1].chunks_exact(SRC_LANES))
        .zip(rows[2].chunks_exact(SRC_LANES))
        .zip(rows[3].chunks_exact(SRC_LANES))
        .zip(w.chunks_exact(SRC_LANES));
    for ((((c0, c1), c2), c3), v) in chunks {
        for l in 0..SRC_LANES {
            a0[l] += c0[l] * v[l];
            a1[l] += c1[l] * v[l];
            a2[l] += c2[l] * v[l];
            a3[l] += c3[l] * v[l];
        }
    }
    let n = w.len() / SRC_LANES * SRC_LANES;
    let tail = |row: &[f64]| -> f64 { row[n..].iter().zip(&w[n..]).map(|(c, v)| c * v).sum() };
    (
        lane_sum(a0) + tail(rows[0]),
        lane_sum(a1) + tail(rows[1]),
        lane_sum(a2) + tail(rows[2]),
        lane_sum(a3) + tail(rows[3]),
    )
}

const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Largest rational phase count (output rate / gcd) a resampler accepts; bounds the bank size.
pub const SRC_MAX_RATIONAL_PHASES: usize = 1024;

/// Mode of operation for the polyphase resampler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolyphaseMode {
    /// Exact rational conversion: `phase_step / num_phases` input frames per output frame, reduced by the gcd
    /// (44.1k -> 48k: 147/160). Every output position is a bank row, no interpolation.
    Rational { phase_step: u64, num_phases: u64 },
    /// Continuously variable ASRC around `nominal_ratio` (input frames per output frame), positions cubic
    /// interpolated between `SRC_ASRC_PHASES` bank rows.
    Asrc { nominal_ratio: f64, drift_ppm: f64 },
}

/// Kaiser-windowed sinc polyphase resampler for cross-family conversions and ASRC drift tracking (`Design.md §4.2`).
///
/// The bank is built from the closed-form prototype (`src_prototype`) at construction. Its band edges are
/// relative to the lower of the two rates, so downsampling is anti-aliased. The delay line holds the newest
/// `taps` inputs; an output at fractional position `phi` lies at time `newest - taps/2 + phi`.
#[derive(Debug, Clone)]
pub struct PolyphaseResampler {
    mode: PolyphaseMode,
    /// Rows of `taps` coefficients (reversed). Rational: `num_phases` rows. ASRC: `SRC_ASRC_PHASES + 3`
    /// rows for phases `-1 ..= SRC_ASRC_PHASES + 1` (guard rows for the 4-point interpolation).
    bank: Vec<f64>,
    taps: usize,
    scale: f64,
    history: Vec<f64>,
    head: usize,
    /// Rational position numerator in `[0, num_phases)`.
    rational_acc: u64,
    /// ASRC fractional position in input samples.
    phase_acc: f64,
}

impl PolyphaseResampler {
    /// Creates an exact rational resampler (any pair of supported rates whose reduced ratio has at most
    /// `SRC_MAX_RATIONAL_PHASES` output phases, e.g. 44.1k <-> 48k at any tier, 96k -> 44.1k).
    pub fn new_rational(in_rate: PcmRateHz, out_rate: PcmRateHz) -> Result<Self, FirError> {
        let (in_hz, out_hz) = (in_rate.get() as u64, out_rate.get() as u64);
        let g = gcd(in_hz, out_hz);
        let (phase_step, num_phases) = (in_hz / g, out_hz / g);
        if num_phases as usize > SRC_MAX_RATIONAL_PHASES {
            return Err(FirError::UnsupportedRatio {
                in_rate_hz: in_rate.get(),
                out_rate_hz: out_rate.get(),
            });
        }
        let scale = src_band_limit_scale(in_hz as f64, out_hz as f64);
        let taps = src_taps_for_scale(scale);
        let mut bank = Vec::with_capacity(num_phases as usize * taps);
        for p in 0..num_phases {
            push_bank_row(&mut bank, p as f64 / num_phases as f64, scale, taps);
        }
        Ok(Self::with_bank(
            PolyphaseMode::Rational {
                phase_step,
                num_phases,
            },
            bank,
            taps,
            scale,
        ))
    }

    /// Creates a continuous ASRC resampler referenced to a nominal conversion ratio.
    pub fn new_asrc(in_rate: PcmRateHz, out_rate: PcmRateHz) -> Self {
        let nominal_ratio = in_rate.get() as f64 / out_rate.get() as f64;
        let scale = src_band_limit_scale(in_rate.get() as f64, out_rate.get() as f64);
        let taps = src_taps_for_scale(scale);
        let mut bank = Vec::with_capacity((SRC_ASRC_PHASES + 3) * taps);
        for r in -1..=(SRC_ASRC_PHASES as i64 + 1) {
            push_bank_row(&mut bank, r as f64 / SRC_ASRC_PHASES as f64, scale, taps);
        }
        Self::with_bank(
            PolyphaseMode::Asrc {
                nominal_ratio,
                drift_ppm: 0.0,
            },
            bank,
            taps,
            scale,
        )
    }

    fn with_bank(mode: PolyphaseMode, bank: Vec<f64>, taps: usize, scale: f64) -> Self {
        Self {
            mode,
            bank,
            taps,
            scale,
            history: vec![0.0; taps * 2],
            head: 0,
            rational_acc: 0,
            phase_acc: 0.0,
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

    /// Input frames consumed per output frame at the current setting (`in_rate / out_rate`, drift included).
    pub fn input_per_output_ratio(&self) -> f64 {
        match self.mode {
            PolyphaseMode::Rational {
                phase_step,
                num_phases,
            } => phase_step as f64 / num_phases as f64,
            PolyphaseMode::Asrc {
                nominal_ratio,
                drift_ppm,
            } => nominal_ratio * (1.0 + drift_ppm * 1e-6),
        }
    }

    /// Phase resolution of the bank: exact rational phases, or the ASRC rows that are interpolated between.
    pub fn table_phases(&self) -> usize {
        match self.mode {
            PolyphaseMode::Rational { num_phases, .. } => num_phases as usize,
            PolyphaseMode::Asrc { .. } => SRC_ASRC_PHASES,
        }
    }

    /// Taps per output sample.
    pub fn taps_per_phase(&self) -> usize {
        self.taps
    }

    /// Band-limit scale `min(1, out/in)` of the prototype.
    pub fn band_limit_scale(&self) -> f64 {
        self.scale
    }

    /// The bank as built (rows of `taps_per_phase()` coefficients, reversed: oldest input first). Rational:
    /// row `p` is phase `p / table_phases()`. ASRC: row `r` is phase `(r - 1) / table_phases()`.
    pub fn bank(&self) -> &[f64] {
        &self.bank
    }

    /// Resets delay line and phase accumulators to zero.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.head = 0;
        self.rational_acc = 0;
        self.phase_acc = 0.0;
    }

    #[inline]
    fn push_input(&mut self, sample: f64) {
        let taps = self.taps;
        self.history[self.head] = sample;
        self.history[self.head + taps] = sample;
        self.head += 1;
        if self.head == taps {
            self.head = 0;
        }
    }

    /// Delay line, oldest first.
    #[inline]
    fn window(&self) -> &[f64] {
        &self.history[self.head..self.head + self.taps]
    }

    #[inline]
    fn row(&self, r: usize) -> &[f64] {
        &self.bank[r * self.taps..(r + 1) * self.taps]
    }

    /// Cubic (4-point Lagrange) interpolation between the bank rows around ASRC position `phi` in `[0, 1)`.
    #[inline]
    fn asrc_output(&self, phi: f64) -> f64 {
        let x = phi * SRC_ASRC_PHASES as f64;
        let p = (x.floor() as usize).min(SRC_ASRC_PHASES - 1);
        let mu = x - p as f64;
        // Row index r + 1 holds phase r; nodes -1, 0, 1, 2 around p are rows p .. p + 3.
        let (r0, r1, r2, r3) = (
            self.row(p),
            self.row(p + 1),
            self.row(p + 2),
            self.row(p + 3),
        );
        let w = self.window();
        let (s0, s1, s2, s3) = dot4_rows([r0, r1, r2, r3], w);
        let (m1, m2) = (mu - 1.0, mu - 2.0);
        let mp = mu + 1.0;
        let c0 = -mu * m1 * m2 / 6.0;
        let c1 = mp * m1 * m2 / 2.0;
        let c2 = -mp * mu * m2 / 2.0;
        let c3 = mp * mu * m1 / 6.0;
        c0 * s0 + c1 * s1 + c2 * s2 + c3 * s3
    }

    /// Processes an input buffer through the resampler.
    ///
    /// Returns `(consumed_input_frames, produced_output_frames)`.
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) -> (usize, usize) {
        let mut in_idx = 0;
        let mut out_idx = 0;

        match self.mode {
            PolyphaseMode::Rational {
                phase_step,
                num_phases,
            } => {
                while in_idx < input.len() && out_idx < output.len() {
                    while self.rational_acc < num_phases && out_idx < output.len() {
                        output[out_idx] = dot(self.row(self.rational_acc as usize), self.window());
                        out_idx += 1;
                        self.rational_acc += phase_step;
                    }
                    while self.rational_acc >= num_phases && in_idx < input.len() {
                        self.push_input(input[in_idx]);
                        in_idx += 1;
                        self.rational_acc -= num_phases;
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
                        output[out_idx] = self.asrc_output(self.phase_acc);
                        out_idx += 1;
                        self.phase_acc += ratio;
                    }
                    while self.phase_acc >= 1.0 && in_idx < input.len() {
                        self.push_input(input[in_idx]);
                        in_idx += 1;
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
    fn test_short_halfband_kernels_bit_exact() {
        for preset in [
            FilterPreset::LowLatency,
            FilterPreset::Balanced,
            FilterPreset::Reference,
        ] {
            let taps = get_halfband_stage1_taps(preset);
            let mut specialized = HalfBandStage::new(taps);
            let mut generic = specialized.clone();
            let input: Vec<f64> = (0..257)
                .map(|i| ((i * 7919 % 65521) as f64 / 65521.0 - 0.5) * 0.9)
                .collect();
            // Irregular tile boundaries exercise history wraparound as well as
            // all three fixed tap counts, including empty and oversized output.
            for tile in std::iter::once(&input[..0]).chain(input.chunks(13)) {
                let mut actual = vec![-1.0; tile.len() * 2 + 3];
                let mut expected = actual.clone();
                specialized.process_tile(tile, &mut actual).unwrap();
                generic
                    .process_tile_kernel::<0>(tile, &mut expected)
                    .unwrap();
                for (a, b) in actual.iter().zip(&expected) {
                    assert_eq!(a.to_bits(), b.to_bits(), "preset {preset:?}");
                }
            }
            assert_eq!(specialized.head, generic.head);
            assert_eq!(specialized.history, generic.history);
            assert!(matches!(
                specialized.process_tile(&[1.0], &mut [0.0]),
                Err(FirError::OutputBufferTooSmall { .. })
            ));
        }
    }

    #[test]
    fn test_cascade_scratch_sizes_and_tile_boundaries() {
        let input: Vec<f64> = (0..37).map(|i| (i as f64 * 0.17).sin() * 0.4).collect();
        for stages in 0..=9 {
            let mut tiled = HalfBandCascade::new(
                FilterPreset::Balanced,
                FilterPhase::Linear,
                Apodization::Off,
                stages,
                13,
            );
            let expected_a = (0..stages.saturating_sub(1))
                .filter(|i| i % 2 == 0)
                .map(|i| 13 * (1 << (i + 1)))
                .max()
                .unwrap_or(0);
            let expected_b = (0..stages.saturating_sub(1))
                .filter(|i| i % 2 == 1)
                .map(|i| 13 * (1 << (i + 1)))
                .max()
                .unwrap_or(0);
            assert_eq!(tiled.scratch_a.len(), expected_a);
            assert_eq!(tiled.scratch_b.len(), expected_b);

            let mut whole = HalfBandCascade::new(
                FilterPreset::Balanced,
                FilterPhase::Linear,
                Apodization::Off,
                stages,
                input.len(),
            );
            let multiplier = tiled.rate_multiplier();
            let mut actual = vec![0.0; input.len() * multiplier];
            let mut expected = actual.clone();
            whole.process_tile(&input, &mut expected).unwrap();
            assert_eq!(tiled.process_tile(&[], &mut []).unwrap(), 0);
            for (tile, out) in input.chunks(13).zip(actual.chunks_mut(13 * multiplier)) {
                assert_eq!(tiled.process_tile(tile, out).unwrap(), out.len());
            }
            for (a, b) in actual.iter().zip(&expected) {
                assert_eq!(a.to_bits(), b.to_bits(), "stages {stages}");
            }
            tiled.reset();
            assert!(tiled.scratch_a.iter().all(|&s| s == 0.0));
            assert!(tiled.scratch_b.iter().all(|&s| s == 0.0));
        }
    }

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
        let mut cascade = HalfBandCascade::new(
            FilterPreset::Balanced,
            FilterPhase::Linear,
            Apodization::Off,
            8,
            32,
        );
        assert_eq!(cascade.rate_multiplier(), 256);
        assert_eq!(cascade.num_stages(), 8);

        // 16 input frames -> 16 * 256 = 4096 samples
        let input = [0.5; 16];
        let mut output = [0.0; 4096];
        let written = cascade.process_tile(&input, &mut output).unwrap();
        assert_eq!(written, 4096);
    }

    #[test]
    fn test_halfband_cascade_512x_dsd512() {
        let mut cascade = HalfBandCascade::new(
            FilterPreset::Balanced,
            FilterPhase::Linear,
            Apodization::Off,
            9,
            32,
        );
        assert_eq!(cascade.rate_multiplier(), 512);
        assert_eq!(cascade.num_stages(), 9);

        // 16 input frames -> 16 * 512 = 8192 samples
        let input = [0.5; 16];
        let mut output = [0.0; 8192];
        let written = cascade.process_tile(&input, &mut output).unwrap();
        assert_eq!(written, 8192);
    }

    #[test]
    fn test_polyphase_stage_matches_zero_stuffed_convolution() {
        let taps = match get_halfband_stage0_filter(
            FilterPreset::Balanced,
            FilterPhase::Linear,
            Apodization::Gentle,
        ) {
            HalfBandTaps::General(taps) => taps,
            HalfBandTaps::Symmetric(_) => panic!("gentle apodization must use general taps"),
        };
        let mut stage = PolyphaseInterpolationStage::new(taps);

        let input: [f64; 16] = [
            0.0, 1.0, -0.5, 0.25, 0.75, -0.125, 0.0, 0.5, -0.25, 0.125, 0.0, -0.75, 0.33, 0.66,
            -0.33, 0.1,
        ];
        let mut output = [0.0f64; 32];
        stage.process_tile(&input, &mut output).unwrap();

        for n in 0..output.len() {
            let mut expected = 0.0;
            for (m, &sample) in input.iter().enumerate() {
                if n >= 2 * m {
                    expected += sample * taps[n - 2 * m];
                }
            }
            expected *= 2.0;
            assert!(
                (output[n] - expected).abs() < 1e-12,
                "sample {n}: {} != {}",
                output[n],
                expected
            );
        }
    }

    #[test]
    fn test_phase_apodization_modes_dc_unity() {
        let modes = [
            (FilterPhase::Linear, Apodization::Off),
            (FilterPhase::Linear, Apodization::Gentle),
            (FilterPhase::Minimum, Apodization::Off),
            (FilterPhase::Minimum, Apodization::Gentle),
        ];

        for preset in [
            FilterPreset::Balanced,
            FilterPreset::Reference,
            FilterPreset::LowLatency,
        ] {
            for (phase, apodization) in modes {
                let mut cascade = HalfBandCascade::new(preset, phase, apodization, 3, 256);
                let input = [1.0f64; 256];
                let mut output = [0.0f64; 2048];
                cascade.process_tile(&input, &mut output).unwrap();

                for (idx, &sample) in output[900..].iter().enumerate() {
                    assert!(
                        (sample - 1.0).abs() < 1e-6,
                        "{preset:?}/{phase:?}/{apodization:?}: sample {} = {sample}",
                        idx + 900
                    );
                }
            }
        }
    }

    #[test]
    fn test_minimum_phase_is_causal_and_front_loaded() {
        for preset in [
            FilterPreset::Balanced,
            FilterPreset::Reference,
            FilterPreset::LowLatency,
        ] {
            for apodization in [Apodization::Off, Apodization::Gentle] {
                let mut cascade =
                    HalfBandCascade::new(preset, FilterPhase::Minimum, apodization, 1, 512);
                let mut input = [0.0f64; 512];
                let mut output = [0.0f64; 1024];
                input[128] = 1.0;
                cascade.process_tile(&input, &mut output).unwrap();

                // Strict causality: no output before the input impulse arrives.
                for (idx, &sample) in output[..256].iter().enumerate() {
                    assert_eq!(
                        sample, 0.0,
                        "{preset:?}/{apodization:?}: pre-impulse sample {idx} is non-zero"
                    );
                }

                let peak_idx = (256..output.len())
                    .max_by(|&a, &b| output[a].abs().total_cmp(&output[b].abs()))
                    .unwrap();
                assert!(
                    peak_idx - 256 < 16,
                    "{preset:?}/{apodization:?}: minimum-phase impulse peak at offset {}",
                    peak_idx - 256
                );

                let pre_energy: f64 = output[256..peak_idx].iter().map(|s| s.abs()).sum();
                let post_energy: f64 = output[peak_idx + 1..].iter().map(|s| s.abs()).sum();
                assert!(
                    pre_energy < 0.5 * post_energy,
                    "{preset:?}/{apodization:?}: energy is not front-loaded (pre={pre_energy}, post={post_energy})"
                );
            }
        }
    }

    #[test]
    fn test_linear_phase_remains_symmetric() {
        let mut cascade = HalfBandCascade::new(
            FilterPreset::Balanced,
            FilterPhase::Linear,
            Apodization::Off,
            1,
            512,
        );
        let mut input = [0.0f64; 512];
        let mut output = [0.0f64; 1024];
        input[128] = 1.0;
        cascade.process_tile(&input, &mut output).unwrap();

        let peak_idx = (256..output.len())
            .max_by(|&a, &b| output[a].abs().total_cmp(&output[b].abs()))
            .unwrap();
        // The linear-phase prototype is symmetric around its impulse peak.
        for offset in 1..=40 {
            let pre = output[peak_idx - offset];
            let post = output[peak_idx + offset];
            assert!(
                (pre - post).abs() < 1e-12,
                "asymmetry at offset {offset}: {pre} != {post}"
            );
        }
    }

    fn normalized_tail_energy(taps: &[f64], offset: usize) -> f64 {
        let peak_idx = taps
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .map(|(idx, _)| idx)
            .unwrap();
        let peak = taps[peak_idx].abs();
        taps[peak_idx + offset..]
            .iter()
            .map(|tap| tap.abs())
            .fold(0.0f64, f64::max)
            / peak
    }

    #[test]
    fn test_gentle_apodization_suppresses_far_tail() {
        for preset in [
            FilterPreset::Balanced,
            FilterPreset::Reference,
            FilterPreset::LowLatency,
        ] {
            for phase in [FilterPhase::Linear, FilterPhase::Minimum] {
                let off = match get_halfband_stage0_filter(preset, phase, Apodization::Off) {
                    HalfBandTaps::General(taps) => taps,
                    HalfBandTaps::Symmetric(taps) => taps,
                };
                let gentle = match get_halfband_stage0_filter(preset, phase, Apodization::Gentle) {
                    HalfBandTaps::General(taps) => taps,
                    HalfBandTaps::Symmetric(_) => panic!("gentle must use general taps"),
                };

                // Compare the effective full impulse responses. For the symmetric
                // half-band prototype the odd taps are symmetric around the centre.
                let off_tail = if matches!(
                    get_halfband_stage0_filter(preset, phase, Apodization::Off),
                    HalfBandTaps::Symmetric(_)
                ) {
                    let center = (off.len() * 2) - 1;
                    let mut full = vec![0.0; center + 2 * off.len()];
                    full[center] = 1.0;
                    for (idx, &tap) in off.iter().enumerate() {
                        let delta = 2 * idx + 1;
                        full[center - delta] = 2.0 * tap;
                        full[center + delta] = 2.0 * tap;
                    }
                    normalized_tail_energy(&full, 64)
                } else {
                    normalized_tail_energy(off, 64)
                };
                let gentle_tail = normalized_tail_energy(gentle, 64);

                assert!(
                    gentle_tail <= off_tail,
                    "{preset:?}/{phase:?}: gentle tail {gentle_tail} exceeds off tail {off_tail}"
                );
                if phase == FilterPhase::Linear {
                    assert!(
                        gentle_tail < 0.5 * off_tail,
                        "{preset:?}/{phase:?}: gentle tail {gentle_tail} not sufficiently below off tail {off_tail}"
                    );
                }
            }
        }
    }

    #[test]
    fn test_minimum_phase_group_delay_is_lower() {
        for preset in [
            FilterPreset::Balanced,
            FilterPreset::Reference,
            FilterPreset::LowLatency,
        ] {
            let linear =
                HalfBandCascade::new(preset, FilterPhase::Linear, Apodization::Off, 1, 128);
            let minimum =
                HalfBandCascade::new(preset, FilterPhase::Minimum, Apodization::Off, 1, 128);
            assert!(
                minimum.total_group_delay_output_samples()
                    < linear.total_group_delay_output_samples(),
                "{preset:?}: minimum-phase delay {} is not below linear delay {}",
                minimum.total_group_delay_output_samples(),
                linear.total_group_delay_output_samples()
            );
        }
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
    fn test_asrc_interpolation_matches_exact_rational_phases() {
        // Same nominal ratio: the rational path evaluates exact phases, the ASRC interpolates between 256
        // rows. They must agree to the interpolation error (far below the 140 dB prototype stopband).
        let (fin, fout) = (PcmRateHz::PCM_44100, PcmRateHz::PCM_48000);
        let mut rational = PolyphaseResampler::new_rational(fin, fout).unwrap();
        let mut asrc = PolyphaseResampler::new_asrc(fin, fout);
        assert_eq!(rational.taps_per_phase(), asrc.taps_per_phase());
        let input: Vec<f64> = (0..4096)
            .map(|n| 0.5 * (2.0 * std::f64::consts::PI * 15_000.0 * n as f64 / 44_100.0).sin())
            .collect();
        let mut out_r = vec![0.0; 5000];
        let mut out_a = vec![0.0; 5000];
        let (_, nr) = rational.process(&input, &mut out_r);
        let (_, na) = asrc.process(&input, &mut out_a);
        let n = nr.min(na);
        assert!(n > 4000);
        let max_err = out_r[..n]
            .iter()
            .zip(&out_a[..n])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(
            max_err < 1e-8,
            "ASRC deviates from exact phases by {max_err}"
        );
    }

    #[test]
    fn test_rational_downsampling_is_band_limited_to_output_rate() {
        // 96k -> 44.1k: scale 44.1/96, taps grow so the transition band stays 20-24.1 kHz at the output.
        let src =
            PolyphaseResampler::new_rational(PcmRateHz::new(96_000).unwrap(), PcmRateHz::PCM_44100)
                .unwrap();
        assert_eq!(src.table_phases(), 147);
        assert!((src.band_limit_scale() - 44_100.0 / 96_000.0).abs() < 1e-15);
        assert_eq!(
            src.taps_per_phase(),
            src_taps_for_scale(44_100.0 / 96_000.0)
        );
        assert!(src.taps_per_phase() > 2 * SRC_TAPS_UNITY);
        for row in src.bank().chunks(src.taps_per_phase()) {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-12);
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
