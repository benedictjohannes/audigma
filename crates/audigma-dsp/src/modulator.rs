//! 5th-Order and 7th-Order CRFB Delta-Sigma Modulators and TPDF dither loop (`Design.md §4.3–§4.5`).
//!
//! Implements:
//! - `GenericCrfbModulator<C>`: Pure-Rust monomorphized Cascade of Resonators with Feedback (CRFB)
//!   modulator utilizing verified Schreier NTF coefficients (`tables.rs`).
//! - `CrfbModulator5` (aliased as `CrfbModulator`): 5th-order CRFB modulator with 2 resonator feedback couplings.
//! - `CrfbModulator7`: 7th-order CRFB modulator with 3 resonator feedback couplings.
//! - `Modulator`: Runtime-selectable enum dispatching at tile boundaries with zero per-sample overhead.
//! - `TpdfDither`: Fast zero-allocation PRNG generating triangular probability density dither.
//! - Overload protection (`OverloadPolicy::MuteRecover` and `OverloadPolicy::Error`),
//!   resetting integrators and substituting balanced DSD idle (`0x69`) upon instability.

use crate::config::{DitherType, OverloadPolicy};
use crate::presets::{DSD_IDLE_BYTE, DSD_IDLE_U32_BE};
use crate::rates::DsdBitRateHz;
use crate::tables::{CrfbCoefficients5, CrfbCoefficients7};
use smallvec::SmallVec;
use thiserror::Error;

/// Maximum integrator state magnitude before triggering overload protection.
pub const STABILITY_THRESHOLD: f64 = 50.0;

/// Modulator error conditions.
#[derive(Debug, Error, PartialEq)]
pub enum ModulatorError {
    #[error("Output buffer too small: required {required}, provided {provided}")]
    OutputBufferTooSmall { required: usize, provided: usize },

    #[error("Input length {len} is not a multiple of {alignment}")]
    UnalignedInputLength { len: usize, alignment: usize },

    #[error("Modulator integrator overload detected: state={state:?}, input={input}")]
    Overload {
        state: SmallVec<[f64; 7]>,
        input: f64,
    },

    #[error("Unsupported modulator order {0}: only 5th and 7th order are supported")]
    UnsupportedOrder(u8),

    #[error("No verified CRFB coefficients found for DSD bit rate {bit_rate} and order {order}")]
    UnsupportedRate { bit_rate: DsdBitRateHz, order: u8 },
}

/// Lightweight, deterministic, zero-allocation PRNG for TPDF dither injection.
///
/// Uses an Xorshift64* generator with triangular PDF folding in `[-amplitude, amplitude]`.
#[derive(Debug, Clone)]
pub struct TpdfDither {
    state: u64,
    amplitude: f64,
}

impl TpdfDither {
    /// Creates a new TPDF dither generator with a default seed and peak amplitude.
    pub const fn new(seed: u64, amplitude: f64) -> Self {
        let state = if seed == 0 {
            0x853c_49e6_748f_ea9b
        } else {
            seed
        };
        Self { state, amplitude }
    }

    /// Advances the PRNG and returns a single triangular dither sample in `[-amplitude, amplitude]`.
    #[inline]
    pub fn next_sample(&mut self) -> f64 {
        // Xorshift64*
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let r1 = (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32;

        let mut y = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        y ^= y >> 12;
        y ^= y << 25;
        y ^= y >> 27;
        let r2 = (y.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32;

        // Two uniform random variables in [-0.5, 0.5]
        const U32_INV: f64 = 1.0 / 4_294_967_296.0;
        let u1 = (r1 as f64) * U32_INV - 0.5;
        let u2 = (r2 as f64) * U32_INV - 0.5;

        // Sum of two uniforms gives triangular PDF
        (u1 + u2) * self.amplitude
    }
}

impl Default for TpdfDither {
    fn default() -> Self {
        // Default amplitude 0.005 as validated in tools/design/design_crfb.py
        Self::new(12345, 0.005)
    }
}

/// Static core definition for CRFB state-space updates.
pub trait CrfbCore: std::fmt::Debug + Clone + Send + 'static {
    /// Coefficient structure for this order.
    type Coeffs: std::fmt::Debug + Copy + Clone + Send + 'static;
    /// Fixed-size integrator state array.
    type State: std::fmt::Debug + Copy + Clone + Send + AsRef<[f64]> + AsMut<[f64]> + 'static;

    /// Filter order.
    const ORDER: usize;

    /// Initializes all integrator states to zero.
    fn zero_state() -> Self::State;

    /// Extracts the unquantized input to the comparator from the state vector.
    fn quantizer_input(state: &Self::State) -> f64;

    /// Computes the unrolled state-space update and checks for numerical instability.
    ///
    /// Returns the new state and `true` if an overload/NaN condition occurred.
    fn step(coeffs: &Self::Coeffs, state: &Self::State, u: f64, v: f64) -> (Self::State, bool);
}

/// 5th-order CRFB state-space core (2 local resonator couplings: g1, g2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrfbCore5;

impl CrfbCore for CrfbCore5 {
    type Coeffs = CrfbCoefficients5;
    type State = [f64; 5];
    const ORDER: usize = 5;

    #[inline(always)]
    fn zero_state() -> Self::State {
        [0.0; 5]
    }

    #[inline(always)]
    fn quantizer_input(state: &Self::State) -> f64 {
        state[4]
    }

    #[inline(always)]
    fn step(c: &Self::Coeffs, s: &Self::State, u: f64, v: f64) -> (Self::State, bool) {
        let x1 = s[0] - c.g1 * s[1] + c.b1 * u - c.a[0] * v;
        let x2 = s[1] + x1 - c.a[1] * v;
        let x3 = s[2] - c.g2 * s[3] + x2 - c.a[2] * v;
        let x4 = s[3] + x3 - c.a[3] * v;
        let x5 = s[4] + x4 - c.a[4] * v;

        let overloaded = x1.abs() > STABILITY_THRESHOLD
            || x2.abs() > STABILITY_THRESHOLD
            || x3.abs() > STABILITY_THRESHOLD
            || x4.abs() > STABILITY_THRESHOLD
            || x5.abs() > STABILITY_THRESHOLD
            || x1.is_nan();

        ([x1, x2, x3, x4, x5], overloaded)
    }
}

/// 7th-order CRFB state-space core (3 local resonator couplings: g1, g2, g3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrfbCore7;

impl CrfbCore for CrfbCore7 {
    type Coeffs = CrfbCoefficients7;
    type State = [f64; 7];
    const ORDER: usize = 7;

    #[inline(always)]
    fn zero_state() -> Self::State {
        [0.0; 7]
    }

    #[inline(always)]
    fn quantizer_input(state: &Self::State) -> f64 {
        state[6]
    }

    #[inline(always)]
    fn step(c: &Self::Coeffs, s: &Self::State, u: f64, v: f64) -> (Self::State, bool) {
        let x1 = s[0] - c.g1 * s[1] + c.b1 * u - c.a[0] * v;
        let x2 = s[1] + x1 - c.a[1] * v;
        let x3 = s[2] - c.g2 * s[3] + x2 - c.a[2] * v;
        let x4 = s[3] + x3 - c.a[3] * v;
        let x5 = s[4] - c.g3 * s[5] + x4 - c.a[4] * v;
        let x6 = s[5] + x5 - c.a[5] * v;
        let x7 = s[6] + x6 - c.a[6] * v;

        let overloaded = x1.abs() > STABILITY_THRESHOLD
            || x2.abs() > STABILITY_THRESHOLD
            || x3.abs() > STABILITY_THRESHOLD
            || x4.abs() > STABILITY_THRESHOLD
            || x5.abs() > STABILITY_THRESHOLD
            || x6.abs() > STABILITY_THRESHOLD
            || x7.abs() > STABILITY_THRESHOLD
            || x1.is_nan();

        ([x1, x2, x3, x4, x5, x6, x7], overloaded)
    }
}

/// Generic, monomorphized Cascade of Resonators with Feedback (CRFB) 1-bit Delta-Sigma Modulator.
#[derive(Debug, Clone)]
pub struct GenericCrfbModulator<C: CrfbCore> {
    coeffs: C::Coeffs,
    state: C::State,
    dither: Option<TpdfDither>,
    overload_policy: OverloadPolicy,
    overload_count: u64,
}

impl<C: CrfbCore> GenericCrfbModulator<C> {
    /// Maximum integrator state magnitude before triggering overload protection.
    pub const STABILITY_THRESHOLD: f64 = STABILITY_THRESHOLD;

    /// Creates a new CRFB modulator with the given coefficients and configuration.
    pub fn new(
        coeffs: C::Coeffs,
        dither_type: DitherType,
        overload_policy: OverloadPolicy,
    ) -> Self {
        let dither = match dither_type {
            DitherType::None => None,
            DitherType::Tpdf => Some(TpdfDither::default()),
        };
        Self {
            coeffs,
            state: C::zero_state(),
            dither,
            overload_policy,
            overload_count: 0,
        }
    }

    /// Resets all internal integrator states to zero without resetting statistics.
    pub fn reset(&mut self) {
        self.state = C::zero_state();
    }

    /// Returns the current fixed-size integrator state vector.
    #[inline]
    pub const fn state(&self) -> C::State {
        self.state
    }

    /// Returns the current integrator state as a slice.
    #[inline]
    pub fn state_slice(&self) -> &[f64] {
        self.state.as_ref()
    }

    /// Returns the filter order.
    #[inline]
    pub const fn order(&self) -> usize {
        C::ORDER
    }

    /// Returns the cumulative number of overload events encountered.
    #[inline]
    pub const fn overload_count(&self) -> u64 {
        self.overload_count
    }

    /// Sets the dither PRNG seed for deterministic testing and reproducible conversion.
    pub fn set_dither_seed(&mut self, seed: u64) {
        if let Some(ref mut dither) = self.dither {
            dither.state = if seed == 0 {
                0x853c_49e6_748f_ea9b
            } else {
                seed
            };
        }
    }

    /// Steps a single floating-point PCM sample through the 1-bit modulator loop.
    ///
    /// Returns the 1-bit quantization decision (`1` for `+1.0`, `0` for `-1.0`).
    #[inline]
    pub fn step_sample(&mut self, u: f64) -> Result<u8, ModulatorError> {
        let dither_val = match self.dither {
            Some(ref mut d) => d.next_sample(),
            None => 0.0,
        };

        // Quantizer comparator input
        let y = C::quantizer_input(&self.state) + dither_val;
        let (v, bit) = if y >= 0.0 { (1.0, 1u8) } else { (-1.0, 0u8) };

        // Discrete integrator state update
        let (new_state, overloaded) = C::step(&self.coeffs, &self.state, u, v);

        if overloaded {
            self.overload_count += 1;
            match self.overload_policy {
                OverloadPolicy::MuteRecover => {
                    self.state = C::zero_state();
                    return Ok(bit);
                }
                OverloadPolicy::Error => {
                    let mut state_vec = SmallVec::new();
                    state_vec.extend_from_slice(new_state.as_ref());
                    return Err(ModulatorError::Overload {
                        state: state_vec,
                        input: u,
                    });
                }
            }
        }

        self.state = new_state;
        Ok(bit)
    }

    /// Modulates an input PCM tile into raw 1-bit DSD samples (`0` or `1`).
    pub fn process_tile_bits(
        &mut self,
        input: &[f64],
        out_bits: &mut [u8],
    ) -> Result<(), ModulatorError> {
        if out_bits.len() < input.len() {
            return Err(ModulatorError::OutputBufferTooSmall {
                required: input.len(),
                provided: out_bits.len(),
            });
        }

        for (i, &sample) in input.iter().enumerate() {
            out_bits[i] = self.step_sample(sample)?;
        }

        Ok(())
    }

    /// Modulates an input PCM tile into chronological MSB-first DSD bytes (`8 bits/byte`).
    ///
    /// Bit 7 of each byte is the oldest sample and bit 0 is the newest.
    pub fn process_tile_bytes(
        &mut self,
        input: &[f64],
        out_bytes: &mut [u8],
    ) -> Result<(), ModulatorError> {
        if input.len() % 8 != 0 {
            return Err(ModulatorError::UnalignedInputLength {
                len: input.len(),
                alignment: 8,
            });
        }
        let required_bytes = input.len() / 8;
        if out_bytes.len() < required_bytes {
            return Err(ModulatorError::OutputBufferTooSmall {
                required: required_bytes,
                provided: out_bytes.len(),
            });
        }

        let initial_overloads = self.overload_count;
        for (byte_idx, out_byte) in out_bytes[..required_bytes].iter_mut().enumerate() {
            let in_off = byte_idx * 8;
            let mut byte = 0u8;
            for bit_pos in (0..8).rev() {
                let bit = self.step_sample(input[in_off + (7 - bit_pos)])?;
                byte |= (bit & 1) << bit_pos;
            }

            // If an overload occurred during this byte and MuteRecover is active, emit DSD idle byte
            if self.overload_count > initial_overloads
                && self.overload_policy == OverloadPolicy::MuteRecover
            {
                *out_byte = DSD_IDLE_BYTE;
            } else {
                *out_byte = byte;
            }
        }

        Ok(())
    }

    /// Modulates an input PCM tile into 32-bit Native `DSD_U32_BE` container words (`32 bits/word`).
    pub fn process_tile_u32(
        &mut self,
        input: &[f64],
        out_words: &mut [u32],
    ) -> Result<(), ModulatorError> {
        if input.len() % 32 != 0 {
            return Err(ModulatorError::UnalignedInputLength {
                len: input.len(),
                alignment: 32,
            });
        }
        let required_words = input.len() / 32;
        if out_words.len() < required_words {
            return Err(ModulatorError::OutputBufferTooSmall {
                required: required_words,
                provided: out_words.len(),
            });
        }

        let initial_overloads = self.overload_count;
        for (word_idx, out_word) in out_words[..required_words].iter_mut().enumerate() {
            let in_off = word_idx * 32;
            let mut bytes = [0u8; 4];
            for (b, byte_slot) in bytes.iter_mut().enumerate() {
                let b_off = in_off + b * 8;
                let mut byte = 0u8;
                for bit_pos in (0..8).rev() {
                    let bit = self.step_sample(input[b_off + (7 - bit_pos)])?;
                    byte |= (bit & 1) << bit_pos;
                }
                *byte_slot = byte;
            }

            if self.overload_count > initial_overloads
                && self.overload_policy == OverloadPolicy::MuteRecover
            {
                *out_word = DSD_IDLE_U32_BE;
            } else {
                *out_word = u32::from_be_bytes(bytes);
            }
        }

        Ok(())
    }
}

/// 5th-order CRFB Delta-Sigma Modulator.
pub type CrfbModulator5 = GenericCrfbModulator<CrfbCore5>;

/// Backward-compatible alias for 5th-order CRFB Delta-Sigma Modulator.
pub type CrfbModulator = CrfbModulator5;

/// 7th-order CRFB Delta-Sigma Modulator.
pub type CrfbModulator7 = GenericCrfbModulator<CrfbCore7>;

/// Unified Delta-Sigma Modulator enum supporting 5th-order and 7th-order loops.
///
/// Dispatches at tile boundaries with zero per-sample dynamic branch or vtable overhead.
#[derive(Debug, Clone)]
pub enum Modulator {
    Order5(CrfbModulator5),
    Order7(CrfbModulator7),
}

impl Modulator {
    /// Creates a new modulator for the given order, target bit rate, dither, and overload policy.
    pub fn new(
        order: u8,
        bit_rate: DsdBitRateHz,
        dither_type: DitherType,
        overload_policy: OverloadPolicy,
    ) -> Result<Self, ModulatorError> {
        match order {
            5 => {
                let coeffs = crate::tables::get_crfb5_coefficients(bit_rate)
                    .ok_or(ModulatorError::UnsupportedRate { bit_rate, order: 5 })?;
                Ok(Self::Order5(CrfbModulator5::new(
                    *coeffs,
                    dither_type,
                    overload_policy,
                )))
            }
            7 => {
                let coeffs = crate::tables::get_crfb7_coefficients(bit_rate)
                    .ok_or(ModulatorError::UnsupportedRate { bit_rate, order: 7 })?;
                Ok(Self::Order7(CrfbModulator7::new(
                    *coeffs,
                    dither_type,
                    overload_policy,
                )))
            }
            other => Err(ModulatorError::UnsupportedOrder(other)),
        }
    }

    /// Resets all internal integrator states to zero without resetting statistics.
    #[inline]
    pub fn reset(&mut self) {
        match self {
            Self::Order5(m) => m.reset(),
            Self::Order7(m) => m.reset(),
        }
    }

    /// Returns the modulator filter order (5 or 7).
    #[inline]
    pub const fn order(&self) -> u8 {
        match self {
            Self::Order5(_) => 5,
            Self::Order7(_) => 7,
        }
    }

    /// Returns the current integrator state as a slice.
    #[inline]
    pub fn state_slice(&self) -> &[f64] {
        match self {
            Self::Order5(m) => m.state_slice(),
            Self::Order7(m) => m.state_slice(),
        }
    }

    /// Returns the cumulative number of overload events encountered.
    #[inline]
    pub const fn overload_count(&self) -> u64 {
        match self {
            Self::Order5(m) => m.overload_count(),
            Self::Order7(m) => m.overload_count(),
        }
    }

    /// Sets the dither PRNG seed for deterministic testing and reproducible conversion.
    #[inline]
    pub fn set_dither_seed(&mut self, seed: u64) {
        match self {
            Self::Order5(m) => m.set_dither_seed(seed),
            Self::Order7(m) => m.set_dither_seed(seed),
        }
    }

    /// Steps a single floating-point PCM sample through the 1-bit modulator loop.
    #[inline]
    pub fn step_sample(&mut self, u: f64) -> Result<u8, ModulatorError> {
        match self {
            Self::Order5(m) => m.step_sample(u),
            Self::Order7(m) => m.step_sample(u),
        }
    }

    /// Modulates an input PCM tile into raw 1-bit DSD samples (`0` or `1`).
    #[inline]
    pub fn process_tile_bits(
        &mut self,
        input: &[f64],
        out_bits: &mut [u8],
    ) -> Result<(), ModulatorError> {
        match self {
            Self::Order5(m) => m.process_tile_bits(input, out_bits),
            Self::Order7(m) => m.process_tile_bits(input, out_bits),
        }
    }

    /// Modulates an input PCM tile into chronological MSB-first DSD bytes (`8 bits/byte`).
    #[inline]
    pub fn process_tile_bytes(
        &mut self,
        input: &[f64],
        out_bytes: &mut [u8],
    ) -> Result<(), ModulatorError> {
        match self {
            Self::Order5(m) => m.process_tile_bytes(input, out_bytes),
            Self::Order7(m) => m.process_tile_bytes(input, out_bytes),
        }
    }

    /// Modulates an input PCM tile into 32-bit Native `DSD_U32_BE` container words (`32 bits/word`).
    #[inline]
    pub fn process_tile_u32(
        &mut self,
        input: &[f64],
        out_words: &mut [u32],
    ) -> Result<(), ModulatorError> {
        match self {
            Self::Order5(m) => m.process_tile_u32(input, out_words),
            Self::Order7(m) => m.process_tile_u32(input, out_words),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rates::DsdBitRateHz;
    use crate::tables::{get_crfb_coefficients, get_crfb7_coefficients};

    #[test]
    fn test_modulator_silence_stability_order5() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();
        let mut mod_loop =
            CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        let silence = [0.0; 8000];
        let mut out_bytes = [0u8; 1000];
        mod_loop
            .process_tile_bytes(&silence, &mut out_bytes)
            .unwrap();

        assert_eq!(mod_loop.overload_count(), 0);
        for &s in &mod_loop.state() {
            assert!(
                s.abs() < STABILITY_THRESHOLD,
                "State {s} exploded on silence"
            );
        }

        let total_ones: u32 = out_bytes.iter().map(|b| b.count_ones()).sum();
        let ratio = (total_ones as f64) / 8000.0;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "Ones ratio {ratio} deviates significantly from 50% on silence"
        );
    }

    #[test]
    fn test_modulator_silence_stability_order7() {
        let coeffs = *get_crfb7_coefficients(DsdBitRateHz::DSD128).unwrap();
        let mut mod_loop =
            CrfbModulator7::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        let silence = [0.0; 8000];
        let mut out_bytes = [0u8; 1000];
        mod_loop
            .process_tile_bytes(&silence, &mut out_bytes)
            .unwrap();

        assert_eq!(mod_loop.overload_count(), 0);
        assert_eq!(mod_loop.state().len(), 7);
        for &s in &mod_loop.state() {
            assert!(
                s.abs() < STABILITY_THRESHOLD,
                "State {s} exploded on silence"
            );
        }

        let total_ones: u32 = out_bytes.iter().map(|b| b.count_ones()).sum();
        let ratio = (total_ones as f64) / 8000.0;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "Ones ratio {ratio} deviates significantly from 50% on silence"
        );
    }

    #[test]
    fn test_modulator_tone_within_headroom_order5() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD128).unwrap();
        let mut mod_loop = CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::Error);

        let f_bit = 5_644_800.0;
        let n_samples = 32_768;
        let amp = 10.0_f64.powf(-6.0 / 20.0);
        let mut input = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let t = i as f64 / f_bit;
            input.push(amp * (2.0 * std::f64::consts::PI * 1000.0 * t).sin());
        }

        let mut out_words = vec![0u32; n_samples / 32];
        let res = mod_loop.process_tile_u32(&input, &mut out_words);
        assert!(res.is_ok());
        assert_eq!(mod_loop.overload_count(), 0);
    }

    #[test]
    fn test_modulator_tone_within_headroom_order7() {
        let coeffs = *get_crfb7_coefficients(DsdBitRateHz::DSD128).unwrap();
        let mut mod_loop = CrfbModulator7::new(coeffs, DitherType::Tpdf, OverloadPolicy::Error);

        let f_bit = 5_644_800.0;
        let n_samples = 32_768;
        let amp = 10.0_f64.powf(-6.0 / 20.0);
        let mut input = Vec::with_capacity(n_samples);
        for i in 0..n_samples {
            let t = i as f64 / f_bit;
            input.push(amp * (2.0 * std::f64::consts::PI * 1000.0 * t).sin());
        }

        let mut out_words = vec![0u32; n_samples / 32];
        let res = mod_loop.process_tile_u32(&input, &mut out_words);
        assert!(res.is_ok());
        assert_eq!(mod_loop.overload_count(), 0);
    }

    #[test]
    fn test_modulator_overload_and_recovery_order5() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();
        let mut mod_err = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::Error);

        let overload_signal = [5.0; 32];
        let mut out_bits = [0u8; 32];
        let err = mod_err.process_tile_bits(&overload_signal, &mut out_bits);
        assert!(matches!(err, Err(ModulatorError::Overload { .. })));
        if let Err(ModulatorError::Overload { state, .. }) = err {
            assert_eq!(state.len(), 5);
        }

        let mut mod_rec = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::MuteRecover);
        let mut out_words = [0u32; 1];
        let res = mod_rec.process_tile_u32(&overload_signal, &mut out_words);
        assert!(res.is_ok());
        assert!(mod_rec.overload_count() > 0);
        assert_eq!(out_words[0], DSD_IDLE_U32_BE);
    }

    #[test]
    fn test_modulator_overload_and_recovery_order7() {
        let coeffs = *get_crfb7_coefficients(DsdBitRateHz::DSD128).unwrap();
        let mut mod_err = CrfbModulator7::new(coeffs, DitherType::None, OverloadPolicy::Error);

        let overload_signal = [5.0; 4096];
        let mut out_bits = vec![0u8; 4096];
        let err = mod_err.process_tile_bits(&overload_signal, &mut out_bits);
        assert!(matches!(err, Err(ModulatorError::Overload { .. })));
        if let Err(ModulatorError::Overload { state, .. }) = err {
            assert_eq!(state.len(), 7);
        }

        let mut mod_rec =
            CrfbModulator7::new(coeffs, DitherType::None, OverloadPolicy::MuteRecover);
        let mut out_words = [0u32; 128];
        let res = mod_rec.process_tile_u32(&overload_signal, &mut out_words);
        assert!(res.is_ok());
        assert!(mod_rec.overload_count() > 0);
        // Overloaded word should be replaced with balanced DSD idle sequence
        assert_eq!(out_words.last().copied(), Some(DSD_IDLE_U32_BE));
    }

    #[test]
    fn test_modulator_enum_dispatch() {
        let mod5 = Modulator::new(
            5,
            DsdBitRateHz::DSD64,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod5.order(), 5);
        assert_eq!(mod5.state_slice().len(), 5);

        let mod7 = Modulator::new(
            7,
            DsdBitRateHz::DSD128,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod7.order(), 7);
        assert_eq!(mod7.state_slice().len(), 7);

        let mod_bad = Modulator::new(
            6,
            DsdBitRateHz::DSD128,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        );
        assert!(matches!(mod_bad, Err(ModulatorError::UnsupportedOrder(6))));
    }
}
