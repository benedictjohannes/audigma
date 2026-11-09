//! High-rate PCM `.wav` file writer using `hound` (`Design.md §3.1`, `§5.3`).
//!
//! Supports 16, 24, 32-bit integer PCM and 32-bit float PCM at sampling rates up to 768 kHz.

use crate::FormatError;
use audigma_dsp::{DitherType, PcmQuantizer, PcmRateHz, SampleFormat};
use hound::{SampleFormat as HoundFormat, WavSpec, WavWriter};
use std::io::{Seek, Write};

/// High-rate PCM WAV file writer.
pub struct PcmWavWriter<W: Write + Seek> {
    writer: WavWriter<W>,
    sample_format: SampleFormat,
    quantizer: PcmQuantizer,
    frames_written: u64,
}

impl<W: Write + Seek> PcmWavWriter<W> {
    /// Initializes a new WAV file writer targeting high-rate stereo PCM.
    pub fn new(
        writer: W,
        sample_rate: PcmRateHz,
        sample_format: SampleFormat,
    ) -> Result<Self, FormatError> {
        let (bits_per_sample, hound_fmt) = match sample_format {
            SampleFormat::S16Le => (16u16, HoundFormat::Int),
            SampleFormat::S24Le | SampleFormat::S24_3Le => (24u16, HoundFormat::Int),
            SampleFormat::S32Le => (32u16, HoundFormat::Int),
            SampleFormat::F32 => (32u16, HoundFormat::Float),
            SampleFormat::F64 => {
                return Err(FormatError::UnsupportedSampleFormat(sample_format));
            }
            SampleFormat::DsdU32Be | SampleFormat::DsdU32Le => {
                return Err(FormatError::UnsupportedSampleFormat(sample_format));
            }
        };

        let spec = WavSpec {
            channels: 2,
            sample_rate: sample_rate.get(),
            bits_per_sample,
            sample_format: hound_fmt,
        };

        let wav_writer = WavWriter::new(writer, spec).map_err(|e| {
            FormatError::EncoderError(format!("failed to initialize WAV writer: {e}"))
        })?;

        let quantizer = PcmQuantizer::new(sample_format, DitherType::None, None, 2)
            .ok_or(FormatError::UnsupportedSampleFormat(sample_format))?;

        Ok(Self {
            writer: wav_writer,
            sample_format,
            quantizer,
            frames_written: 0,
        })
    }

    /// Selects the dither applied before integer quantization (default: none, plain rounding).
    pub fn with_dither(mut self, dither: DitherType, seed: Option<u64>) -> Self {
        self.quantizer = PcmQuantizer::new(self.sample_format, dither, seed, 2)
            .expect("sample format validated in `new`");
        self
    }

    /// Writes stereo PCM samples in normalized `[-1.0, 1.0]` floating-point range.
    pub fn write_stereo_samples(&mut self, left: &[f64], right: &[f64]) -> Result<(), FormatError> {
        if left.len() != right.len() {
            return Err(FormatError::ChannelLengthMismatch {
                left: left.len(),
                right: right.len(),
            });
        }

        match self.sample_format {
            SampleFormat::S16Le
            | SampleFormat::S24Le
            | SampleFormat::S24_3Le
            | SampleFormat::S32Le => {
                for (&l, &r) in left.iter().zip(right.iter()) {
                    let il = self.quantizer.quantize(0, l);
                    let ir = self.quantizer.quantize(1, r);
                    // hound takes i16 for 16-bit WAV and i32 for 24/32-bit.
                    if self.sample_format == SampleFormat::S16Le {
                        self.writer.write_sample(il as i16).map_err(map_hound_err)?;
                        self.writer.write_sample(ir as i16).map_err(map_hound_err)?;
                    } else {
                        self.writer.write_sample(il).map_err(map_hound_err)?;
                        self.writer.write_sample(ir).map_err(map_hound_err)?;
                    }
                }
            }
            SampleFormat::F32 => {
                for (&l, &r) in left.iter().zip(right.iter()) {
                    self.writer.write_sample(l as f32).map_err(map_hound_err)?;
                    self.writer.write_sample(r as f32).map_err(map_hound_err)?;
                }
            }
            _ => return Err(FormatError::UnsupportedSampleFormat(self.sample_format)),
        }

        self.frames_written += left.len() as u64;
        Ok(())
    }

    /// Flushes samples and updates RIFF chunk sizes in the WAV header.
    pub fn finalize(self) -> Result<u64, FormatError> {
        let frames = self.frames_written;
        self.writer
            .finalize()
            .map_err(|e| FormatError::EncoderError(format!("failed to finalize WAV: {e}")))?;
        Ok(frames)
    }
}

fn map_hound_err(e: hound::Error) -> FormatError {
    FormatError::EncoderError(format!("WAV sample write error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_wav_writer_s24le_roundtrip() {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer =
            PcmWavWriter::new(&mut buffer, PcmRateHz::PCM_192000, SampleFormat::S24Le).unwrap();

        let left = vec![0.0, 0.5, -0.5, 0.9999];
        let right = vec![0.0, -0.25, 0.25, -0.9999];

        writer.write_stereo_samples(&left, &right).unwrap();
        let frames = writer.finalize().unwrap();
        assert_eq!(frames, 4);

        // Read back with hound
        buffer.set_position(0);
        let mut reader = hound::WavReader::new(buffer).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 192000);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.bits_per_sample, 24);

        let samples: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        assert_eq!(samples.len(), 8);
        assert_eq!(samples[0], 0); // L[0]
        assert_eq!(samples[1], 0); // R[0]
    }
}
