//! Two-pass true inter-sample peak analysis (`Design.md §4.4`).
//!
//! Because the PCM polyphase SRC and 2^K half-band FIR interpolation cascade are Linear and
//! Time-Invariant (LTI), scaling pre-SRC input by gain G scales reconstructed peaks entering
//! the modulator by exactly G.
//!
//! Pass 1 evaluates only the LTI interpolation cascade to measure the true inter-sample peak
//! `P` in dBFS, solving `gain_db = min(configured_gain_db, L - M - P)` per Design.md §4.4.

use crate::traits::{AudioInput, EngineError};
use audigma_dsp::config::DspConfiguration;
use audigma_dsp::fir::{HalfBandCascade, PolyphaseResampler};

/// Peak analysis measurement result from Pass 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass1PeakResult {
    /// True inter-sample peak amplitude in linear units (1.0 = 0 dBFS).
    pub peak_linear: f64,
    /// True inter-sample peak in dBFS (20 * log10(peak_linear)).
    pub peak_dbfs: f64,
    /// Calibrated safe pre-modulator gain in dB (`min(configured_gain_db, L - M - P)`).
    pub calibrated_gain_db: f64,
    /// Calibrated safe pre-modulator gain in linear factor (`10^(gain_db / 20)`).
    pub calibrated_gain_linear: f64,
    /// Total baseband input frames measured.
    pub total_input_frames: u64,
}

/// Measures the true inter-sample peak across the LTI interpolation cascade.
pub fn measure_true_peak_pass1(
    input: &mut dyn AudioInput,
    mut rational_src: Option<(PolyphaseResampler, PolyphaseResampler)>,
    mut cascade_left: HalfBandCascade,
    mut cascade_right: HalfBandCascade,
    dsp_config: &DspConfiguration,
    tile_size: usize,
) -> Result<Pass1PeakResult, EngineError> {
    if !input.can_rewind() {
        return Err(EngineError::TwoPassRequiresSeekableInput);
    }

    let mult = cascade_left.rate_multiplier();
    let cascade_tile_len = tile_size * mult;

    let mut in_left = vec![0.0; tile_size];
    let mut in_right = vec![0.0; tile_size];
    let mut cascade_out_left = vec![0.0; cascade_tile_len];
    let mut cascade_out_right = vec![0.0; cascade_tile_len];

    let mut src_out_left = vec![0.0; tile_size * 2];
    let mut src_out_right = vec![0.0; tile_size * 2];

    let mut max_peak = 0.0f64;
    let mut total_frames = 0u64;

    loop {
        let frames_read = input.read_stereo(&mut in_left, &mut in_right)?;
        if frames_read == 0 {
            break;
        }
        total_frames += frames_read as u64;

        let left_slice = &in_left[..frames_read];
        let right_slice = &in_right[..frames_read];

        match rational_src {
            Some((ref mut src_l, ref mut src_r)) => {
                let (_, prod_l) = src_l.process(left_slice, &mut src_out_left);
                let (_, prod_r) = src_r.process(right_slice, &mut src_out_right);
                let prod = prod_l.min(prod_r);

                let mut offset = 0;
                while offset < prod {
                    let chunk = (prod - offset).min(tile_size);
                    let req_out = chunk * mult;

                    cascade_left.process_tile(
                        &src_out_left[offset..offset + chunk],
                        &mut cascade_out_left[..req_out],
                    )?;
                    cascade_right.process_tile(
                        &src_out_right[offset..offset + chunk],
                        &mut cascade_out_right[..req_out],
                    )?;

                    for &sample in &cascade_out_left[..req_out] {
                        let abs = sample.abs();
                        if abs > max_peak {
                            max_peak = abs;
                        }
                    }
                    for &sample in &cascade_out_right[..req_out] {
                        let abs = sample.abs();
                        if abs > max_peak {
                            max_peak = abs;
                        }
                    }

                    offset += chunk;
                }
            }
            None => {
                let req_out = frames_read * mult;
                cascade_left.process_tile(left_slice, &mut cascade_out_left[..req_out])?;
                cascade_right.process_tile(right_slice, &mut cascade_out_right[..req_out])?;

                for &sample in &cascade_out_left[..req_out] {
                    let abs = sample.abs();
                    if abs > max_peak {
                        max_peak = abs;
                    }
                }
                for &sample in &cascade_out_right[..req_out] {
                    let abs = sample.abs();
                    if abs > max_peak {
                        max_peak = abs;
                    }
                }
            }
        }
    }

    // Flush filter group delay with digital silence to capture filter tail overshoots
    let delay_frames = cascade_left.total_group_delay_output_samples() / mult + 48;
    in_left.fill(0.0);
    in_right.fill(0.0);

    let mut flushed = 0;
    while flushed < delay_frames {
        let chunk = (delay_frames - flushed).min(tile_size);
        let req_out = chunk * mult;

        cascade_left.process_tile(&in_left[..chunk], &mut cascade_out_left[..req_out])?;
        cascade_right.process_tile(&in_right[..chunk], &mut cascade_out_right[..req_out])?;

        for &sample in &cascade_out_left[..req_out] {
            let abs = sample.abs();
            if abs > max_peak {
                max_peak = abs;
            }
        }
        for &sample in &cascade_out_right[..req_out] {
            let abs = sample.abs();
            if abs > max_peak {
                max_peak = abs;
            }
        }

        flushed += chunk;
    }

    // Rewind input stream back to timestamp 0 for Pass 2
    input.rewind()?;

    // Compute peak in dBFS
    let peak_linear = max_peak.max(1e-12);
    let peak_dbfs = 20.0 * peak_linear.log10();
    let calibrated_gain_db = dsp_config.resolve_two_pass_gain_db(peak_dbfs);
    let calibrated_gain_linear = 10.0f64.powf(calibrated_gain_db / 20.0);

    Ok(Pass1PeakResult {
        peak_linear,
        peak_dbfs,
        calibrated_gain_db,
        calibrated_gain_linear,
        total_input_frames: total_frames,
    })
}
