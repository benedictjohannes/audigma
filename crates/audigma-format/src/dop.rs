//! DoP (DSD over PCM v1.1) bit-packing across all three ALSA container alignments (`Design.md §5.2`).
//!
//! Each DoP frame carries 16 chronological 1-bit DSD samples (`2 bytes: HI, LO`, oldest bit in MSB)
//! per channel, prefixed by an 8-bit marker alternating `0x05` and `0xFA` on consecutive frames
//! (synchronized across stereo channels).
//!
//! Three ALSA container alignments are supported:
//! 1. `s32le` (32-bit container, MSB-justified `<< 8`): `0xMM_HI_LO_00`, written LE as `[0x00, LO, HI, MM]`.
//! 2. `s24le` (32-bit container, LSB-justified in 32-bit): `0x00_MM_HI_LO`, written LE as `[LO, HI, MM, 0x00]`.
//! 3. `s24_3le` (Packed 24-bit container, 3 bytes/sample): written LE as `[LO, HI, MM]`.

use crate::FormatError;
use audigma_dsp::{DSD_IDLE_BYTE, SampleFormat};
use serde::{Deserialize, Serialize};

/// First DoP v1.1 marker byte (`0x05`).
pub const DOP_MARKER_EVEN: u8 = 0x05;

/// Alternating DoP v1.1 marker byte (`0xFA`).
pub const DOP_MARKER_ODD: u8 = 0xFA;

/// Number of DSD bytes carried per channel per DoP frame (`16 bits = 2 bytes`).
pub const DSD_BYTES_PER_DOP_FRAME: usize = 2;

/// Supported ALSA container alignments for DoP v1.1 (`Design.md §5.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DopContainerFormat {
    /// 32-bit little-endian container, MSB-justified (`0xMM_HI_LO_00` -> `[0x00, LO, HI, MM]`).
    #[serde(rename = "s32le")]
    S32Le,
    /// 32-bit little-endian container, LSB-justified in 24 bits (`0x00_MM_HI_LO` -> `[LO, HI, MM, 0x00]`).
    #[serde(rename = "s24le")]
    S24Le,
    /// Packed 24-bit little-endian container (`3 bytes/sample` -> `[LO, HI, MM]`).
    #[serde(rename = "s24_3le")]
    S24_3Le,
}

impl DopContainerFormat {
    /// Converts a generic `SampleFormat` into a `DopContainerFormat` if it is a valid DoP carrier.
    pub const fn from_sample_format(fmt: SampleFormat) -> Option<Self> {
        match fmt {
            SampleFormat::S32Le => Some(Self::S32Le),
            SampleFormat::S24Le => Some(Self::S24Le),
            SampleFormat::S24_3Le => Some(Self::S24_3Le),
            _ => None,
        }
    }

    /// Returns the number of bytes per single-channel sample (`4` for `s32le`/`s24le`, `3` for `s24_3le`).
    #[inline]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::S32Le | Self::S24Le => 4,
            Self::S24_3Le => 3,
        }
    }

    /// Returns the number of bytes per stereo frame (`8` for `s32le`/`s24le`, `6` for `s24_3le`).
    #[inline]
    pub const fn bytes_per_stereo_frame(self) -> usize {
        self.bytes_per_sample() * 2
    }

    /// Encodes a single channel's `(marker, hi, lo)` DoP word into `out` (`3` or `4` bytes).
    #[inline]
    pub fn write_sample(self, marker: u8, hi: u8, lo: u8, out: &mut [u8]) {
        match self {
            Self::S32Le => {
                // 0xMM_HI_LO_00 in little-endian: [0x00, LO, HI, MM]
                out[0] = 0x00;
                out[1] = lo;
                out[2] = hi;
                out[3] = marker;
            }
            Self::S24Le => {
                // 0x00_MM_HI_LO in little-endian: [LO, HI, MM, 0x00]
                out[0] = lo;
                out[1] = hi;
                out[2] = marker;
                out[3] = 0x00;
            }
            Self::S24_3Le => {
                // Packed 24-bit in little-endian: [LO, HI, MM]
                out[0] = lo;
                out[1] = hi;
                out[2] = marker;
            }
        }
    }

    /// Decodes a single channel's `(marker, hi, lo)` DoP word from `src` (`3` or `4` bytes).
    #[inline]
    pub fn read_sample(self, src: &[u8]) -> Result<(u8, u8, u8), FormatError> {
        match self {
            Self::S32Le => {
                if src[0] != 0x00 {
                    return Err(FormatError::InvalidDopPaddingByte {
                        container: "s32le",
                        got: src[0],
                    });
                }
                Ok((src[3], src[2], src[1]))
            }
            Self::S24Le => {
                if src[3] != 0x00 {
                    return Err(FormatError::InvalidDopPaddingByte {
                        container: "s24le",
                        got: src[3],
                    });
                }
                Ok((src[2], src[1], src[0]))
            }
            Self::S24_3Le => Ok((src[2], src[1], src[0])),
        }
    }
}

/// Stateful DoP v1.1 stereo packer that preserves `0x05`/`0xFA` marker alternation
/// continuously across active audio blocks and idle silence frames (`Design.md §1.3`, `§4.5`).
#[derive(Debug, Clone)]
pub struct DopPacker {
    format: DopContainerFormat,
    next_marker: u8,
}

impl DopPacker {
    /// Creates a new `DopPacker` initialized to emit `0x05` on the first frame.
    pub const fn new(format: DopContainerFormat) -> Self {
        Self {
            format,
            next_marker: DOP_MARKER_EVEN,
        }
    }

    /// Returns the container alignment format (`S32Le`, `S24Le`, or `S24_3Le`).
    #[inline]
    pub const fn format(&self) -> DopContainerFormat {
        self.format
    }

    /// Returns the marker byte (`0x05` or `0xFA`) that will be emitted on the next frame.
    #[inline]
    pub const fn next_marker(&self) -> u8 {
        self.next_marker
    }

    #[inline]
    fn advance_marker(&mut self) -> u8 {
        let current = self.next_marker;
        self.next_marker = if current == DOP_MARKER_EVEN {
            DOP_MARKER_ODD
        } else {
            DOP_MARKER_EVEN
        };
        current
    }

    /// Packs chronological MSB-first stereo DSD byte slices (`left_msb_bytes`, `right_msb_bytes`)
    /// into interleaved DoP frames (`2 DSD bytes = 16 bits` per channel per frame).
    pub fn pack_stereo_frames(
        &mut self,
        left_msb_bytes: &[u8],
        right_msb_bytes: &[u8],
        out_frames: &mut [u8],
    ) -> Result<usize, FormatError> {
        if left_msb_bytes.len() != right_msb_bytes.len() {
            return Err(FormatError::ChannelLengthMismatch {
                left: left_msb_bytes.len(),
                right: right_msb_bytes.len(),
            });
        }
        if left_msb_bytes.len() % DSD_BYTES_PER_DOP_FRAME != 0 {
            return Err(FormatError::UnalignedDsdByteCount {
                count: left_msb_bytes.len(),
                required_multiple: DSD_BYTES_PER_DOP_FRAME,
            });
        }

        let num_frames = left_msb_bytes.len() / DSD_BYTES_PER_DOP_FRAME;
        let sample_bytes = self.format.bytes_per_sample();
        let frame_bytes = self.format.bytes_per_stereo_frame();
        let required_out_bytes = num_frames * frame_bytes;

        if out_frames.len() < required_out_bytes {
            return Err(FormatError::OutputBufferTooSmall {
                required: required_out_bytes,
                provided: out_frames.len(),
            });
        }

        for frame_idx in 0..num_frames {
            let marker = self.advance_marker();
            let in_off = frame_idx * DSD_BYTES_PER_DOP_FRAME;
            let out_off = frame_idx * frame_bytes;

            let l_hi = left_msb_bytes[in_off];
            let l_lo = left_msb_bytes[in_off + 1];
            let r_hi = right_msb_bytes[in_off];
            let r_lo = right_msb_bytes[in_off + 1];

            self.format.write_sample(
                marker,
                l_hi,
                l_lo,
                &mut out_frames[out_off..out_off + sample_bytes],
            );
            self.format.write_sample(
                marker,
                r_hi,
                r_lo,
                &mut out_frames[out_off + sample_bytes..out_off + frame_bytes],
            );
        }

        Ok(required_out_bytes)
    }

    /// Emits `num_frames` of balanced DSD silence (`0x69`) while maintaining continuous `0x05`/`0xFA` marker alternation.
    pub fn pack_stereo_silence_frames(
        &mut self,
        num_frames: usize,
        out_frames: &mut [u8],
    ) -> Result<usize, FormatError> {
        let sample_bytes = self.format.bytes_per_sample();
        let frame_bytes = self.format.bytes_per_stereo_frame();
        let required_out_bytes = num_frames * frame_bytes;

        if out_frames.len() < required_out_bytes {
            return Err(FormatError::OutputBufferTooSmall {
                required: required_out_bytes,
                provided: out_frames.len(),
            });
        }

        for frame_idx in 0..num_frames {
            let marker = self.advance_marker();
            let out_off = frame_idx * frame_bytes;
            self.format.write_sample(
                marker,
                DSD_IDLE_BYTE,
                DSD_IDLE_BYTE,
                &mut out_frames[out_off..out_off + sample_bytes],
            );
            self.format.write_sample(
                marker,
                DSD_IDLE_BYTE,
                DSD_IDLE_BYTE,
                &mut out_frames[out_off + sample_bytes..out_off + frame_bytes],
            );
        }

        Ok(required_out_bytes)
    }
}

/// Unpacks and verifies interleaved stereo DoP frames, checking that `0x05`/`0xFA` markers
/// are valid, synchronized across channels, and strictly alternating on every consecutive frame.
pub fn unpack_stereo_dop_frames(
    format: DopContainerFormat,
    in_frames: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), FormatError> {
    let sample_bytes = format.bytes_per_sample();
    let frame_bytes = format.bytes_per_stereo_frame();
    if in_frames.len() % frame_bytes != 0 {
        return Err(FormatError::UnalignedDsdByteCount {
            count: in_frames.len(),
            required_multiple: frame_bytes,
        });
    }

    let num_frames = in_frames.len() / frame_bytes;
    let mut left = Vec::with_capacity(num_frames * DSD_BYTES_PER_DOP_FRAME);
    let mut right = Vec::with_capacity(num_frames * DSD_BYTES_PER_DOP_FRAME);
    let mut expected_marker: Option<u8> = None;

    for (frame_idx, frame) in in_frames.chunks_exact(frame_bytes).enumerate() {
        let (l_marker, l_hi, l_lo) = format.read_sample(&frame[0..sample_bytes])?;
        let (r_marker, r_hi, r_lo) = format.read_sample(&frame[sample_bytes..frame_bytes])?;

        if l_marker != r_marker || (l_marker != DOP_MARKER_EVEN && l_marker != DOP_MARKER_ODD) {
            return Err(FormatError::InvalidDopMarker {
                frame_index: frame_idx,
                left_marker: l_marker,
                right_marker: r_marker,
            });
        }

        if let Some(exp) = expected_marker {
            if l_marker != exp {
                return Err(FormatError::InvalidDopMarker {
                    frame_index: frame_idx,
                    left_marker: l_marker,
                    right_marker: r_marker,
                });
            }
        }

        expected_marker = Some(if l_marker == DOP_MARKER_EVEN {
            DOP_MARKER_ODD
        } else {
            DOP_MARKER_EVEN
        });

        left.push(l_hi);
        left.push(l_lo);
        right.push(r_hi);
        right.push(r_lo);
    }

    Ok((left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dop_s32le_golden_vector() {
        // 2 DoP frames = 4 DSD bytes per channel
        // Frame 0: Left = [0x12, 0x34], Right = [0x56, 0x78], Marker = 0x05
        // Frame 1: Left = [0x9A, 0xBC], Right = [0xDE, 0xF0], Marker = 0xFA
        let left_dsd = [0x12, 0x34, 0x9A, 0xBC];
        let right_dsd = [0x56, 0x78, 0xDE, 0xF0];

        let mut packer = DopPacker::new(DopContainerFormat::S32Le);
        let mut out = [0u8; 16];
        let written = packer
            .pack_stereo_frames(&left_dsd, &right_dsd, &mut out)
            .unwrap();
        assert_eq!(written, 16);

        // Design.md §5.2: s32le is 0xMM_HI_LO_00 written little-endian as [0x00, LO, HI, MM]
        assert_eq!(
            out,
            [
                // Frame 0 (Marker 0x05)
                0x00, 0x34, 0x12, 0x05, // Left:  0x05_12_34_00
                0x00, 0x78, 0x56, 0x05, // Right: 0x05_56_78_00
                // Frame 1 (Marker 0xFA)
                0x00, 0xBC, 0x9A, 0xFA, // Left:  0xFA_9A_BC_00
                0x00, 0xF0, 0xDE, 0xFA, // Right: 0xFA_DE_F0_00
            ]
        );

        // Verify 32-bit little-endian integer word representations
        assert_eq!(
            u32::from_le_bytes(out[0..4].try_into().unwrap()),
            0x0512_3400
        );
        assert_eq!(
            u32::from_le_bytes(out[4..8].try_into().unwrap()),
            0x0556_7800
        );
        assert_eq!(
            u32::from_le_bytes(out[8..12].try_into().unwrap()),
            0xFA9A_BC00
        );
        assert_eq!(
            u32::from_le_bytes(out[12..16].try_into().unwrap()),
            0xFADE_F000
        );

        let (unpacked_l, unpacked_r) =
            unpack_stereo_dop_frames(DopContainerFormat::S32Le, &out).unwrap();
        assert_eq!(unpacked_l, left_dsd);
        assert_eq!(unpacked_r, right_dsd);
    }

    #[test]
    fn test_dop_s24le_golden_vector() {
        let left_dsd = [0x12, 0x34, 0x9A, 0xBC];
        let right_dsd = [0x56, 0x78, 0xDE, 0xF0];

        let mut packer = DopPacker::new(DopContainerFormat::S24Le);
        let mut out = [0u8; 16];
        let written = packer
            .pack_stereo_frames(&left_dsd, &right_dsd, &mut out)
            .unwrap();
        assert_eq!(written, 16);

        // Design.md §5.2: s24le is 0x00_MM_HI_LO written little-endian as [LO, HI, MM, 0x00]
        assert_eq!(
            out,
            [
                // Frame 0 (Marker 0x05)
                0x34, 0x12, 0x05, 0x00, // Left:  0x00_05_12_34
                0x78, 0x56, 0x05, 0x00, // Right: 0x00_05_56_78
                // Frame 1 (Marker 0xFA)
                0xBC, 0x9A, 0xFA, 0x00, // Left:  0x00_FA_9A_BC
                0xF0, 0xDE, 0xFA, 0x00, // Right: 0x00_FA_DE_F0
            ]
        );

        assert_eq!(
            u32::from_le_bytes(out[0..4].try_into().unwrap()),
            0x0005_1234
        );
        assert_eq!(
            u32::from_le_bytes(out[4..8].try_into().unwrap()),
            0x0005_5678
        );

        let (unpacked_l, unpacked_r) =
            unpack_stereo_dop_frames(DopContainerFormat::S24Le, &out).unwrap();
        assert_eq!(unpacked_l, left_dsd);
        assert_eq!(unpacked_r, right_dsd);
    }

    #[test]
    fn test_dop_s24_3le_golden_vector() {
        let left_dsd = [0x12, 0x34, 0x9A, 0xBC];
        let right_dsd = [0x56, 0x78, 0xDE, 0xF0];

        let mut packer = DopPacker::new(DopContainerFormat::S24_3Le);
        let mut out = [0u8; 12];
        let written = packer
            .pack_stereo_frames(&left_dsd, &right_dsd, &mut out)
            .unwrap();
        assert_eq!(written, 12);

        // Design.md §5.2: s24_3le is packed 3-byte written little-endian as [LO, HI, MM]
        assert_eq!(
            out,
            [
                // Frame 0 (Marker 0x05)
                0x34, 0x12, 0x05, // Left
                0x78, 0x56, 0x05, // Right
                // Frame 1 (Marker 0xFA)
                0xBC, 0x9A, 0xFA, // Left
                0xF0, 0xDE, 0xFA, // Right
            ]
        );

        let (unpacked_l, unpacked_r) =
            unpack_stereo_dop_frames(DopContainerFormat::S24_3Le, &out).unwrap();
        assert_eq!(unpacked_l, left_dsd);
        assert_eq!(unpacked_r, right_dsd);
    }

    #[test]
    fn test_dop_marker_continuity_across_active_and_silence_frames() {
        let mut packer = DopPacker::new(DopContainerFormat::S24_3Le);
        let mut stream = Vec::new();

        // 1 active frame (Marker 0x05)
        let mut f0 = [0u8; 6];
        packer
            .pack_stereo_frames(&[0xAA, 0xBB], &[0xCC, 0xDD], &mut f0)
            .unwrap();
        stream.extend_from_slice(&f0);
        assert_eq!(packer.next_marker(), DOP_MARKER_ODD);

        // 2 silence frames (Markers 0xFA then 0x05)
        let mut silence = [0u8; 12];
        packer.pack_stereo_silence_frames(2, &mut silence).unwrap();
        stream.extend_from_slice(&silence);
        assert_eq!(packer.next_marker(), DOP_MARKER_ODD);

        // 1 more active frame (Marker 0xFA)
        let mut f3 = [0u8; 6];
        packer
            .pack_stereo_frames(&[0x11, 0x22], &[0x33, 0x44], &mut f3)
            .unwrap();
        stream.extend_from_slice(&f3);

        // Unpacking the entire 4-frame sequence must succeed with valid marker alternation
        let (l, r) = unpack_stereo_dop_frames(DopContainerFormat::S24_3Le, &stream).unwrap();
        assert_eq!(l, [0xAA, 0xBB, 0x69, 0x69, 0x69, 0x69, 0x11, 0x22]);
        assert_eq!(r, [0xCC, 0xDD, 0x69, 0x69, 0x69, 0x69, 0x33, 0x44]);
    }
}
