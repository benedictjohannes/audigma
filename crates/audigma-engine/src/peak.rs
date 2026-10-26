//! Two-pass true inter-sample peak analysis (`Design.md §4.4`).
//!
//! Because the PCM polyphase SRC and 2^K half-band FIR interpolation cascade are Linear and
//! Time-Invariant (LTI), scaling pre-SRC input by gain G scales reconstructed peaks entering
//! the modulator by exactly G.
//!
//! Pass 1 evaluates only the LTI interpolation cascade to measure the true inter-sample peak
//! `P` in dBFS, solving `gain_db = min(configured_gain_db, L - M - P)` per Design.md §4.4.

use crate::decimator::ChannelDecimatorPool;
use crate::traits::{AudioInput, EngineError};
use crate::worker::ChannelWorkerPool;
use audigma_dsp::config::DspConfiguration;
use audigma_dsp::fir::PolyphaseResampler;

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

/// Measures the true inter-sample peak across the LTI interpolation cascade using the channel worker pool.
pub fn measure_true_peak_pass1(
    input: &mut dyn AudioInput,
    mut rational_src: Option<(PolyphaseResampler, PolyphaseResampler)>,
    mut decimator: Option<ChannelDecimatorPool>,
    worker_pool: &mut ChannelWorkerPool,
    dsp_config: &DspConfiguration,
    tile_size: usize,
) -> Result<Pass1PeakResult, EngineError> {
    if !input.can_rewind() {
        return Err(EngineError::TwoPassRequiresSeekableInput);
    }

    let mult = worker_pool.rate_multiplier();
    let dec_divider = decimator.as_ref().map(|d| d.divider()).unwrap_or(1);
    let in_tile_size = tile_size * dec_divider;

    let mut in_left = vec![0.0; in_tile_size];
    let mut in_right = vec![0.0; in_tile_size];

    let mut dec_left = vec![0.0; tile_size * 2];
    let mut dec_right = vec![0.0; tile_size * 2];

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

        let (left_slice, right_slice) = if let Some(ref mut dec) = decimator {
            let produced = dec.process(
                &[&in_left[..frames_read], &in_right[..frames_read]],
                &mut [&mut dec_left, &mut dec_right],
            )?;
            if produced == 0 {
                continue;
            }
            (&dec_left[..produced], &dec_right[..produced])
        } else {
            (&in_left[..frames_read], &in_right[..frames_read])
        };

        match rational_src {
            Some((ref mut src_l, ref mut src_r)) => {
                let (_, prod_l) = src_l.process(left_slice, &mut src_out_left);
                let (_, prod_r) = src_r.process(right_slice, &mut src_out_right);
                let prod = prod_l.min(prod_r);

                let mut offset = 0;
                while offset < prod {
                    let chunk = (prod - offset).min(tile_size);

                    worker_pool.process_tile_pcm(&[
                        &src_out_left[offset..offset + chunk],
                        &src_out_right[offset..offset + chunk],
                    ])?;

                    let out_l = worker_pool.channel_pcm(0);
                    let out_r = worker_pool.channel_pcm(1);

                    for &sample in out_l {
                        let abs = sample.abs();
                        if abs > max_peak {
                            max_peak = abs;
                        }
                    }
                    for &sample in out_r {
                        let abs = sample.abs();
                        if abs > max_peak {
                            max_peak = abs;
                        }
                    }

                    offset += chunk;
                }
            }
            None => {
                worker_pool.process_tile_pcm(&[left_slice, right_slice])?;

                let out_l = worker_pool.channel_pcm(0);
                let out_r = worker_pool.channel_pcm(1);

                for &sample in out_l {
                    let abs = sample.abs();
                    if abs > max_peak {
                        max_peak = abs;
                    }
                }
                for &sample in out_r {
                    let abs = sample.abs();
                    if abs > max_peak {
                        max_peak = abs;
                    }
                }
            }
        }
    }

    // Flush filter group delay with digital silence to capture filter tail overshoots
    let delay_frames = worker_pool.total_group_delay_output_samples() / mult + 48;
    in_left.fill(0.0);
    in_right.fill(0.0);

    let mut flushed = 0;
    while flushed < delay_frames {
        let chunk = (delay_frames - flushed).min(tile_size);

        worker_pool.process_tile_pcm(&[&in_left[..chunk], &in_right[..chunk]])?;

        let out_l = worker_pool.channel_pcm(0);
        let out_r = worker_pool.channel_pcm(1);

        for &sample in out_l {
            let abs = sample.abs();
            if abs > max_peak {
                max_peak = abs;
            }
        }
        for &sample in out_r {
            let abs = sample.abs();
            if abs > max_peak {
                max_peak = abs;
            }
        }

        flushed += chunk;
    }

    // Reset worker pool filters back to initial clean state
    worker_pool.reset()?;

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
