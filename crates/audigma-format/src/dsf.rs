//! Sony DSF (`.dsf`) v1.01 container header, 4096-byte block interleaver, and padding modes (`Design.md §5.2`).
//!
//! Sony DSF v1.01 layout:
//! - `DSD ` chunk (28 bytes)
//! - `fmt ` chunk (52 bytes)
//! - `data` chunk (`12 bytes` header + interleaved `L[4096], R[4096]` channel blocks)
//!
//! By default (`DsfBitOrder::LsbFirst`), DSF writes `bits_per_sample = 1` (LSB-first within each byte,
//! per Sony DSF v1.01 / MPD / ffmpeg). `DsfBitOrder::MsbFirst` (`bits_per_sample = 8`) is also supported.
//!
//! Final 4096-byte block padding supports both:
//! - `DsfPadMode::SpecZero` (`0x00` fill with exact `sample_count`)
//! - `DsfPadMode::DsdSilence` (`0x69` balanced DSD silence fill to prevent end-of-track pops on naive players)

use crate::FormatError;
use audigma_dsp::{DSD_IDLE_BYTE, DsdBitRateHz};
use serde::{Deserialize, Serialize};

/// Fixed per-channel block size in bytes mandated by Sony DSF v1.01 (`4096 bytes = 32,768 bits`).
pub const DSF_BLOCK_SIZE_PER_CHANNEL: usize = 4096;

/// Total size of the fixed DSF headers (`28` DSD + `52` fmt + `12` data header = `92 bytes`).
pub const DSF_HEADER_TOTAL_BYTES: usize = 28 + 52 + 12;

/// Bit ordering inside each DSF payload byte (`fmt ` chunk `bits_per_sample`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DsfBitOrder {
    /// Sony DSF v1.01 standard (`bits_per_sample = 1`): oldest chronological DSD sample in bit 0 (LSB).
    #[default]
    #[serde(rename = "lsb-first")]
    LsbFirst,
    /// Alternative MSB-first (`bits_per_sample = 8`): oldest chronological DSD sample in bit 7 (MSB).
    #[serde(rename = "msb-first")]
    MsbFirst,
}

impl DsfBitOrder {
    /// Returns the `bits_per_sample` field value for the DSF `fmt ` chunk (`1` or `8`).
    #[inline]
    pub const fn dsf_bits_per_sample_field(self) -> u32 {
        match self {
            Self::LsbFirst => 1,
            Self::MsbFirst => 8,
        }
    }

    /// Converts an MSB-first chronological DSD byte into the target DSF byte representation.
    #[inline]
    pub const fn encode_from_msb_byte(self, msb_byte: u8) -> u8 {
        match self {
            Self::LsbFirst => msb_byte.reverse_bits(),
            Self::MsbFirst => msb_byte,
        }
    }

    /// Converts a DSF byte into the canonical MSB-first chronological DSD byte representation.
    #[inline]
    pub const fn decode_to_msb_byte(self, dsf_byte: u8) -> u8 {
        match self {
            Self::LsbFirst => dsf_byte.reverse_bits(),
            Self::MsbFirst => dsf_byte,
        }
    }
}

/// Padding strategy for unused trailing bytes in the final 4096-byte DSF block (`Design.md §5.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum DsfPadMode {
    /// Sony DSF v1.01 §4 default: fill unused trailing bytes with `0x00` and record exact `sample_count`.
    #[default]
    #[serde(rename = "spec-zero")]
    SpecZero,
    /// Fill unused trailing bytes with balanced DSD silence (`0x69`) to prevent pops on players that ignore `sample_count`.
    #[serde(rename = "dsd-silence")]
    DsdSilence,
}

impl DsfPadMode {
    /// Returns the pad byte written to unused trailing positions in the final 4096-byte block.
    #[inline]
    pub const fn pad_byte(self, bit_order: DsfBitOrder) -> u8 {
        match self {
            Self::SpecZero => 0x00,
            Self::DsdSilence => bit_order.encode_from_msb_byte(DSD_IDLE_BYTE),
        }
    }
}

/// Parsed or prepared Sony DSF v1.01 file header metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DsfHeader {
    pub bit_rate: DsdBitRateHz,
    pub channels: u32,
    pub bit_order: DsfBitOrder,
    /// Exact number of valid 1-bit DSD samples per channel (excluding trailing block padding).
    pub sample_count_per_channel: u64,
    /// Total byte length of the interleaved `L[4096], R[4096]` payload blocks.
    pub data_payload_bytes: u64,
}

impl DsfHeader {
    /// Encodes the 92-byte DSF header (`DSD ` + `fmt ` + `data` chunk header).
    pub fn to_bytes(&self) -> [u8; DSF_HEADER_TOTAL_BYTES] {
        let mut buf = [0u8; DSF_HEADER_TOTAL_BYTES];
        let total_file_size = (DSF_HEADER_TOTAL_BYTES as u64) + self.data_payload_bytes;
        let data_chunk_size = 12u64 + self.data_payload_bytes;

        // 1. `DSD ` chunk (28 bytes)
        buf[0..4].copy_from_slice(b"DSD ");
        buf[4..12].copy_from_slice(&28u64.to_le_bytes());
        buf[12..20].copy_from_slice(&total_file_size.to_le_bytes());
        buf[20..28].copy_from_slice(&0u64.to_le_bytes()); // metadata_offset = 0

        // 2. `fmt ` chunk (52 bytes, offset 28..80)
        let channel_type = if self.channels == 1 { 1u32 } else { 2u32 };
        buf[28..32].copy_from_slice(b"fmt ");
        buf[32..40].copy_from_slice(&52u64.to_le_bytes());
        buf[40..44].copy_from_slice(&1u32.to_le_bytes()); // format_version = 1
        buf[44..48].copy_from_slice(&0u32.to_le_bytes()); // format_id = 0 (DSD raw)
        buf[48..52].copy_from_slice(&channel_type.to_le_bytes());
        buf[52..56].copy_from_slice(&self.channels.to_le_bytes());
        buf[56..60].copy_from_slice(&self.bit_rate.get().to_le_bytes());
        buf[60..64].copy_from_slice(&self.bit_order.dsf_bits_per_sample_field().to_le_bytes());
        buf[64..72].copy_from_slice(&self.sample_count_per_channel.to_le_bytes());
        buf[72..76].copy_from_slice(&(DSF_BLOCK_SIZE_PER_CHANNEL as u32).to_le_bytes());
        buf[76..80].copy_from_slice(&0u32.to_le_bytes()); // reserved = 0

        // 3. `data` chunk header (12 bytes, offset 80..92)
        buf[80..84].copy_from_slice(b"data");
        buf[84..92].copy_from_slice(&data_chunk_size.to_le_bytes());

        buf
    }

    /// Parses a 92-byte DSF header and validates Sony DSF v1.01 invariants.
    pub fn from_bytes(buf: &[u8]) -> Result<Self, FormatError> {
        if buf.len() < DSF_HEADER_TOTAL_BYTES {
            return Err(FormatError::InvalidDsfHeader(
                "file shorter than 92-byte DSF header",
            ));
        }
        if &buf[0..4] != b"DSD " {
            return Err(FormatError::InvalidDsfHeader("missing 'DSD ' magic"));
        }
        let dsd_chunk_size = u64::from_le_bytes(buf[4..12].try_into().unwrap());
        if dsd_chunk_size != 28 {
            return Err(FormatError::InvalidDsfHeader("DSD chunk size must be 28"));
        }

        if &buf[28..32] != b"fmt " {
            return Err(FormatError::InvalidDsfHeader("missing 'fmt ' chunk magic"));
        }
        let fmt_chunk_size = u64::from_le_bytes(buf[32..40].try_into().unwrap());
        if fmt_chunk_size != 52 {
            return Err(FormatError::InvalidDsfHeader("fmt chunk size must be 52"));
        }

        let channels = u32::from_le_bytes(buf[52..56].try_into().unwrap());
        let rate_raw = u32::from_le_bytes(buf[56..60].try_into().unwrap());
        let bit_rate = DsdBitRateHz::new(rate_raw).ok_or(FormatError::InvalidDsfHeader(
            "invalid DSD sampling frequency",
        ))?;
        let bits_per_sample = u32::from_le_bytes(buf[60..64].try_into().unwrap());
        let bit_order = match bits_per_sample {
            1 => DsfBitOrder::LsbFirst,
            8 => DsfBitOrder::MsbFirst,
            _ => {
                return Err(FormatError::InvalidDsfHeader(
                    "bits_per_sample must be 1 or 8",
                ));
            }
        };
        let sample_count_per_channel = u64::from_le_bytes(buf[64..72].try_into().unwrap());
        let block_size = u32::from_le_bytes(buf[72..76].try_into().unwrap()) as usize;
        if block_size != DSF_BLOCK_SIZE_PER_CHANNEL {
            return Err(FormatError::InvalidDsfHeader(
                "block_size_per_channel must be 4096",
            ));
        }

        if &buf[80..84] != b"data" {
            return Err(FormatError::InvalidDsfHeader("missing 'data' chunk magic"));
        }
        let data_chunk_size = u64::from_le_bytes(buf[84..92].try_into().unwrap());
        if data_chunk_size < 12 {
            return Err(FormatError::InvalidDsfHeader("data chunk size < 12"));
        }

        Ok(Self {
            bit_rate,
            channels,
            bit_order,
            sample_count_per_channel,
            data_payload_bytes: data_chunk_size - 12,
        })
    }
}

/// Packs chronological MSB-first stereo DSD byte slices (`left_msb_bytes`, `right_msb_bytes`)
/// into interleaved 4096-byte per-channel DSF blocks (`L[4096], R[4096]`), applying `bit_order`
/// and `pad_mode` (`Design.md §5.2`).
pub fn pack_stereo_dsf_blocks(
    left_msb_bytes: &[u8],
    right_msb_bytes: &[u8],
    bit_order: DsfBitOrder,
    pad_mode: DsfPadMode,
) -> Result<Vec<u8>, FormatError> {
    if left_msb_bytes.len() != right_msb_bytes.len() {
        return Err(FormatError::ChannelLengthMismatch {
            left: left_msb_bytes.len(),
            right: right_msb_bytes.len(),
        });
    }

    let total_channel_bytes = left_msb_bytes.len();
    let num_blocks = total_channel_bytes.div_ceil(DSF_BLOCK_SIZE_PER_CHANNEL);
    let pad_byte = pad_mode.pad_byte(bit_order);
    let mut out = Vec::with_capacity(num_blocks * DSF_BLOCK_SIZE_PER_CHANNEL * 2);

    for block_idx in 0..num_blocks {
        let start = block_idx * DSF_BLOCK_SIZE_PER_CHANNEL;
        let end = (start + DSF_BLOCK_SIZE_PER_CHANNEL).min(total_channel_bytes);
        let valid_len = end - start;

        // Left 4096-byte block
        for &b in &left_msb_bytes[start..end] {
            out.push(bit_order.encode_from_msb_byte(b));
        }
        if valid_len < DSF_BLOCK_SIZE_PER_CHANNEL {
            out.resize(
                out.len() + (DSF_BLOCK_SIZE_PER_CHANNEL - valid_len),
                pad_byte,
            );
        }

        // Right 4096-byte block
        for &b in &right_msb_bytes[start..end] {
            out.push(bit_order.encode_from_msb_byte(b));
        }
        if valid_len < DSF_BLOCK_SIZE_PER_CHANNEL {
            out.resize(
                out.len() + (DSF_BLOCK_SIZE_PER_CHANNEL - valid_len),
                pad_byte,
            );
        }
    }

    Ok(out)
}

/// Encodes a complete Sony DSF v1.01 stereo file in memory (`header + interleaved 4096-byte blocks`).
pub fn pack_stereo_dsf_file(
    bit_rate: DsdBitRateHz,
    left_msb_bytes: &[u8],
    right_msb_bytes: &[u8],
    bit_order: DsfBitOrder,
    pad_mode: DsfPadMode,
) -> Result<Vec<u8>, FormatError> {
    let payload = pack_stereo_dsf_blocks(left_msb_bytes, right_msb_bytes, bit_order, pad_mode)?;
    let header = DsfHeader {
        bit_rate,
        channels: 2,
        bit_order,
        sample_count_per_channel: (left_msb_bytes.len() as u64) * 8,
        data_payload_bytes: payload.len() as u64,
    };

    let mut file_bytes = Vec::with_capacity(DSF_HEADER_TOTAL_BYTES + payload.len());
    file_bytes.extend_from_slice(&header.to_bytes());
    file_bytes.extend_from_slice(&payload);
    Ok(file_bytes)
}

/// Parses a complete Sony DSF v1.01 stereo file and extracts the header and exact trimmed
/// chronological MSB-first Left/Right byte streams (respecting `sample_count_per_channel`).
pub fn parse_stereo_dsf_file(
    file_bytes: &[u8],
) -> Result<(DsfHeader, Vec<u8>, Vec<u8>), FormatError> {
    let header = DsfHeader::from_bytes(file_bytes)?;
    if header.channels != 2 {
        return Err(FormatError::InvalidDsfHeader(
            "expected stereo (2-channel) DSF file",
        ));
    }

    let payload = &file_bytes[DSF_HEADER_TOTAL_BYTES..];
    if (payload.len() as u64) < header.data_payload_bytes {
        return Err(FormatError::InvalidDsfHeader(
            "truncated DSF data chunk payload",
        ));
    }

    let payload = &payload[..header.data_payload_bytes as usize];
    let stereo_block_bytes = DSF_BLOCK_SIZE_PER_CHANNEL * 2;
    if payload.len() % stereo_block_bytes != 0 {
        return Err(FormatError::InvalidDsfHeader(
            "DSF data payload is not a multiple of 8192 bytes (2 * 4096)",
        ));
    }

    let valid_bytes_per_channel = (header.sample_count_per_channel / 8) as usize;
    let mut left = Vec::with_capacity(valid_bytes_per_channel);
    let mut right = Vec::with_capacity(valid_bytes_per_channel);

    for stereo_block in payload.chunks_exact(stereo_block_bytes) {
        let l_block = &stereo_block[0..DSF_BLOCK_SIZE_PER_CHANNEL];
        let r_block = &stereo_block[DSF_BLOCK_SIZE_PER_CHANNEL..stereo_block_bytes];

        let remaining = valid_bytes_per_channel.saturating_sub(left.len());
        let take = remaining.min(DSF_BLOCK_SIZE_PER_CHANNEL);

        for &b in &l_block[..take] {
            left.push(header.bit_order.decode_to_msb_byte(b));
        }
        for &b in &r_block[..take] {
            right.push(header.bit_order.decode_to_msb_byte(b));
        }
    }

    Ok((header, left, right))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dsf_lsb_first_default_and_spec_zero_padding() {
        // 5000 bytes per channel -> spans 2 blocks of 4096 bytes (block 0: 4096 valid, block 1: 904 valid + 3192 pad)
        let left: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
        let right: Vec<u8> = (0..5000).map(|i| ((i * 7 + 3) % 253) as u8).collect();

        let dsf_bytes = pack_stereo_dsf_file(
            DsdBitRateHz::DSD128,
            &left,
            &right,
            DsfBitOrder::LsbFirst,
            DsfPadMode::SpecZero,
        )
        .unwrap();

        // Total file size: 92 header + 2 blocks * 2 channels * 4096 = 92 + 16,384 = 16,476 bytes
        assert_eq!(dsf_bytes.len(), 92 + 16_384);

        // Check first byte bit-reversal (LSB-first `bits_per_sample = 1`)
        assert_eq!(dsf_bytes[92], left[0].reverse_bits());

        // Check trailing pad bytes in Block 1 Left (offset 92 + 8192 + 904 .. 92 + 8192 + 4096) are 0x00
        let block1_left_pad = &dsf_bytes[92 + 8192 + 904..92 + 8192 + 4096];
        assert!(block1_left_pad.iter().all(|&b| b == 0x00));

        // Round-trip decode and verify exact sample_count trimming
        let (hdr, dec_l, dec_r) = parse_stereo_dsf_file(&dsf_bytes).unwrap();
        assert_eq!(hdr.bit_rate, DsdBitRateHz::DSD128);
        assert_eq!(hdr.bit_order, DsfBitOrder::LsbFirst);
        assert_eq!(hdr.sample_count_per_channel, 5000 * 8);
        assert_eq!(dec_l, left);
        assert_eq!(dec_r, right);
    }

    #[test]
    fn test_dsf_silence_padding_mode_uses_balanced_0x69() {
        let left = vec![0xAA; 100];
        let right = vec![0x55; 100];

        let dsf_bytes = pack_stereo_dsf_file(
            DsdBitRateHz::DSD64,
            &left,
            &right,
            DsfBitOrder::LsbFirst,
            DsfPadMode::DsdSilence,
        )
        .unwrap();

        let expected_pad_byte = 0x69u8.reverse_bits(); // 0x69 reversed is 0x96 (still 4 ones, 4 zeros!)
        assert_eq!(expected_pad_byte.count_ones(), 4);

        let left_pad = &dsf_bytes[92 + 100..92 + 4096];
        let right_pad = &dsf_bytes[92 + 4096 + 100..92 + 8192];
        assert!(left_pad.iter().all(|&b| b == expected_pad_byte));
        assert!(right_pad.iter().all(|&b| b == expected_pad_byte));

        let (hdr, dec_l, dec_r) = parse_stereo_dsf_file(&dsf_bytes).unwrap();
        assert_eq!(hdr.sample_count_per_channel, 100 * 8);
        assert_eq!(dec_l, left);
        assert_eq!(dec_r, right);
    }
}
