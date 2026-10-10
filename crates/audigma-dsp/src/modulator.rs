//! 5th-Order CRFB Delta-Sigma Modulator and TPDF dither loop (`Design.md §4.3–§4.5`).
//!
//! Implements:
//! - `CrfbModulator`: Pure-Rust 5th-order Cascade of Resonators with Feedback (CRFB)
//!   modulator utilizing verified Schreier NTF coefficients (`tables.rs`).
//! - `TpdfDither`: Fast zero-allocation PRNG generating triangular probability density dither.
//! - Overload protection (`OverloadPolicy::MuteRecover` and `OverloadPolicy::Error`),
//!   resetting integrators and substituting balanced DSD idle (`0x69`) upon instability.

use crate::config::{DitherType, OverloadPolicy};
use crate::presets::{DSD_IDLE_BYTE, DSD_IDLE_U32_BE};
use crate::tables::CrfbCoefficients;
use thiserror::Error;

/// Modulator error conditions.
#[derive(Debug, Error, PartialEq)]
pub enum ModulatorError {
    #[error("Output buffer too small: required {required}, provided {provided}")]
    OutputBufferTooSmall { required: usize, provided: usize },

    #[error("Input length {len} is not a multiple of {alignment}")]
    UnalignedInputLength { len: usize, alignment: usize },

    #[error("Modulator integrator overload detected: state={state:?}, input={input}")]
    Overload { state: [f64; 5], input: f64 },
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

/// 5th-order Cascade of Resonators with Feedback (CRFB) 1-bit Delta-Sigma Modulator.
#[derive(Debug, Clone)]
pub struct CrfbModulator {
    coeffs: CrfbCoefficients,
    state: [f64; 5],
    dither: Option<TpdfDither>,
    overload_policy: OverloadPolicy,
    overload_count: u64,
}

impl CrfbModulator {
    /// Maximum integrator state magnitude before triggering overload protection.
    pub const STABILITY_THRESHOLD: f64 = 50.0;

    /// Creates a new 5th-order CRFB modulator with the given coefficients and configuration.
    pub fn new(
        coeffs: CrfbCoefficients,
        dither_type: DitherType,
        overload_policy: OverloadPolicy,
    ) -> Self {
        let dither = match dither_type {
            DitherType::None => None,
            DitherType::Tpdf => Some(TpdfDither::default()),
        };
        Self {
            coeffs,
            state: [0.0; 5],
            dither,
            overload_policy,
            overload_count: 0,
        }
    }

    /// Resets all internal integrator states to zero without resetting statistics.
    pub fn reset(&mut self) {
        self.state.fill(0.0);
    }

    /// Returns the current 5-element integrator state vector.
    #[inline]
    pub const fn state(&self) -> [f64; 5] {
        self.state
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

        // Quantizer input
        let y = self.state[4] + dither_val;
        let (v, bit) = if y >= 0.0 { (1.0, 1u8) } else { (-1.0, 0u8) };

        // 5th-order Lossless Discrete Integrator (LDI) state update
        let c = &self.coeffs;
        let x1 = self.state[0] - c.g1 * self.state[1] + c.b1 * u - c.a[0] * v;
        let x2 = self.state[1] + x1 - c.a[1] * v;
        let x3 = self.state[2] - c.g2 * self.state[3] + x2 - c.a[2] * v;
        let x4 = self.state[3] + x3 - c.a[3] * v;
        let x5 = self.state[4] + x4 - c.a[4] * v;

        // Stability check across state vector
        if x1.abs() > Self::STABILITY_THRESHOLD
            || x2.abs() > Self::STABILITY_THRESHOLD
            || x3.abs() > Self::STABILITY_THRESHOLD
            || x4.abs() > Self::STABILITY_THRESHOLD
            || x5.abs() > Self::STABILITY_THRESHOLD
            || x1.is_nan()
        {
            self.overload_count += 1;
            match self.overload_policy {
                OverloadPolicy::MuteRecover => {
                    self.state.fill(0.0);
                    // Return alternating pattern or bit from idle stream
                    return Ok(bit);
                }
                OverloadPolicy::Error => {
                    return Err(ModulatorError::Overload {
                        state: [x1, x2, x3, x4, x5],
                        input: u,
                    });
                }
            }
        }

        self.state = [x1, x2, x3, x4, x5];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rates::DsdBitRateHz;
    use crate::tables::get_crfb_coefficients;

    #[test]
    fn test_modulator_silence_stability() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();
        let mut mod_loop =
            CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        // Run 10,000 samples of digital silence (0.0)
        let silence = [0.0; 8000];
        let mut out_bytes = [0u8; 1000];
        mod_loop
            .process_tile_bytes(&silence, &mut out_bytes)
            .unwrap();

        assert_eq!(mod_loop.overload_count(), 0);
        for &s in &mod_loop.state() {
            assert!(
                s.abs() < CrfbModulator::STABILITY_THRESHOLD,
                "State {s} exploded on silence"
            );
        }

        // Bit stream should have roughly equal ones and zeros on silence
        let total_ones: u32 = out_bytes.iter().map(|b| b.count_ones()).sum();
        let ratio = (total_ones as f64) / 8000.0;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "Ones ratio {ratio} deviates significantly from 50% on silence"
        );
    }

    #[test]
    fn test_modulator_tone_within_headroom() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD128).unwrap();
        let mut mod_loop = CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::Error);

        // 1 kHz sine tone at -6 dBFS (amp = 0.501)
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
    fn test_modulator_overload_and_recovery() {
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();
        let mut mod_err = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::Error);

        // Huge square wave well beyond 0 dBFS to force instability
        let overload_signal = [5.0; 32];
        let mut out_bits = [0u8; 32];
        let err = mod_err.process_tile_bits(&overload_signal, &mut out_bits);
        assert!(matches!(err, Err(ModulatorError::Overload { .. })));

        // Test MuteRecover policy
        let mut mod_rec = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::MuteRecover);
        let mut out_words = [0u32; 1];
        let res = mod_rec.process_tile_u32(&overload_signal, &mut out_words);
        assert!(res.is_ok());
        assert!(mod_rec.overload_count() > 0);
        // Overloaded word should be replaced with balanced DSD idle sequence
        assert_eq!(out_words[0], DSD_IDLE_U32_BE);
    }
}
