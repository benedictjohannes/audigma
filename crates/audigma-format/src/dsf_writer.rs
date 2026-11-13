//! Seekable Sony DSF (`.dsf`) file writer (`Design.md §5.2`).
//!
//! Enforces Sony DSF v1.01 block interleaving (4096 bytes L, 4096 bytes R), exact sample counts,
//! and final block padding via `DsfPadMode` (`spec-zero` vs. `dsd-silence` balanced `0x69`).

use crate::FormatError;
use crate::dsf::{
    DSF_BLOCK_SIZE_PER_CHANNEL, DSF_HEADER_TOTAL_BYTES, DsfBitOrder, DsfHeader, DsfPadMode,
};
use crate::metadata::AudioMetadata;
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
    metadata: Option<AudioMetadata>,
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
            metadata_offset: 0,
            total_file_size: 0,
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
            metadata: None,
        })
    }

    /// Attaches metadata to be written to the DSF file as an ID3v2 chunk at EOF.
    pub fn with_metadata(mut self, metadata: AudioMetadata) -> Self {
        if !metadata.is_empty() {
            self.metadata = Some(metadata);
        }
        self
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

        // If metadata is attached, serialize and append ID3v2 chunk at current file position
        let (metadata_offset, metadata_len) = if let Some(ref meta) = self.metadata {
            if let Some(id3_bytes) = meta.encode_dsf_id3v2_tag() {
                let meta_pos = (DSF_HEADER_TOTAL_BYTES as u64) + self.payload_bytes_written;
                self.writer
                    .write_all(&id3_bytes)
                    .map_err(|e| FormatError::IoError(e.to_string()))?;
                (meta_pos, id3_bytes.len() as u64)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        let total_file_size =
            (DSF_HEADER_TOTAL_BYTES as u64) + self.payload_bytes_written + metadata_len;

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
            metadata_offset,
            total_file_size,
        };

        self.writer
            .write_all(&finalized_header.to_bytes())
            .map_err(|e| FormatError::IoError(e.to_string()))?;
        self.writer
            .flush()
            .map_err(|e| FormatError::IoError(e.to_string()))?;

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

    #[test]
    fn test_dsf_file_writer_with_metadata_roundtrip() {
        use crate::dsf_reader::DsfReader;
        use crate::metadata::{AudioMetadata, Picture, PictureType};

        let mut buffer = Cursor::new(Vec::new());
        let mut meta = AudioMetadata::default();
        meta.title = Some("DSD Title".into());
        meta.artist = Some("DSD Artist".into());
        meta.album = Some("DSD Album".into());
        meta.track_number = Some(5);
        meta.pictures.push(Picture {
            picture_type: PictureType::CoverFront,
            mime_type: "image/jpeg".into(),
            description: "Front".into(),
            width: 300,
            height: 300,
            color_depth: 24,
            num_colors: 0,
            data: vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10],
        });

        let mut writer = DsfFileWriter::new(
            &mut buffer,
            DsdBitRateHz::DSD64,
            DsfBitOrder::LsbFirst,
            DsfPadMode::SpecZero,
        )
        .unwrap()
        .with_metadata(meta);

        let left_msb = vec![0x12; 100];
        let right_msb = vec![0x34; 100];
        writer.write_stereo_dsd_bytes(&left_msb, &right_msb).unwrap();
        let (samples, total_bytes) = writer.finalize().unwrap();

        assert_eq!(samples, 800);
        assert!(total_bytes > 92 + 8192);

        // Read back with DsfReader
        buffer.set_position(0);
        let reader = DsfReader::try_new(buffer).unwrap();
        assert!(reader.header().metadata_offset > 0);
        let read_meta = reader.metadata().expect("metadata present");
        assert_eq!(read_meta.title.as_deref(), Some("DSD Title"));
        assert_eq!(read_meta.artist.as_deref(), Some("DSD Artist"));
        assert_eq!(read_meta.album.as_deref(), Some("DSD Album"));
        assert_eq!(read_meta.track_number, Some(5));
        assert_eq!(read_meta.pictures.len(), 1);
        assert_eq!(read_meta.pictures[0].picture_type, PictureType::CoverFront);
        assert_eq!(read_meta.pictures[0].data, vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
    }
}
