//! Shared PCM quantizer (`Design.md §4.2`): scaling, optional TPDF dither, clamping and
//! rounding of normalized `f64` samples to the target integer sample format.
//!
//! One implementation serves the live loop and every offline writer (WAV, FLAC, raw), so
//! `target.dither` / `dither_tpdf_seed` behave identically everywhere.

use crate::config::{DitherType, SampleFormat};
use crate::modulator::TpdfDither;

/// Peak amplitude (in LSB) of the TPDF dither added before rounding.
pub const PCM_DITHER_AMPLITUDE_LSB: f64 = 0.5;

/// Seed used when `dither_tpdf_seed` is not configured.
pub const DEFAULT_PCM_DITHER_SEED: u64 = 12345;

/// Per-channel PCM quantizer with independent dither streams.
#[derive(Debug, Clone)]
pub struct PcmQuantizer {
    format: SampleFormat,
    /// Empty when dither is off or the format is floating point.
    dithers: Vec<TpdfDither>,
}

impl PcmQuantizer {
    /// Creates a quantizer for `channels` channels. Integer formats are dithered
    /// iff `dither == Tpdf`; float formats are written verbatim.
    ///
    /// Returns `None` for formats that are not PCM (`DsdU32*`).
    pub fn new(
        format: SampleFormat,
        dither: DitherType,
        seed: Option<u64>,
        channels: usize,
    ) -> Option<Self> {
        if matches!(format, SampleFormat::DsdU32Be | SampleFormat::DsdU32Le) {
            return None;
        }
        let dithers = if dither == DitherType::Tpdf && Self::is_integer(format) {
            let base = seed.unwrap_or(DEFAULT_PCM_DITHER_SEED);
            (0..channels)
                .map(|c| {
                    // Decorrelate channels: nearby xorshift seeds would otherwise start close.
                    let offset = (c as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    TpdfDither::new(base ^ offset, PCM_DITHER_AMPLITUDE_LSB)
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(Self { format, dithers })
    }

    fn is_integer(format: SampleFormat) -> bool {
        matches!(
            format,
            SampleFormat::S16Le | SampleFormat::S24Le | SampleFormat::S24_3Le | SampleFormat::S32Le
        )
    }

    /// Target sample format.
    pub fn format(&self) -> SampleFormat {
        self.format
    }

    /// Bytes occupied by one sample of the target format.
    pub fn bytes_per_sample(&self) -> usize {
        match self.format {
            SampleFormat::S16Le => 2,
            SampleFormat::S24_3Le => 3,
            SampleFormat::F64 => 8,
            _ => 4,
        }
    }

    /// Quantizes one sample to the integer code of the target format
    /// (S16: ±15 bits, S24*: ±23 bits, S32: ±31 bits). Float formats return `0`.
    #[inline]
    pub fn quantize(&mut self, channel: usize, x: f64) -> i32 {
        let (scale, lo, hi) = match self.format {
            SampleFormat::S16Le => (32767.0, -32768.0, 32767.0),
            SampleFormat::S24Le | SampleFormat::S24_3Le => (8388607.0, -8388608.0, 8388607.0),
            SampleFormat::S32Le => (2147483647.0, -2147483648.0, 2147483647.0),
            _ => return 0,
        };
        let d = match self.dithers.get_mut(channel) {
            Some(dither) => dither.next_sample(),
            None => 0.0,
        };
        (x * scale + d).clamp(lo, hi).round() as i32
    }

    /// Quantizes `src` for one channel, appending integer codes to `dst`.
    pub fn quantize_slice(&mut self, channel: usize, src: &[f64], dst: &mut Vec<i32>) {
        dst.reserve(src.len());
        for &x in src {
            let q = self.quantize(channel, x);
            dst.push(q);
        }
    }

    /// Appends one sample's little-endian bytes to `out`.
    #[inline]
    fn push_sample(&mut self, channel: usize, x: f64, out: &mut Vec<u8>) {
        match self.format {
            SampleFormat::F32 => out.extend_from_slice(&(x as f32).to_le_bytes()),
            SampleFormat::F64 => out.extend_from_slice(&x.to_le_bytes()),
            SampleFormat::S16Le => {
                out.extend_from_slice(&(self.quantize(channel, x) as i16).to_le_bytes());
            }
            SampleFormat::S24_3Le => {
                let q = self.quantize(channel, x).to_le_bytes();
                out.extend_from_slice(&q[..3]);
            }
            _ => out.extend_from_slice(&self.quantize(channel, x).to_le_bytes()),
        }
    }

    /// Interleaves `channels` (planar, equal length) frame by frame and appends the
    /// quantized little-endian bytes to `out`. Returns the number of frames written.
    pub fn write_interleaved<S: AsRef<[f64]>>(
        &mut self,
        channels: &[S],
        out: &mut Vec<u8>,
    ) -> usize {
        let frames = channels.first().map_or(0, |c| c.as_ref().len());
        out.reserve(frames * channels.len() * self.bytes_per_sample());
        for i in 0..frames {
            for (c, ch) in channels.iter().enumerate() {
                self.push_sample(c, ch.as_ref()[i], out);
            }
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(format: SampleFormat, dither: DitherType) -> PcmQuantizer {
        PcmQuantizer::new(format, dither, Some(7), 2).unwrap()
    }

    #[test]
    fn rejects_dsd_formats() {
        assert!(PcmQuantizer::new(SampleFormat::DsdU32Be, DitherType::Tpdf, None, 2).is_none());
    }

    #[test]
    fn undithered_scaling_and_clamp() {
        let mut s16 = q(SampleFormat::S16Le, DitherType::None);
        assert_eq!(s16.quantize(0, 1.0), 32767);
        assert_eq!(s16.quantize(0, -1.0), -32767);
        assert_eq!(s16.quantize(0, 5.0), 32767);
        assert_eq!(s16.quantize(0, -5.0), -32768);
        let mut s24 = q(SampleFormat::S24Le, DitherType::None);
        assert_eq!(s24.quantize(0, 1.0), 8388607);
        let mut s32 = q(SampleFormat::S32Le, DitherType::None);
        assert_eq!(s32.quantize(0, 1.0), i32::MAX);
        assert_eq!(s32.quantize(0, -9.0), i32::MIN);
    }

    #[test]
    fn dither_is_deterministic_and_dithers_rounding() {
        let mut a = q(SampleFormat::S16Le, DitherType::Tpdf);
        let mut b = q(SampleFormat::S16Le, DitherType::Tpdf);
        // A mid-LSB input lets the dither decide between the two neighbouring codes.
        let x = 0.5 / 32767.0;
        let (mut zeros, mut ones) = (0, 0);
        for _ in 0..10_000 {
            let qa = a.quantize(0, x);
            assert_eq!(qa, b.quantize(0, x));
            match qa {
                0 => zeros += 1,
                1 => ones += 1,
                other => panic!("dither exceeded its bound: {other}"),
            }
        }
        assert!(zeros > 1000 && ones > 1000, "zeros={zeros} ones={ones}");
    }

    #[test]
    fn dither_decorrelates_channels() {
        let mut s = q(SampleFormat::S16Le, DitherType::Tpdf);
        let mut same = 0;
        for _ in 0..4096 {
            // Mid-LSB input makes the dither decide the rounding direction.
            let x = 0.5 / 32767.0;
            if s.quantize(0, x) == s.quantize(1, x) {
                same += 1;
            }
        }
        assert!(same < 4096, "channels share one dither stream");
    }

    #[test]
    fn none_dither_has_no_noise() {
        let mut s = q(SampleFormat::S16Le, DitherType::None);
        for _ in 0..1000 {
            assert_eq!(s.quantize(0, 0.0), 0);
        }
    }

    #[test]
    fn float_formats_ignore_dither() {
        let mut f = q(SampleFormat::F32, DitherType::Tpdf);
        let mut out = Vec::new();
        f.write_interleaved(&[[0.25f64], [-0.5]], &mut out);
        assert_eq!(&out[..4], &0.25f32.to_le_bytes());
        assert_eq!(&out[4..], &(-0.5f32).to_le_bytes());
    }

    #[test]
    fn interleaved_byte_layout() {
        let mut s24_3 = q(SampleFormat::S24_3Le, DitherType::None);
        let mut out = Vec::new();
        let frames = s24_3.write_interleaved(&[vec![1.0, 0.0], vec![-1.0, 0.0]], &mut out);
        assert_eq!(frames, 2);
        assert_eq!(out.len(), 12);
        assert_eq!(&out[..3], &[0xFF, 0xFF, 0x7F]);
        assert_eq!(&out[3..6], &[0x01, 0x00, 0x80]);

        let mut s24 = q(SampleFormat::S24Le, DitherType::None);
        out.clear();
        s24.write_interleaved(&[[1.0f64], [0.0]], &mut out);
        assert_eq!(out, [0xFF, 0xFF, 0x7F, 0x00, 0, 0, 0, 0]);
    }
}
