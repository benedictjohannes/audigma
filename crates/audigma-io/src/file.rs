//! File I/O driver (`Design.md §2`, `§5.3`).
//!
//! Provides seekable file-backed implementations of `AudioInput` and `AudioOutput`.

use audigma_engine::traits::{AudioInput, AudioOutput, DecoderInput, EngineError};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// File-backed streaming audio input that decodes WAV or FLAC files.
pub struct FileAudioInput {
    inner: DecoderInput,
}

impl FileAudioInput {
    /// Opens an audio file for decoding.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, EngineError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| {
            EngineError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to open input file '{}': {e}", path_ref.display()),
            ))
        })?;

        let ext = path_ref
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_string());

        let decoder = AudioDecoder::try_new(file, ext.as_deref())?;
        Ok(Self {
            inner: DecoderInput::new(decoder),
        })
    }
}

impl AudioInput for FileAudioInput {
    #[inline]
    fn info(&self) -> &AudioStreamInfo {
        self.inner.info()
    }

    #[inline]
    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        self.inner.read_stereo(left, right)
    }

    #[inline]
    fn can_rewind(&self) -> bool {
        self.inner.can_rewind()
    }

    #[inline]
    fn rewind(&mut self) -> Result<(), EngineError> {
        self.inner.rewind()
    }
}

/// File-backed seekable audio output sink.
pub struct FileAudioOutput {
    file: File,
}

impl FileAudioOutput {
    /// Creates or truncates a file for output writing.
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self, EngineError> {
        let path_ref = path.as_ref();
        let file = File::create(path_ref).map_err(|e| {
            EngineError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to create output file '{}': {e}", path_ref.display()),
            ))
        })?;
        Ok(Self { file })
    }
}

impl Write for FileAudioOutput {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl AudioOutput for FileAudioOutput {
    #[inline]
    fn can_seek(&self) -> bool {
        true
    }

    #[inline]
    fn seek_sink(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}
