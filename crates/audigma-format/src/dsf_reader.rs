//! Streaming Sony DSF v1.01 file decoder and de-interleaver (`Design.md §5.2`).

use crate::FormatError;
use crate::dsf::{DSF_BLOCK_SIZE_PER_CHANNEL, DSF_HEADER_TOTAL_BYTES, DsfHeader};
use crate::metadata::AudioMetadata;
use std::io::{Read, Seek, SeekFrom};

/// A streaming reader for Sony DSF v1.01 files.
///
/// Reads and parses the 92-byte DSF header, reads interleaved 4096-byte channel blocks,
/// converts bytes to canonical MSB-first chronological DSD bytes, and tracks exact
/// `sample_count_per_channel` so padding bytes in the final block are not delivered.
pub struct DsfReader<R> {
    reader: R,
    header: DsfHeader,
    samples_read_per_channel: u64,
    current_block_idx: u64,
    total_blocks: u64,
    block_buffer: Vec<u8>,
    pos_in_block: usize,
    metadata: Option<AudioMetadata>,
}

impl<R: Read + Seek> DsfReader<R> {
    /// Opens and validates a DSF stream, reading the 92-byte header.
    pub fn try_new(mut reader: R) -> Result<Self, FormatError> {
        let mut header_buf = [0u8; DSF_HEADER_TOTAL_BYTES];
        reader.read_exact(&mut header_buf)?;
        let header = DsfHeader::from_bytes(&header_buf)?;

        let metadata = if header.metadata_offset != 0 {
            if reader.seek(SeekFrom::Start(header.metadata_offset)).is_ok() {
                let tag_res = id3::Tag::read_from2(&mut reader).ok();
                let _ = reader.seek(SeekFrom::Start(DSF_HEADER_TOTAL_BYTES as u64));
                tag_res
                    .map(|t| AudioMetadata::from_id3_tag(&t))
                    .filter(|m| !m.is_empty())
            } else {
                None
            }
        } else {
            None
        };

        let channels = header.channels as usize;
        let stereo_block_bytes = channels * DSF_BLOCK_SIZE_PER_CHANNEL;
        let total_blocks = header.data_payload_bytes / (stereo_block_bytes as u64);

        Ok(Self {
            reader,
            header,
            samples_read_per_channel: 0,
            current_block_idx: 0,
            total_blocks,
            block_buffer: Vec::new(),
            pos_in_block: DSF_BLOCK_SIZE_PER_CHANNEL, // Trigger reading first block on first read
            metadata,
        })
    }

    /// Returns container audio metadata (ID3v2 tags and pictures) if present.
    #[inline]
    pub fn metadata(&self) -> Option<&AudioMetadata> {
        self.metadata.as_ref()
    }

    /// Returns the parsed DSF header metadata.
    #[inline]
    pub const fn header(&self) -> &DsfHeader {
        &self.header
    }

    /// Returns the exact total number of 1-bit DSD samples per channel in this file.
    #[inline]
    pub const fn total_samples_per_channel(&self) -> u64 {
        self.header.sample_count_per_channel
    }

    /// Rewinds the reader to the beginning of audio data (byte 92).
    pub fn rewind(&mut self) -> Result<(), FormatError> {
        self.reader
            .seek(SeekFrom::Start(DSF_HEADER_TOTAL_BYTES as u64))?;
        self.samples_read_per_channel = 0;
        self.current_block_idx = 0;
        self.block_buffer.clear();
        self.pos_in_block = DSF_BLOCK_SIZE_PER_CHANNEL;
        Ok(())
    }

    /// Reads up to `channels[0].len()` MSB-first chronological DSD bytes for each channel.
    ///
    /// Slices in `channels` must all have the same length and length must equal `header.channels`.
    /// Returns the number of DSD bytes read per channel (`0` at EOF).
    pub fn read_dsd_bytes(&mut self, channels: &mut [&mut [u8]]) -> Result<usize, FormatError> {
        if channels.len() != self.header.channels as usize {
            return Err(FormatError::ChannelLengthMismatch {
                left: channels.len(),
                right: self.header.channels as usize,
            });
        }
        let requested_bytes = channels.iter().map(|s| s.len()).min().unwrap_or(0);
        if requested_bytes == 0 {
            return Ok(0);
        }

        let total_valid_bytes_per_ch = self.header.sample_count_per_channel.div_ceil(8);
        let remaining_valid_bytes =
            (total_valid_bytes_per_ch.saturating_sub(self.samples_read_per_channel / 8)) as usize;
        if remaining_valid_bytes == 0 {
            return Ok(0);
        }

        let target_bytes = requested_bytes.min(remaining_valid_bytes);
        let mut bytes_delivered = 0;
        let ch_count = channels.len();
        let stereo_block_bytes = ch_count * DSF_BLOCK_SIZE_PER_CHANNEL;

        while bytes_delivered < target_bytes {
            if self.pos_in_block >= DSF_BLOCK_SIZE_PER_CHANNEL {
                if self.current_block_idx >= self.total_blocks {
                    break;
                }
                self.block_buffer.resize(stereo_block_bytes, 0);
                self.reader.read_exact(&mut self.block_buffer)?;
                self.current_block_idx += 1;
                self.pos_in_block = 0;
            }

            let available_in_block = DSF_BLOCK_SIZE_PER_CHANNEL - self.pos_in_block;
            let to_copy = (target_bytes - bytes_delivered).min(available_in_block);

            for (c, ch_slice) in channels.iter_mut().enumerate() {
                let start = c * DSF_BLOCK_SIZE_PER_CHANNEL + self.pos_in_block;
                let end = start + to_copy;
                let raw_slice = &self.block_buffer[start..end];
                let out_slice = &mut ch_slice[bytes_delivered..bytes_delivered + to_copy];

                for (src_b, dst_b) in raw_slice.iter().zip(out_slice.iter_mut()) {
                    *dst_b = self.header.bit_order.decode_to_msb_byte(*src_b);
                }
            }

            self.pos_in_block += to_copy;
            bytes_delivered += to_copy;
            self.samples_read_per_channel += (to_copy as u64) * 8;
        }

        Ok(bytes_delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsf::{DsfBitOrder, DsfPadMode, pack_stereo_dsf_file};
    use audigma_dsp::DsdBitRateHz;
    use std::io::Cursor;

    #[test]
    fn test_dsf_reader_roundtrip_with_padding_truncation() {
        // 5000 bytes per channel (spans 2 blocks: 4096 + 904 valid bytes, 3192 pad bytes)
        let left: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
        let right: Vec<u8> = (0..5000).map(|i| ((i * 7 + 3) % 253) as u8).collect();

        let file_bytes = pack_stereo_dsf_file(
            DsdBitRateHz::DSD128,
            &left,
            &right,
            DsfBitOrder::LsbFirst,
            DsfPadMode::SpecZero,
        )
        .unwrap();

        let mut reader = DsfReader::try_new(Cursor::new(file_bytes)).unwrap();
        assert_eq!(reader.header().bit_rate, DsdBitRateHz::DSD128);
        assert_eq!(reader.total_samples_per_channel(), 5000 * 8);

        // Read in chunks of 1024 bytes
        let mut read_left = Vec::new();
        let mut read_right = Vec::new();
        let mut buf_l = [0u8; 1024];
        let mut buf_r = [0u8; 1024];

        loop {
            let n = reader
                .read_dsd_bytes(&mut [&mut buf_l, &mut buf_r])
                .unwrap();
            if n == 0 {
                break;
            }
            read_left.extend_from_slice(&buf_l[..n]);
            read_right.extend_from_slice(&buf_r[..n]);
        }

        assert_eq!(read_left.len(), 5000);
        assert_eq!(read_right.len(), 5000);
        assert_eq!(read_left, left);
        assert_eq!(read_right, right);

        // Test rewind
        reader.rewind().unwrap();
        let mut buf_l2 = [0u8; 5000];
        let mut buf_r2 = [0u8; 5000];
        let n = reader
            .read_dsd_bytes(&mut [&mut buf_l2, &mut buf_r2])
            .unwrap();
        assert_eq!(n, 5000);
        assert_eq!(&buf_l2[..], &left[..]);
        assert_eq!(&buf_r2[..], &right[..]);
    }
}
