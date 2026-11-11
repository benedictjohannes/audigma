//! Lossless `.flac` file encoder using the vendored `audigma-flac` (`Design.md §5`).
//!
//! Writes 16, 24 or 32-bit integer PCM. 24 bit and below use the full encoder (LPC, stereo
//! decorrelation); 32 bit is coded per channel with FIXED predictors or verbatim. Sample rates
//! up to 1,048,575 Hz are representable in the STREAMINFO header; rates above 96 kHz and
//! 32-bit depth are outside the FLAC "subset", so player support varies.

use crate::FormatError;
use audigma_dsp::{DitherType, PcmQuantizer, PcmRateHz, SampleFormat};
use rusty_flac::Encoder;
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
            16 | 24 | 32 => bits_per_sample,
            other => {
                return Err(FormatError::EncoderError(format!(
                    "unsupported FLAC bits_per_sample: {other} (must be 16, 24 or 32)"
                )));
            }
        };

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
        let format = match bits {
            16 => SampleFormat::S16Le,
            24 => SampleFormat::S24Le,
            _ => SampleFormat::S32Le,
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
        let mut encoder = Encoder::new(self.sample_rate.get(), 2, self.bits_per_sample as u32)
            .map_err(|e| FormatError::EncoderError(format!("FLAC encoder config error: {e}")))?;
        encoder.set_compression_level(8);
        encoder
            .push_interleaved(&self.interleaved_samples)
            .map_err(|e| FormatError::EncoderError(format!("FLAC encoding failed: {e}")))?;
        let bytes = encoder.finish();

        self.writer
            .write_all(&bytes)
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

    fn test_signal(frames: usize) -> (Vec<f64>, Vec<f64>) {
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = i as f64 / 96_000.0;
            l.push((2.0 * std::f64::consts::PI * 997.0 * t).sin() * 0.9);
            r.push((2.0 * std::f64::consts::PI * 3001.0 * t).sin() * 0.3);
        }
        (l, r)
    }

    /// Encode, then decode with the vendored decoder and compare against the quantizer output
    /// bit for bit, for every supported depth with dither off and on.
    #[test]
    fn roundtrip_matches_quantizer_at_16_24_32_bit() {
        let (l, r) = test_signal(10_000);
        for bits in [16usize, 24, 32] {
            for (dither, seed) in [(DitherType::None, None), (DitherType::Tpdf, Some(12345))] {
                let mut out = Vec::new();
                let mut enc = FlacEncoder::new(&mut out, PcmRateHz::PCM_96000, bits)
                    .unwrap()
                    .with_dither(dither, seed);
                enc.write_stereo_samples(&l, &r).unwrap();
                enc.finalize().unwrap();

                let mut q = FlacEncoder::<Vec<u8>>::quantizer_for(bits, dither, seed);
                let want_l: Vec<i32> = l.iter().map(|&x| q.quantize(0, x)).collect();
                let want_r: Vec<i32> = r.iter().map(|&x| q.quantize(1, x)).collect();

                let (info, planes) = rusty_flac::decode(&out).unwrap();
                assert_eq!(info.bits_per_sample as usize, bits);
                assert_eq!(planes[0], want_l, "{bits} bit {dither:?} left");
                assert_eq!(planes[1], want_r, "{bits} bit {dither:?} right");
            }
        }
    }

    /// The reader used by `convert`/`play` (symphonia) must accept what the writer produces.
    #[test]
    fn symphonia_reads_32_bit_output() {
        let (l, r) = test_signal(6_000);
        let mut out = Vec::new();
        let mut enc = FlacEncoder::new(&mut out, PcmRateHz::PCM_96000, 32).unwrap();
        enc.write_stereo_samples(&l, &r).unwrap();
        enc.finalize().unwrap();

        let mut dec =
            crate::AudioDecoder::try_new(std::io::Cursor::new(out), Some("flac")).unwrap();
        let (mut dl, mut dr) = (Vec::new(), Vec::new());
        let mut n = 0;
        loop {
            let got = dec.read_stereo_chunk(&mut dl, &mut dr).unwrap();
            if got == 0 {
                break;
            }
            n += got;
        }
        assert_eq!(n, l.len());
        for i in 0..l.len() {
            assert!((dl[i] - l[i]).abs() < 1e-8, "L {i}: {} vs {}", dl[i], l[i]);
            assert!((dr[i] - r[i]).abs() < 1e-8, "R {i}");
        }
    }

    #[test]
    fn rejects_unsupported_depth() {
        assert!(FlacEncoder::new(Vec::new(), PcmRateHz::PCM_44100, 20).is_err());
    }
}
