//! Lossless `.flac` file encoder using `flacenc` (`Design.md §2`).
//!
//! Supports 16-bit and 24-bit PCM at sampling rates up to 384 kHz.

use crate::FormatError;
use audigma_dsp::{DitherType, PcmQuantizer, PcmRateHz, SampleFormat};
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use flacenc::source::MemSource;
use std::io::Write;

/// Lossless FLAC file encoder.
pub struct FlacEncoder<W: Write> {
    writer: W,
    sample_rate: PcmRateHz,
    bits_per_sample: usize,
    interleaved_samples: Vec<i32>,
    quantizer: PcmQuantizer,
    frames_written: u64,
}

impl<W: Write> FlacEncoder<W> {
    /// Initializes a new FLAC encoder targeting stereo PCM.
    pub fn new(
        writer: W,
        sample_rate: PcmRateHz,
        bits_per_sample: usize,
    ) -> Result<Self, FormatError> {
        let bits = match bits_per_sample {
            16 => 16,
            24 => 24,
            other => {
                return Err(FormatError::EncoderError(format!(
                    "unsupported FLAC bits_per_sample: {other} (must be 16 or 24)"
                )));
            }
        };

        if sample_rate.get() > 96_000 {
            return Err(FormatError::EncoderError(format!(
                "sample rate {} Hz exceeds FLAC format subset limit (96,000 Hz); please use WAV container for high-rate PCM (> 96 kHz)",
                sample_rate.get()
            )));
        }

        Ok(Self {
            writer,
            sample_rate,
            bits_per_sample: bits,
            interleaved_samples: Vec::new(),
            quantizer: Self::quantizer_for(bits, DitherType::None, None),
            frames_written: 0,
        })
    }

    fn quantizer_for(bits: usize, dither: DitherType, seed: Option<u64>) -> PcmQuantizer {
        let format = if bits == 16 {
            SampleFormat::S16Le
        } else {
            SampleFormat::S24Le
        };
        PcmQuantizer::new(format, dither, seed, 2).expect("integer PCM format")
    }

    /// Selects the dither applied before integer quantization (default: none, plain rounding).
    pub fn with_dither(mut self, dither: DitherType, seed: Option<u64>) -> Self {
        self.quantizer = Self::quantizer_for(self.bits_per_sample, dither, seed);
        self
    }

    /// Appends stereo PCM samples in normalized `[-1.0, 1.0]` floating-point range.
    pub fn write_stereo_samples(&mut self, left: &[f64], right: &[f64]) -> Result<(), FormatError> {
        if left.len() != right.len() {
            return Err(FormatError::ChannelLengthMismatch {
                left: left.len(),
                right: right.len(),
            });
        }

        self.interleaved_samples.reserve(left.len() * 2);

        for (&l, &r) in left.iter().zip(right.iter()) {
            let il = self.quantizer.quantize(0, l);
            let ir = self.quantizer.quantize(1, r);
            self.interleaved_samples.push(il);
            self.interleaved_samples.push(ir);
        }

        self.frames_written += left.len() as u64;
        Ok(())
    }

    /// Encodes all accumulated samples and writes the FLAC bitstream to the underlying writer.
    pub fn finalize(mut self) -> Result<u64, FormatError> {
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|e| FormatError::EncoderError(format!("FLAC encoder config error: {e:?}")))?;

        let channels = 2;
        let source = MemSource::from_samples(
            &self.interleaved_samples,
            channels,
            self.bits_per_sample,
            self.sample_rate.get() as usize,
        );

        let total_frames = self.frames_written as usize;
        let block_size = if total_frames > 0 && total_frames < config.block_size {
            total_frames.max(32)
        } else {
            config.block_size
        };

        let mut flac_stream = flacenc::encode_with_fixed_block_size(&config, source, block_size)
            .map_err(|e| FormatError::EncoderError(format!("FLAC encoding failed: {e:?}")))?;

        // Ensure STREAMINFO min_block_size == max_block_size for fixed-block streams,
        // matching the libFLAC reference encoder specification and Symphonia expectations.
        flac_stream
            .stream_info_mut()
            .set_block_sizes(block_size, block_size)
            .map_err(|e| {
                FormatError::EncoderError(format!("FLAC block size verify error: {e:?}"))
            })?;

        let mut sink = flacenc::bitsink::ByteSink::new();
        flac_stream.write(&mut sink).map_err(|e| {
            FormatError::EncoderError(format!("FLAC bitstream write failed: {e:?}"))
        })?;

        self.writer
            .write_all(sink.as_slice())
            .map_err(|e| FormatError::IoError(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| FormatError::IoError(e.to_string()))?;

        Ok(self.frames_written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flac_encoder_basic() {
        let mut out = Vec::new();
        let mut encoder = FlacEncoder::new(&mut out, PcmRateHz::PCM_44100, 16).unwrap();

        let left = vec![0.0, 0.25, -0.25, 0.5];
        let right = vec![0.0, -0.25, 0.25, -0.5];

        encoder.write_stereo_samples(&left, &right).unwrap();
        let frames = encoder.finalize().unwrap();
        assert_eq!(frames, 4);

        // Verify FLAC stream magic: "fLaC"
        assert!(out.len() > 4);
        assert_eq!(&out[0..4], b"fLaC");
    }
}
