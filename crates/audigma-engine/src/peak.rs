//! Two-pass true inter-sample peak analysis (`Design.md §4.4`).
//!
//! Because the PCM polyphase SRC and 2^K half-band FIR interpolation cascade are Linear and
//! Time-Invariant (LTI), scaling pre-SRC input by gain G scales reconstructed peaks entering
//! the modulator by exactly G.
//!
//! Pass 1 evaluates only the LTI interpolation cascade to measure the true inter-sample peak
//! `P` in dBFS, solving `gain_db = min(configured_gain_db, L - M - P)` per Design.md §4.4.

use crate::offline_feed::InputFeeder;
use crate::processor::TileProcessor;
use crate::traits::{AudioInput, EngineError};
use audigma_dsp::config::DspConfiguration;

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
///
/// Runs the processor's own input stage (decimation, SRC) at unity gain and stops after the
/// cascade, before any modulator. The processor must be configured with an input stage
/// (`TileProcessor::new_with_input`); `raw_dsd` selects raw DSD reads when that stage decodes DSD. It is reset afterwards and `input` is rewound.
pub fn measure_true_peak_pass1(
    input: &mut dyn AudioInput,
    processor: &mut TileProcessor,
    raw_dsd: bool,
    dsp_config: &DspConfiguration,
) -> Result<Pass1PeakResult, EngineError> {
    if !input.can_rewind() {
        return Err(EngineError::TwoPassRequiresSeekableInput);
    }

    let mult = processor.rate_multiplier();
    let mut feeder = InputFeeder::new(processor, raw_dsd);
    let mut max_peak = 0.0f64;
    let mut total_frames = 0u64;

    let mut scan = |processor: &TileProcessor| {
        for ch in 0..2 {
            for &sample in processor.channel_pcm(ch) {
                max_peak = max_peak.max(sample.abs());
            }
        }
    };

    while let Some(frames_read) = feeder.read(processor, input)? {
        total_frames += frames_read as u64;
        if feeder.process(processor, 1.0, true)? > 0 {
            scan(processor);
        }
    }

    // Flush filter group delay with digital silence to capture filter tail overshoots
    let delay_frames = processor.total_group_delay_output_samples() / mult + 48;
    let mut flushed = 0;
    while flushed < delay_frames {
        let chunk = (delay_frames - flushed).min(feeder.silence_capacity());
        flushed += feeder.process_silence(processor, chunk, 1.0, true)?;
        scan(processor);
    }

    // Reset worker pool filters back to initial clean state
    processor.reset()?;

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
