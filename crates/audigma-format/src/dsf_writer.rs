//! Seekable Sony DSF (`.dsf`) file writer (`Design.md §5.2`).
//!
//! Enforces Sony DSF v1.01 block interleaving (4096 bytes L, 4096 bytes R), exact sample counts,
//! and final block padding via `DsfPadMode` (`spec-zero` vs. `dsd-silence` balanced `0x69`).

use crate::FormatError;
use crate::dsf::{
    DSF_BLOCK_SIZE_PER_CHANNEL, DSF_HEADER_TOTAL_BYTES, DsfBitOrder, DsfHeader, DsfPadMode,
};
use audigma_dsp::DsdBitRateHz;
use std::io::{Seek, SeekFrom, Write};

/// Sony DSF file writer.
pub struct DsfFileWriter<W: Write + Seek> {
    writer: W,
    bit_rate: DsdBitRateHz,
    bit_order: DsfBitOrder,
    pad_mode: DsfPadMode,
    left_staging: Vec<u8>,
    right_staging: Vec<u8>,
    samples_per_channel: u64,
    payload_bytes_written: u64,
}

impl<W: Write + Seek> DsfFileWriter<W> {
    /// Initializes a new DSF file writer, reserving the 92-byte header block.
    pub fn new(
        mut writer: W,
        bit_rate: DsdBitRateHz,
        bit_order: DsfBitOrder,
        pad_mode: DsfPadMode,
    ) -> Result<Self, FormatError> {
        let dummy_header = DsfHeader {
            bit_rate,
            channels: 2,
            bit_order,
            sample_count_per_channel: 0,
            data_payload_bytes: 0,
        };

        writer
            .write_all(&dummy_header.to_bytes())
            .map_err(|e| FormatError::IoError(e.to_string()))?;

        Ok(Self {
            writer,
            bit_rate,
            bit_order,
            pad_mode,
            left_staging: Vec::with_capacity(DSF_BLOCK_SIZE_PER_CHANNEL),
            right_staging: Vec::with_capacity(DSF_BLOCK_SIZE_PER_CHANNEL),
            samples_per_channel: 0,
            payload_bytes_written: 0,
        })
    }

    /// Appends 1-bit DSD bytes (in canonical MSB-first chronological order) for left and right channels.
    pub fn write_stereo_dsd_bytes(
        &mut self,
        left_msb: &[u8],
        right_msb: &[u8],
    ) -> Result<(), FormatError> {
        if left_msb.len() != right_msb.len() {
            return Err(FormatError::ChannelLengthMismatch {
                left: left_msb.len(),
                right: right_msb.len(),
            });
        }

        self.left_staging.reserve(left_msb.len());
        self.right_staging.reserve(right_msb.len());

        for (&l, &r) in left_msb.iter().zip(right_msb.iter()) {
            self.left_staging
                .push(self.bit_order.encode_from_msb_byte(l));
            self.right_staging
                .push(self.bit_order.encode_from_msb_byte(r));
        }

        self.samples_per_channel += (left_msb.len() as u64) * 8;

        // Flush full 4096-byte blocks
        while self.left_staging.len() >= DSF_BLOCK_SIZE_PER_CHANNEL
            && self.right_staging.len() >= DSF_BLOCK_SIZE_PER_CHANNEL
        {
            self.writer
                .write_all(&self.left_staging[..DSF_BLOCK_SIZE_PER_CHANNEL])
                .map_err(|e| FormatError::IoError(e.to_string()))?;
            self.writer
                .write_all(&self.right_staging[..DSF_BLOCK_SIZE_PER_CHANNEL])
                .map_err(|e| FormatError::IoError(e.to_string()))?;

            self.left_staging.drain(..DSF_BLOCK_SIZE_PER_CHANNEL);
            self.right_staging.drain(..DSF_BLOCK_SIZE_PER_CHANNEL);
            self.payload_bytes_written += (DSF_BLOCK_SIZE_PER_CHANNEL as u64) * 2;
        }

        Ok(())
    }

    /// Pads the final channel blocks and patches header chunk sizes at offset 0.
    ///
    /// Returns `(sample_count_per_channel, total_file_size_bytes)`.
    pub fn finalize(mut self) -> Result<(u64, u64), FormatError> {
        // If there are partial leftover bytes, pad to full 4096-byte blocks
        if !self.left_staging.is_empty() || !self.right_staging.is_empty() {
            let pad = self.pad_mode.pad_byte(self.bit_order);
            self.left_staging.resize(DSF_BLOCK_SIZE_PER_CHANNEL, pad);
            self.right_staging.resize(DSF_BLOCK_SIZE_PER_CHANNEL, pad);

            self.writer
                .write_all(&self.left_staging)
                .map_err(|e| FormatError::IoError(e.to_string()))?;
            self.writer
                .write_all(&self.right_staging)
                .map_err(|e| FormatError::IoError(e.to_string()))?;

            self.payload_bytes_written += (DSF_BLOCK_SIZE_PER_CHANNEL as u64) * 2;
        }

        // Seek back to patch the finalized header
        self.writer
            .seek(SeekFrom::Start(0))
            .map_err(|e| FormatError::IoError(e.to_string()))?;

        let finalized_header = DsfHeader {
            bit_rate: self.bit_rate,
            channels: 2,
            bit_order: self.bit_order,
            sample_count_per_channel: self.samples_per_channel,
            data_payload_bytes: self.payload_bytes_written,
        };

        self.writer
            .write_all(&finalized_header.to_bytes())
            .map_err(|e| FormatError::IoError(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| FormatError::IoError(e.to_string()))?;

        let total_file_size = (DSF_HEADER_TOTAL_BYTES as u64) + self.payload_bytes_written;
        Ok((self.samples_per_channel, total_file_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsf::parse_stereo_dsf_file;
    use std::io::Cursor;

    #[test]
    fn test_dsf_file_writer_roundtrip() {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer = DsfFileWriter::new(
            &mut buffer,
            DsdBitRateHz::DSD256,
            DsfBitOrder::LsbFirst,
            DsfPadMode::DsdSilence,
        )
        .unwrap();

        // Write 100 bytes of alternating test bits per channel
        let left_msb = vec![0xAA; 100];
        let right_msb = vec![0x55; 100];

        writer
            .write_stereo_dsd_bytes(&left_msb, &right_msb)
            .unwrap();
        let (samples, total_bytes) = writer.finalize().unwrap();

        assert_eq!(samples, 800);
        // Header (92) + 1 padded block pair (4096 * 2 = 8192) = 8284 bytes
        assert_eq!(total_bytes, 92 + 8192);

        // Verify with parser
        buffer.set_position(0);
        let bytes = buffer.into_inner();
        let (header, l_out, r_out) = parse_stereo_dsf_file(&bytes).unwrap();
        assert_eq!(header.sample_count_per_channel, 800);
        assert_eq!(header.bit_rate, DsdBitRateHz::DSD256);
        assert_eq!(l_out.len(), 100);
        assert_eq!(r_out.len(), 100);
        assert_eq!(l_out, left_msb);
        assert_eq!(r_out, right_msb);
    }
}
