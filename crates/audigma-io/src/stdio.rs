//! Stdio stream I/O driver (`Design.md §2`, `§5.3`).
//!
//! Provides non-seekable stdio stream implementations:
//! - `StdioAudioInput`: reads raw PCM bytes from `stdin` or a non-seekable pipe.
//! - `StdioAudioOutput`: writes raw PCM or packed DSD/DoP frames directly to `stdout`.
//!
//! Invariants (`Design.md §5.3`):
//! - Non-seekable `stdout` is rejected if the target container requires seekability (such as `.dsf` or `.wav`).
//! - Pipe resilience: handles partial reads/writes, pipe backpressure, clean EOF, and `EPIPE` (broken pipe).

use audigma_dsp::PcmRateHz;
use audigma_dsp::config::SampleFormat;
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_format::AudioStreamInfo;
use std::io::{Read, Stdin, Stdout, Write};

/// Streaming raw PCM input read from standard input (`stdin`).
pub struct StdioAudioInput {
    stdin: Stdin,
    format: SampleFormat,
    channels: u8,
    bytes_per_sample: usize,
    info: AudioStreamInfo,
    raw_scratch: Vec<u8>,
}

impl StdioAudioInput {
    /// Creates a new `StdioAudioInput` with user-declared format, sample rate, and channels.
    pub fn new(format: SampleFormat, sample_rate: PcmRateHz, channels: u8) -> Self {
        let bytes_per_sample = match format {
            SampleFormat::S16Le => 2,
            SampleFormat::S24_3Le => 3,
            SampleFormat::S24Le | SampleFormat::S32Le | SampleFormat::F32 => 4,
            _ => 2,
        };

        let info = AudioStreamInfo::new_pcm(
            sample_rate,
            channels,
            None,
            Some(bytes_per_sample as u32 * 8),
        );

        Self {
            stdin: std::io::stdin(),
            format,
            channels,
            bytes_per_sample,
            info,
            raw_scratch: Vec::new(),
        }
    }
}

impl AudioInput for StdioAudioInput {
    fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        assert_eq!(
            left.len(),
            right.len(),
            "left and right buffer lengths must match"
        );
        let max_frames = left.len();
        let bytes_per_frame = self.bytes_per_sample * (self.channels as usize);
        let required_bytes = max_frames * bytes_per_frame;

        self.raw_scratch.resize(required_bytes, 0);

        let mut total_bytes_read = 0;
        while total_bytes_read < required_bytes {
            match self.stdin.read(&mut self.raw_scratch[total_bytes_read..]) {
                Ok(0) => break, // EOF
                Ok(n) => total_bytes_read += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(EngineError::Io(e)),
            }
        }

        let full_frames_read = total_bytes_read / bytes_per_frame;
        if full_frames_read == 0 {
            return Ok(0);
        }

        // Convert raw bytes to normalized f64 stereo
        let raw = &self.raw_scratch[..full_frames_read * bytes_per_frame];
        for frame_idx in 0..full_frames_read {
            let offset = frame_idx * bytes_per_frame;
            let (l_f64, r_f64) = match (self.format, self.channels) {
                (SampleFormat::S16Le, 1) => {
                    let s = i16::from_le_bytes([raw[offset], raw[offset + 1]]) as f64 / 32768.0;
                    (s, s)
                }
                (SampleFormat::S16Le, 2) => {
                    let l = i16::from_le_bytes([raw[offset], raw[offset + 1]]) as f64 / 32768.0;
                    let r = i16::from_le_bytes([raw[offset + 2], raw[offset + 3]]) as f64 / 32768.0;
                    (l, r)
                }
                (SampleFormat::S24_3Le, 1) => {
                    let mut b = [0u8; 4];
                    b[0..3].copy_from_slice(&raw[offset..offset + 3]);
                    let raw_val = i32::from_le_bytes(b);
                    // Sign extend 24-bit to 32-bit
                    let val = (raw_val << 8) >> 8;
                    let s = val as f64 / 8388608.0;
                    (s, s)
                }
                (SampleFormat::S24_3Le, 2) => {
                    let mut b_l = [0u8; 4];
                    b_l[0..3].copy_from_slice(&raw[offset..offset + 3]);
                    let l = (((i32::from_le_bytes(b_l)) << 8) >> 8) as f64 / 8388608.0;

                    let mut b_r = [0u8; 4];
                    b_r[0..3].copy_from_slice(&raw[offset + 3..offset + 6]);
                    let r = (((i32::from_le_bytes(b_r)) << 8) >> 8) as f64 / 8388608.0;
                    (l, r)
                }
                (SampleFormat::S24Le | SampleFormat::S32Le, 1) => {
                    let s = i32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as f64
                        / 2147483648.0;
                    (s, s)
                }
                (SampleFormat::S24Le | SampleFormat::S32Le, 2) => {
                    let l = i32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as f64
                        / 2147483648.0;
                    let r = i32::from_le_bytes(raw[offset + 4..offset + 8].try_into().unwrap())
                        as f64
                        / 2147483648.0;
                    (l, r)
                }
                (SampleFormat::F32, 1) => {
                    let s = f32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as f64;
                    (s, s)
                }
                (SampleFormat::F32, 2) => {
                    let l = f32::from_le_bytes(raw[offset..offset + 4].try_into().unwrap()) as f64;
                    let r =
                        f32::from_le_bytes(raw[offset + 4..offset + 8].try_into().unwrap()) as f64;
                    (l, r)
                }
                _ => (0.0, 0.0),
            };

            left[frame_idx] = l_f64;
            right[frame_idx] = r_f64;
        }

        Ok(full_frames_read)
    }

    fn can_rewind(&self) -> bool {
        false
    }

    fn rewind(&mut self) -> Result<(), EngineError> {
        Err(EngineError::Pipeline(
            "standard input (stdin) does not support rewinding".into(),
        ))
    }
}

/// Standard output stream driver (`stdout`) implementing non-seekable `AudioOutput`.
///
/// Handles `EPIPE` (broken pipe) cleanly without crashing.
pub struct StdioAudioOutput {
    stdout: Stdout,
}

impl StdioAudioOutput {
    /// Creates a new `StdioAudioOutput` writing to `stdout`.
    pub fn new() -> Self {
        Self {
            stdout: std::io::stdout(),
        }
    }
}

impl Default for StdioAudioOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for StdioAudioOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.stdout.write(buf) {
            Ok(n) => Ok(n),
            Err(ref e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                // Return EOF on broken pipe to allow orderly pipeline drain
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.stdout.flush() {
            Ok(()) => Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(e),
        }
    }
}

impl AudioOutput for StdioAudioOutput {
    fn can_seek(&self) -> bool {
        false
    }
}
