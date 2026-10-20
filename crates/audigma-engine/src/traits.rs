//! Abstract audio stream input and output traits (`Design.md §2`, `§3.4`).
//!
//! Pure Rust abstractions with zero OS or C-FFI dependencies, enabling instant
//! unit and integration testing with mock streams and in-memory buffers.

use audigma_format::{AudioDecoder, AudioStreamInfo, FormatError};
use std::io::{Cursor, Seek, SeekFrom, Write};
use thiserror::Error;

/// Errors arising from audio engine input, output, or pipeline execution.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("format container or packing error: {0}")]
    Format(#[from] FormatError),

    #[error("DSP filter error: {0}")]
    Fir(#[from] audigma_dsp::FirError),

    #[error("modulator error: {0}")]
    Modulator(#[from] audigma_dsp::ModulatorError),

    #[error("settings resolution error: {0}")]
    Settings(#[from] audigma_settings::SettingsError),

    #[error("two-pass peak analysis requires a seekable input stream")]
    TwoPassRequiresSeekableInput,

    #[error("output container '{container}' requires a seekable output stream")]
    SeekableOutputRequired { container: &'static str },

    #[error("unsupported rate conversion from {in_rate} Hz to {out_rate} Hz")]
    UnsupportedRateConversion { in_rate: u32, out_rate: u32 },

    #[error("pipeline error: {0}")]
    Pipeline(String),

    #[error("output device disconnected: {0}")]
    DeviceDisconnected(String),
}

/// Abstract streaming audio input trait.
pub trait AudioInput: Send {
    /// Returns metadata describing the input audio stream.
    fn info(&self) -> &AudioStreamInfo;

    /// Reads up to `left.len()` stereo PCM frames (normalized in `[-1.0, 1.0]`).
    ///
    /// Slices `left` and `right` must have equal lengths.
    /// Returns `Ok(frames_read)`. Returns `Ok(0)` when end-of-stream (EOF) is reached.
    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError>;

    /// Returns `true` if the input stream supports rewinding for two-pass processing.
    fn can_rewind(&self) -> bool;

    /// Rewinds the stream back to timestamp 0.
    fn rewind(&mut self) -> Result<(), EngineError>;

    /// Returns current input queue buffer fill and capacity `(current_fill, capacity)`, if supported.
    fn buffer_occupancy(&self) -> Option<(usize, usize)> {
        None
    }
}

/// Abstract audio output sink trait with optional seek support.
pub trait AudioOutput: Write + Send {
    /// Returns `true` if this sink supports seeking.
    fn can_seek(&self) -> bool {
        false
    }

    /// Seeks to an offset in this sink. Fails with `ErrorKind::Unsupported` if not seekable.
    fn seek_sink(&mut self, _pos: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "seeking is not supported on this output sink",
        ))
    }
}

// Implement `std::io::Seek` for `Box<dyn AudioOutput>` so container writers (DSF, WAV)
// that require `W: Write + Seek` can wrap a boxed `AudioOutput`.
impl Seek for dyn AudioOutput + '_ {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.seek_sink(pos)
    }
}

// Blanket implementations for common standard library types
impl AudioOutput for std::fs::File {
    fn can_seek(&self) -> bool {
        true
    }

    fn seek_sink(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.seek(pos)
    }
}

impl AudioOutput for Cursor<Vec<u8>> {
    fn can_seek(&self) -> bool {
        true
    }

    fn seek_sink(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.seek(pos)
    }
}

/// Wrapper for any `W: Write + Seek + Send` that implements `AudioOutput`.
pub struct SeekableSink<W: Write + Seek + Send>(pub W);

impl<W: Write + Seek + Send> Write for SeekableSink<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write + Seek + Send> AudioOutput for SeekableSink<W> {
    fn can_seek(&self) -> bool {
        true
    }

    fn seek_sink(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

/// Wrapper for non-seekable `W: Write + Send` (e.g. `stdout`, pipes) that implements `AudioOutput`.
pub struct StreamSink<W: Write + Send>(pub W);

impl<W: Write + Send> Write for StreamSink<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write + Send> AudioOutput for StreamSink<W> {
    fn can_seek(&self) -> bool {
        false
    }
}

/// Adapter converting an `AudioDecoder` (from `audigma-format`) into an `AudioInput`.
pub struct DecoderInput {
    decoder: AudioDecoder,
    scratch_left: Vec<f64>,
    scratch_right: Vec<f64>,
    scratch_pos: usize,
}

impl DecoderInput {
    /// Creates a new `DecoderInput` wrapping an `AudioDecoder`.
    pub fn new(decoder: AudioDecoder) -> Self {
        Self {
            decoder,
            scratch_left: Vec::with_capacity(4096),
            scratch_right: Vec::with_capacity(4096),
            scratch_pos: 0,
        }
    }
}

impl AudioInput for DecoderInput {
    fn info(&self) -> &AudioStreamInfo {
        self.decoder.info()
    }

    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        assert_eq!(
            left.len(),
            right.len(),
            "left and right buffer lengths must match"
        );
        let max_to_read = left.len();
        let mut total_filled = 0;

        while total_filled < max_to_read {
            // Drain existing scratch buffer first
            let available = self.scratch_left.len() - self.scratch_pos;
            if available > 0 {
                let to_copy = available.min(max_to_read - total_filled);
                let start = self.scratch_pos;
                let end = start + to_copy;

                left[total_filled..total_filled + to_copy]
                    .copy_from_slice(&self.scratch_left[start..end]);
                right[total_filled..total_filled + to_copy]
                    .copy_from_slice(&self.scratch_right[start..end]);

                self.scratch_pos += to_copy;
                total_filled += to_copy;
                continue;
            }

            // Refill scratch buffer from decoder
            self.scratch_left.clear();
            self.scratch_right.clear();
            self.scratch_pos = 0;

            let frames_read = self
                .decoder
                .read_stereo_chunk(&mut self.scratch_left, &mut self.scratch_right)?;

            if frames_read == 0 {
                // Clean EOF reached
                break;
            }
        }

        Ok(total_filled)
    }

    fn can_rewind(&self) -> bool {
        true
    }

    fn rewind(&mut self) -> Result<(), EngineError> {
        self.scratch_left.clear();
        self.scratch_right.clear();
        self.scratch_pos = 0;
        self.decoder.rewind()?;
        Ok(())
    }
}

/// In-memory mock audio input stream for unit testing.
pub struct MockAudioInput {
    info: AudioStreamInfo,
    left: Vec<f64>,
    right: Vec<f64>,
    pos: usize,
    allow_rewind: bool,
}

impl MockAudioInput {
    /// Creates a new in-memory `MockAudioInput` with given stereo sample channels.
    pub fn new(info: AudioStreamInfo, left: Vec<f64>, right: Vec<f64>, allow_rewind: bool) -> Self {
        assert_eq!(
            left.len(),
            right.len(),
            "left and right channel lengths must match"
        );
        Self {
            info,
            left,
            right,
            pos: 0,
            allow_rewind,
        }
    }
}

impl AudioInput for MockAudioInput {
    fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        assert_eq!(
            left.len(),
            right.len(),
            "left and right buffer lengths must match"
        );
        if self.pos >= self.left.len() {
            return Ok(0);
        }

        let available = self.left.len() - self.pos;
        let to_read = available.min(left.len());

        left[..to_read].copy_from_slice(&self.left[self.pos..self.pos + to_read]);
        right[..to_read].copy_from_slice(&self.right[self.pos..self.pos + to_read]);

        self.pos += to_read;
        Ok(to_read)
    }

    fn can_rewind(&self) -> bool {
        self.allow_rewind
    }

    fn rewind(&mut self) -> Result<(), EngineError> {
        if !self.allow_rewind {
            return Err(EngineError::Pipeline(
                "mock stream does not support rewinding".into(),
            ));
        }
        self.pos = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_dsp::PcmRateHz;

    #[test]
    fn test_mock_audio_input_chunked_reads_and_rewind() {
        let info = AudioStreamInfo {
            sample_rate: PcmRateHz::PCM_44100,
            original_channels: 2,
            total_frames: Some(10),
            bits_per_sample: Some(16),
        };
        let left = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let right = vec![-1.0, -2.0, -3.0, -4.0, -5.0, -6.0, -7.0, -8.0, -9.0, -10.0];

        let mut input = MockAudioInput::new(info, left, right, true);
        assert!(input.can_rewind());

        let mut b_l = [0.0; 4];
        let mut b_r = [0.0; 4];

        let n1 = input.read_stereo(&mut b_l, &mut b_r).unwrap();
        assert_eq!(n1, 4);
        assert_eq!(b_l, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(b_r, [-1.0, -2.0, -3.0, -4.0]);

        let n2 = input.read_stereo(&mut b_l, &mut b_r).unwrap();
        assert_eq!(n2, 4);
        assert_eq!(b_l, [5.0, 6.0, 7.0, 8.0]);

        let n3 = input.read_stereo(&mut b_l, &mut b_r).unwrap();
        assert_eq!(n3, 2);
        assert_eq!(&b_l[..2], &[9.0, 10.0]);

        let n4 = input.read_stereo(&mut b_l, &mut b_r).unwrap();
        assert_eq!(n4, 0);

        // Rewind and re-read
        input.rewind().unwrap();
        let n5 = input.read_stereo(&mut b_l, &mut b_r).unwrap();
        assert_eq!(n5, 4);
        assert_eq!(b_l, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_sinks_seek_support() {
        let mut buf = Cursor::new(Vec::new());
        let mut seekable = SeekableSink(&mut buf);
        assert!(seekable.can_seek());
        assert!(seekable.seek_sink(SeekFrom::Start(10)).is_ok());

        let mut out = Vec::new();
        let mut stream = StreamSink(&mut out);
        assert!(!stream.can_seek());
        assert!(stream.seek_sink(SeekFrom::Start(10)).is_err());
    }
}
