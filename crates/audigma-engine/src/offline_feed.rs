//! Coordinator-side feeding for offline conversion.
//!
//! Offline reuses the live worker input stage (`process_input_pcm` / `process_input_dsd`):
//! DSD decoding, PCM decimation, the constant pre-modulator gain and rational SRC all run
//! inside the channel workers, exactly as in the live loop. The feeder only reads stereo
//! blocks from the input and dispatches them with a constant [`FrameGain::Preserve`]
//! envelope (no lifecycle ramps offline).

use crate::lifecycle::FrameGain;
use crate::processor::TileProcessor;
use crate::traits::{AudioInput, EngineError};
use crate::worker::InputTileControls;

/// Reusable stereo input buffers plus the constant frame-gain envelope.
pub(crate) struct InputFeeder {
    left: Vec<f64>,
    right: Vec<f64>,
    dsd_left: Vec<u8>,
    dsd_right: Vec<u8>,
    zeros: Vec<f64>,
    gains: Vec<FrameGain>,
    /// Units per channel held by the last [`read`](Self::read): DSD bytes (raw DSD) or
    /// PCM frames.
    pending: usize,
    raw_dsd: bool,
}

impl InputFeeder {
    /// `raw_dsd`: read raw DSD bytes (`read_dsd_bytes`) instead of decoded PCM. The
    /// processor must have been built with a matching `InputDspConfig::dsd`.
    pub(crate) fn new(processor: &TileProcessor, raw_dsd: bool) -> Self {
        let capacity = processor.input_frame_capacity();
        let dsd_capacity = if raw_dsd {
            processor.input_dsd_byte_capacity()
        } else {
            0
        };
        Self {
            left: vec![0.0; if raw_dsd { 0 } else { capacity }],
            right: vec![0.0; if raw_dsd { 0 } else { capacity }],
            dsd_left: vec![0; dsd_capacity],
            dsd_right: vec![0; dsd_capacity],
            zeros: vec![0.0; capacity],
            // A block of `capacity` input frames never yields more than one tile of baseband.
            gains: vec![FrameGain::Preserve; processor.tile_baseband_frames()],
            pending: 0,
            raw_dsd,
        }
    }

    /// Maximum PCM frames per silence block (the PCM input capacity).
    pub(crate) fn silence_capacity(&self) -> usize {
        self.zeros.len()
    }

    /// Reads the next block from `input`. Returns the number of input frames it holds
    /// (decoded PCM frames for raw DSD; may be `0` while the workers keep a residue), or
    /// `None` at end of input.
    pub(crate) fn read(
        &mut self,
        processor: &TileProcessor,
        input: &mut dyn AudioInput,
    ) -> Result<Option<usize>, EngineError> {
        if self.raw_dsd {
            let bytes = input.read_dsd_bytes(&mut [&mut self.dsd_left, &mut self.dsd_right])?;
            if bytes == 0 {
                return Ok(None);
            }
            self.pending = bytes;
            Ok(Some(processor.dsd_decoded_frames(bytes)?))
        } else {
            let frames = input.read_stereo(&mut self.left, &mut self.right)?;
            self.pending = frames;
            Ok((frames > 0).then_some(frames))
        }
    }

    /// Dispatches the last [`read`](Self::read) block. Returns the number of frames
    /// produced at the intermediate rate (the cascade then yields
    /// `produced * rate_multiplier` output samples per channel).
    pub(crate) fn process(
        &mut self,
        processor: &mut TileProcessor,
        linear_gain: f64,
        interpolate_only: bool,
    ) -> Result<usize, EngineError> {
        if self.raw_dsd {
            let n = self.pending;
            let baseband = processor.dsd_baseband_frames(n)?;
            let controls = self.controls(baseband, linear_gain);
            let inputs: [&[u8]; 2] = [&self.dsd_left[..n], &self.dsd_right[..n]];
            if interpolate_only {
                processor.process_input_dsd_interpolated(&inputs, controls)
            } else {
                processor.process_input_dsd(&inputs, controls)
            }
        } else {
            let n = self.pending;
            self.dispatch_pcm(processor, n, false, linear_gain, interpolate_only)
        }
    }

    /// Feeds `frames` PCM frames of digital silence through the PCM input stage (after any
    /// DSD decoding), flushing decimation, SRC and cascade state.
    pub(crate) fn process_silence(
        &mut self,
        processor: &mut TileProcessor,
        frames: usize,
        linear_gain: f64,
        interpolate_only: bool,
    ) -> Result<usize, EngineError> {
        self.dispatch_pcm(processor, frames, true, linear_gain, interpolate_only)
    }

    fn dispatch_pcm(
        &mut self,
        processor: &mut TileProcessor,
        frames: usize,
        silence: bool,
        linear_gain: f64,
        interpolate_only: bool,
    ) -> Result<usize, EngineError> {
        let baseband = processor.pcm_baseband_frames(frames)?;
        let controls = self.controls(baseband, linear_gain);
        let inputs: [&[f64]; 2] = if silence {
            [&self.zeros[..frames], &self.zeros[..frames]]
        } else {
            [&self.left[..frames], &self.right[..frames]]
        };
        if interpolate_only {
            processor.process_input_pcm_interpolated(&inputs, controls)
        } else {
            processor.process_input_pcm(&inputs, controls)
        }
    }

    fn controls(&self, baseband: usize, linear_gain: f64) -> InputTileControls<'_> {
        InputTileControls {
            frame_gains: &self.gains[..baseband],
            linear_gain,
            drift_ppm: 0.0,
        }
    }
}
