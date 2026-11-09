//! Coordinator-side PCM feeding for offline conversion.
//!
//! Offline reuses the live worker input stage (`process_input_pcm`): decimation, the
//! constant pre-modulator gain and rational SRC all run inside the channel workers, exactly
//! as in the live loop. The feeder only reads stereo blocks from the input and dispatches
//! them with a constant [`FrameGain::Preserve`] envelope (no lifecycle ramps offline).

use crate::lifecycle::FrameGain;
use crate::processor::TileProcessor;
use crate::traits::{AudioInput, EngineError};
use crate::worker::InputTileControls;

/// Reusable stereo input buffers plus the constant frame-gain envelope.
pub(crate) struct PcmFeeder {
    left: Vec<f64>,
    right: Vec<f64>,
    zeros: Vec<f64>,
    gains: Vec<FrameGain>,
}

impl PcmFeeder {
    pub(crate) fn new(processor: &TileProcessor) -> Self {
        let capacity = processor.input_frame_capacity();
        Self {
            left: vec![0.0; capacity],
            right: vec![0.0; capacity],
            zeros: vec![0.0; capacity],
            // A block of `capacity` input frames never yields more than one tile of baseband.
            gains: vec![FrameGain::Preserve; processor.tile_baseband_frames()],
        }
    }

    /// Maximum input frames per block.
    pub(crate) fn capacity(&self) -> usize {
        self.left.len()
    }

    /// Reads the next block from `input`. Returns the frame count (`0` at EOF).
    pub(crate) fn read(&mut self, input: &mut dyn AudioInput) -> Result<usize, EngineError> {
        input.read_stereo(&mut self.left, &mut self.right)
    }

    /// Dispatches the first `frames` frames of the last [`read`](Self::read). Returns the
    /// number of frames produced at the intermediate rate (the cascade then yields
    /// `produced * rate_multiplier` output samples per channel).
    pub(crate) fn process(
        &mut self,
        processor: &mut TileProcessor,
        frames: usize,
        linear_gain: f64,
        interpolate_only: bool,
    ) -> Result<usize, EngineError> {
        dispatch(
            processor,
            &[&self.left[..frames], &self.right[..frames]],
            &self.gains,
            linear_gain,
            interpolate_only,
        )
    }

    /// Feeds `frames` frames of digital silence through the same input stage, flushing
    /// decimation, SRC and cascade state.
    pub(crate) fn process_silence(
        &mut self,
        processor: &mut TileProcessor,
        frames: usize,
        linear_gain: f64,
        interpolate_only: bool,
    ) -> Result<usize, EngineError> {
        dispatch(
            processor,
            &[&self.zeros[..frames], &self.zeros[..frames]],
            &self.gains,
            linear_gain,
            interpolate_only,
        )
    }
}

fn dispatch(
    processor: &mut TileProcessor,
    inputs: &[&[f64]],
    gains: &[FrameGain],
    linear_gain: f64,
    interpolate_only: bool,
) -> Result<usize, EngineError> {
    let baseband = processor.pcm_baseband_frames(inputs[0].len())?;
    let controls = InputTileControls {
        frame_gains: &gains[..baseband],
        linear_gain,
        drift_ppm: 0.0,
    };
    if interpolate_only {
        processor.process_input_pcm_interpolated(inputs, controls)
    } else {
        processor.process_input_pcm(inputs, controls)
    }
}
