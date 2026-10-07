//! ALSA Native `DSD_U32_BE` and `DSD_U32_LE` bit-packing and unpacking (`Design.md §5.2`).
//!
//! Packs 32 consecutive 1-bit DSD samples per channel (`4 MSB-first DSD bytes`) into each 32-bit
//! ALSA container word (`f_frame = f_bit / 32`).
//!
//! For `DSD_U32_BE` with `bitrev = false` (matching FiiO K17 `bitrev=0`):
//! - Given chronological MSB-first DSD bytes `[B0, B1, B2, B3]` for a channel (where bit 7 of `B0`
//!   is the oldest bit `t0` and bit 0 of `B3` is `t31`),
//! - The 32-bit container word in memory is `[B0, B1, B2, B3]` (`u32::from_be_bytes([B0, B1, B2, B3])`),
//! - Stereo frames interleave Left and Right 32-bit words: `[L0, L1, L2, L3, R0, R1, R2, R3]` (`8 bytes/frame`).

use crate::FormatError;
use audigma_dsp::DSD_IDLE_BYTE;

/// Number of DSD bytes per channel in a single `DSD_U32` frame (`32 bits = 4 bytes`).
pub const DSD_BYTES_PER_U32_WORD: usize = 4;

/// Number of bytes in a stereo `DSD_U32` frame (`4 bytes Left + 4 bytes Right = 8 bytes`).
pub const STEREO_DSD_U32_FRAME_BYTES: usize = 8;

/// Packs chronological MSB-first stereo DSD byte slices (`left_msb_bytes`, `right_msb_bytes`)
/// into interleaved ALSA `DSD_U32_BE` frames (`[L0, L1, L2, L3, R0, R1, R2, R3]`).
///
/// If `bitrev` is `false` (default `bitrev=0`, e.g. FiiO K17), each byte retains MSB-first bit order.
/// If `bitrev` is `true`, each byte's bits are reversed to LSB-first.
pub fn pack_stereo_dsd_u32_be(
    left_msb_bytes: &[u8],
    right_msb_bytes: &[u8],
    bitrev: bool,
    out_frames: &mut [u8],
) -> Result<usize, FormatError> {
    if left_msb_bytes.len() != right_msb_bytes.len() {
        return Err(FormatError::ChannelLengthMismatch {
            left: left_msb_bytes.len(),
            right: right_msb_bytes.len(),
        });
    }
    if left_msb_bytes.len() % DSD_BYTES_PER_U32_WORD != 0 {
        return Err(FormatError::UnalignedDsdByteCount {
            count: left_msb_bytes.len(),
            required_multiple: DSD_BYTES_PER_U32_WORD,
        });
    }

    let num_frames = left_msb_bytes.len() / DSD_BYTES_PER_U32_WORD;
    let required_out_bytes = num_frames * STEREO_DSD_U32_FRAME_BYTES;
    if out_frames.len() < required_out_bytes {
        return Err(FormatError::OutputBufferTooSmall {
            required: required_out_bytes,
            provided: out_frames.len(),
        });
    }

    for frame_idx in 0..num_frames {
        let in_off = frame_idx * DSD_BYTES_PER_U32_WORD;
        let out_off = frame_idx * STEREO_DSD_U32_FRAME_BYTES;

        for i in 0..DSD_BYTES_PER_U32_WORD {
            let l = left_msb_bytes[in_off + i];
            let r = right_msb_bytes[in_off + i];
            out_frames[out_off + i] = if bitrev { l.reverse_bits() } else { l };
            out_frames[out_off + 4 + i] = if bitrev { r.reverse_bits() } else { r };
        }
    }

    Ok(required_out_bytes)
}

/// Unpacks interleaved ALSA `DSD_U32_BE` stereo frames back into chronological MSB-first
/// `left_msb_bytes` and `right_msb_bytes`.
pub fn unpack_stereo_dsd_u32_be(
    in_frames: &[u8],
    bitrev: bool,
) -> Result<(Vec<u8>, Vec<u8>), FormatError> {
    if in_frames.len() % STEREO_DSD_U32_FRAME_BYTES != 0 {
        return Err(FormatError::UnalignedDsdByteCount {
            count: in_frames.len(),
            required_multiple: STEREO_DSD_U32_FRAME_BYTES,
        });
    }

    let num_frames = in_frames.len() / STEREO_DSD_U32_FRAME_BYTES;
    let mut left = Vec::with_capacity(num_frames * DSD_BYTES_PER_U32_WORD);
    let mut right = Vec::with_capacity(num_frames * DSD_BYTES_PER_U32_WORD);

    for frame in in_frames.chunks_exact(STEREO_DSD_U32_FRAME_BYTES) {
        for &b in &frame[0..4] {
            left.push(if bitrev { b.reverse_bits() } else { b });
        }
        for &b in &frame[4..8] {
            right.push(if bitrev { b.reverse_bits() } else { b });
        }
    }

    Ok((left, right))
}

/// Fills an output buffer with balanced `DSD_U32_BE` DSD silence (`0x69` bytes, never `0x00`).
pub fn fill_stereo_dsd_u32_be_silence(
    num_frames: usize,
    out_frames: &mut [u8],
) -> Result<usize, FormatError> {
    let required = num_frames * STEREO_DSD_U32_FRAME_BYTES;
    if out_frames.len() < required {
        return Err(FormatError::OutputBufferTooSmall {
            required,
            provided: out_frames.len(),
        });
    }
    out_frames[..required].fill(DSD_IDLE_BYTE);
    Ok(required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsd_u32_be_golden_vector_k17_bitrev0() {
        // Golden vector: 1 stereo frame = 32 DSD bits (4 bytes) per channel
        let left_dsd = [0xAA, 0x55, 0xF0, 0x0F];
        let right_dsd = [0x12, 0x34, 0x56, 0x78];

        let mut packed = [0u8; 8];
        let written = pack_stereo_dsd_u32_be(&left_dsd, &right_dsd, false, &mut packed).unwrap();
        assert_eq!(written, 8);

        // In DSD_U32_BE (bitrev=0), bytes [0..4] are Left BE word and [4..8] are Right BE word
        assert_eq!(packed, [0xAA, 0x55, 0xF0, 0x0F, 0x12, 0x34, 0x56, 0x78]);
        assert_eq!(
            u32::from_be_bytes(packed[0..4].try_into().unwrap()),
            0xAA55_F00F
        );
        assert_eq!(
            u32::from_be_bytes(packed[4..8].try_into().unwrap()),
            0x1234_5678
        );

        // Round-trip unpack
        let (unpacked_l, unpacked_r) = unpack_stereo_dsd_u32_be(&packed, false).unwrap();
        assert_eq!(unpacked_l, left_dsd);
        assert_eq!(unpacked_r, right_dsd);
    }

    #[test]
    fn test_dsd_u32_be_silence_never_zero() {
        let mut buf = [0u8; 16];
        let written = fill_stereo_dsd_u32_be_silence(2, &mut buf).unwrap();
        assert_eq!(written, 16);
        assert!(buf.iter().all(|&b| b == 0x69));
    }
}
