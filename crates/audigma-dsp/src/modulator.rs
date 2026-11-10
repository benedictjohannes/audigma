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
use crate::defaults::{DSD_IDLE_BYTE, DSD_IDLE_U32_BE};
use crate::rates::DsdBitRateHz;
use crate::tables::{CrfbCoefficients5, CrfbCoefficients7};
use smallvec::SmallVec;
use thiserror::Error;

/// Maximum integrator state magnitude before triggering overload protection.
pub const STABILITY_THRESHOLD: f64 = 50.0;

/// Maximum magnitude, preserving a NaN in the right-hand operand.
///
/// Unlike `f64::max`, this allows a hardware maximum without the extra
/// handling needed to discard NaNs. The final reduction operand must
/// be the last integrator, to which any NaN in the recurrence propagates.
#[inline(always)]
fn max_state_magnitude(a: f64, b: f64) -> f64 {
    let a = a.abs();
    let b = b.abs();
    if a > b { a } else { b }
}

/// True when `magnitude` is above the stability threshold or is NaN.
///
/// The negated `<=` is deliberate: every comparison with NaN is false, so
/// `!(x <= T)` flags a diverged (NaN) state as overloaded, whereas the
/// "simplified" `x > T` would silently report it as stable.
#[inline(always)]
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn exceeds_stability(magnitude: f64) -> bool {
    !(magnitude <= STABILITY_THRESHOLD)
}

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
        let sum = (r1 as u64 + r2 as u64) as f64;
        (sum * U32_INV - 1.0) * self.amplitude
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
    /// `v` carries the comparator decision in its sign only: the quantizer output is `copysign(1, v)`
    /// (callers pass the comparator input `y` itself, so `-0.0` decides `-1`).
    ///
    /// The CRFB update `x' = L^-1 (R s + b1 u e_0 - a v)` is evaluated expanded: `L^-1` (all `c = 1`) is a
    /// prefix sum, so `x'_k = p_k - gamma_k v` with `p` the prefix sums of `w` (`w_k = s_k - g s_{k+1}` on
    /// resonator stages, `s_k` elsewhere, `b1 u` folded into `w_0`) computed as a depth-3 parallel prefix, and
    /// `gamma_k v` applied as `copysign(gamma_k, v)` (`gamma_k >= 0`). This keeps the decision off a branch and
    /// out of the integrator chain: the loop-carried path is a few FP ops instead of two per stage
    /// (measured 1.5-1.6x faster per bit than the stage-by-stage form). The operation order is
    /// mirrored exactly by `tools/design/dsm_analysis.crfb_step` (bit-exact reference).
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
        // Prefix-sum form (see `CrfbCore::step`); same operation order as `dsm_analysis.crfb_step`.
        let w0 = (s[0] + c.b1 * u) - c.g1 * s[1];
        let w2 = s[2] - c.g2 * s[3];
        let t01 = w0 + s[1];
        let t23 = w2 + s[3];
        let p3 = t01 + t23;
        let x1 = w0 - c.gamma[0].copysign(v);
        let x2 = t01 - c.gamma[1].copysign(v);
        let x3 = (t01 + w2) - c.gamma[2].copysign(v);
        let x4 = p3 - c.gamma[3].copysign(v);
        let x5 = (p3 + s[4]) - c.gamma[4].copysign(v);

        let max_state = max_state_magnitude(
            max_state_magnitude(max_state_magnitude(x1, x2), max_state_magnitude(x3, x4)),
            x5,
        );
        let overloaded = exceeds_stability(max_state);

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
        // Prefix-sum form (see `CrfbCore::step`); same operation order as `dsm_analysis.crfb_step`.
        let w0 = (s[0] + c.b1 * u) - c.g1 * s[1];
        let w2 = s[2] - c.g2 * s[3];
        let w4 = s[4] - c.g3 * s[5];
        let t01 = w0 + s[1];
        let t23 = w2 + s[3];
        let t45 = w4 + s[5];
        let t456 = t45 + s[6];
        let p3 = t01 + t23;
        let x1 = w0 - c.gamma[0].copysign(v);
        let x2 = t01 - c.gamma[1].copysign(v);
        let x3 = (t01 + w2) - c.gamma[2].copysign(v);
        let x4 = p3 - c.gamma[3].copysign(v);
        let x5 = (p3 + w4) - c.gamma[4].copysign(v);
        let x6 = (p3 + t45) - c.gamma[5].copysign(v);
        let x7 = (p3 + t456) - c.gamma[6].copysign(v);

        let max_state = max_state_magnitude(
            max_state_magnitude(max_state_magnitude(x1, x2), max_state_magnitude(x3, x4)),
            max_state_magnitude(max_state_magnitude(x5, x6), x7),
        );
        let overloaded = exceeds_stability(max_state);

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
        // Sign-bit comparator: `v = y` carries the decision `copysign(1, y)` into `step`, branch-free.
        let v = y;
        let bit = (!y.is_sign_negative()) as u8;

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

        let mut state = self.state;
        let mut dither = self.dither.clone();
        let mut overload_count = self.overload_count;
        let coeffs = self.coeffs;

        for (byte_idx, out_byte) in out_bytes[..required_bytes].iter_mut().enumerate() {
            let in_off = byte_idx * 8;
            let byte_initial_overloads = overload_count;
            let mut byte = 0u8;

            for bit_idx in 0..8 {
                let u = input[in_off + bit_idx];
                let dither_val = match dither {
                    Some(ref mut d) => d.next_sample(),
                    None => 0.0,
                };

                let y = C::quantizer_input(&state) + dither_val;
                // Sign-bit comparator: `v = y` carries the decision `copysign(1, y)` into `step`, branch-free.
                let v = y;
                let bit = (!y.is_sign_negative()) as u8;

                let (new_state, overloaded) = C::step(&coeffs, &state, u, v);
                if overloaded {
                    overload_count += 1;
                    match self.overload_policy {
                        OverloadPolicy::MuteRecover => {
                            state = C::zero_state();
                        }
                        OverloadPolicy::Error => {
                            self.state = state;
                            self.dither = dither;
                            self.overload_count = overload_count;
                            let mut state_vec = SmallVec::new();
                            state_vec.extend_from_slice(new_state.as_ref());
                            return Err(ModulatorError::Overload {
                                state: state_vec,
                                input: u,
                            });
                        }
                    }
                } else {
                    state = new_state;
                }

                byte = (byte << 1) | bit;
            }

            if overload_count > byte_initial_overloads
                && self.overload_policy == OverloadPolicy::MuteRecover
            {
                *out_byte = DSD_IDLE_BYTE;
            } else {
                *out_byte = byte;
            }
        }

        self.state = state;
        self.dither = dither;
        self.overload_count = overload_count;

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

        let mut state = self.state;
        let mut dither = self.dither.clone();
        let mut overload_count = self.overload_count;
        let coeffs = self.coeffs;

        for (word_idx, out_word) in out_words[..required_words].iter_mut().enumerate() {
            let in_off = word_idx * 32;
            let word_initial_overloads = overload_count;
            let mut word = 0u32;

            for bit_idx in 0..32 {
                let u = input[in_off + bit_idx];
                let dither_val = match dither {
                    Some(ref mut d) => d.next_sample(),
                    None => 0.0,
                };

                let y = C::quantizer_input(&state) + dither_val;
                // Sign-bit comparator: `v = y` carries the decision `copysign(1, y)` into `step`, branch-free.
                let v = y;
                let bit = (!y.is_sign_negative()) as u32;

                let (new_state, overloaded) = C::step(&coeffs, &state, u, v);
                if overloaded {
                    overload_count += 1;
                    match self.overload_policy {
                        OverloadPolicy::MuteRecover => {
                            state = C::zero_state();
                        }
                        OverloadPolicy::Error => {
                            self.state = state;
                            self.dither = dither;
                            self.overload_count = overload_count;
                            let mut state_vec = SmallVec::new();
                            state_vec.extend_from_slice(new_state.as_ref());
                            return Err(ModulatorError::Overload {
                                state: state_vec,
                                input: u,
                            });
                        }
                    }
                } else {
                    state = new_state;
                }

                word = (word << 1) | bit;
            }

            if overload_count > word_initial_overloads
                && self.overload_policy == OverloadPolicy::MuteRecover
            {
                *out_word = DSD_IDLE_U32_BE;
            } else {
                *out_word = word;
            }
        }

        self.state = state;
        self.dither = dither;
        self.overload_count = overload_count;

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

/// Measured maximum stable input `L` (dBFS) of the modulator for `order` at `bit_rate` (`Design.md §4.3-§4.4`).
///
/// Generated per rate and order by `tools/design/design_crfb.py` (DC + tone level sweep); the headroom
/// inequality subtracts the safety margin `M` from it.
pub fn max_stable_input_dbfs(order: u8, bit_rate: DsdBitRateHz) -> Result<f64, ModulatorError> {
    match order {
        5 => crate::tables::get_crfb5_coefficients(bit_rate)
            .map(|c| c.max_stable_input_dbfs)
            .ok_or(ModulatorError::UnsupportedRate { bit_rate, order }),
        7 => crate::tables::get_crfb7_coefficients(bit_rate)
            .map(|c| c.max_stable_input_dbfs)
            .ok_or(ModulatorError::UnsupportedRate { bit_rate, order }),
        other => Err(ModulatorError::UnsupportedOrder(other)),
    }
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

    fn check_core_overload_detection<C: CrfbCore>(coeffs: C::Coeffs) {
        let values = [
            0.0,
            STABILITY_THRESHOLD,
            -STABILITY_THRESHOLD,
            STABILITY_THRESHOLD + 1.0,
            -STABILITY_THRESHOLD - 1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        for index in 0..C::ORDER {
            for value in values {
                let mut state = C::zero_state();
                state.as_mut()[index] = value;
                for input in [0.0, 0.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                    for feedback in [-1.0, 1.0] {
                        let (next, overloaded) = C::step(&coeffs, &state, input, feedback);
                        let expected = next
                            .as_ref()
                            .iter()
                            .any(|s| s.is_nan() || s.abs() > STABILITY_THRESHOLD);
                        assert_eq!(
                            overloaded,
                            expected,
                            "order {}, state {state:?}, input {input}, feedback {feedback}",
                            C::ORDER,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_overload_reduction_rejects_nonfinite_states() {
        check_core_overload_detection::<CrfbCore5>(
            *get_crfb_coefficients(DsdBitRateHz::DSD512).unwrap(),
        );
        check_core_overload_detection::<CrfbCore7>(
            *get_crfb7_coefficients(DsdBitRateHz::DSD512).unwrap(),
        );
    }

    #[test]
    fn test_nan_input_error_and_mute_recovery() {
        for order in [5, 7] {
            let mut error = Modulator::new(
                order,
                DsdBitRateHz::DSD512,
                DitherType::Tpdf,
                OverloadPolicy::Error,
            )
            .unwrap();
            assert!(matches!(
                error.process_tile_bytes(&[f64::NAN; 8], &mut [0]),
                Err(ModulatorError::Overload { .. })
            ));
            assert_eq!(error.overload_count(), 1);
            assert!(error.state_slice().iter().all(|s| s.is_finite()));

            let mut recover = Modulator::new(
                order,
                DsdBitRateHz::DSD512,
                DitherType::Tpdf,
                OverloadPolicy::MuteRecover,
            )
            .unwrap();
            let mut input = [0.0; 64];
            input[31] = f64::NAN;
            let mut words = [0; 2];
            recover.process_tile_u32(&input, &mut words).unwrap();
            assert_eq!(words[0], DSD_IDLE_U32_BE);
            assert_eq!(recover.overload_count(), 1);
            assert!(recover.state_slice().iter().all(|s| s.is_finite()));
        }
    }

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
    fn test_modulator_tone_within_headroom_dsd512_order5() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD512).unwrap();
        let mut mod_loop = CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::Error);

        let f_bit = 22_579_200.0;
        let n_samples = 65_536;
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
    fn test_modulator_tone_within_headroom_dsd512_order7() {
        let coeffs = *get_crfb7_coefficients(DsdBitRateHz::DSD512).unwrap();
        let mut mod_loop = CrfbModulator7::new(coeffs, DitherType::Tpdf, OverloadPolicy::Error);

        let f_bit = 22_579_200.0;
        let n_samples = 65_536;
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

        let mod5_512 = Modulator::new(
            5,
            DsdBitRateHz::DSD512,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod5_512.order(), 5);
        assert_eq!(mod5_512.state_slice().len(), 5);

        let mod7_512 = Modulator::new(
            7,
            DsdBitRateHz::DSD512,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod7_512.order(), 7);
        assert_eq!(mod7_512.state_slice().len(), 7);

        let mod5_512_48 = Modulator::new(
            5,
            DsdBitRateHz::DSD512_48,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod5_512_48.order(), 5);
        assert_eq!(mod5_512_48.state_slice().len(), 5);

        let mod7_512_48 = Modulator::new(
            7,
            DsdBitRateHz::DSD512_48,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        )
        .unwrap();
        assert_eq!(mod7_512_48.order(), 7);
        assert_eq!(mod7_512_48.state_slice().len(), 7);

        let mod_bad = Modulator::new(
            6,
            DsdBitRateHz::DSD128,
            DitherType::Tpdf,
            OverloadPolicy::MuteRecover,
        );
        assert!(matches!(mod_bad, Err(ModulatorError::UnsupportedOrder(6))));
    }
}
