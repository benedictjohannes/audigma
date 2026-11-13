//! `audigma-format`: Container formats (`DSF`, `WAV`, `FLAC`) and exact hardware bit-packers
//! (`DSD_U32_BE`, `DoP` across `s32le`, `s24le`, `s24_3le`) (`Design.md §5.2–§5.3`).
//!
//! Zero OS audio library dependencies.

pub mod decoder;
pub mod dop;
pub mod dsd_u32;
pub mod dsf;
pub mod dsf_reader;
pub mod dsf_writer;
pub mod flac_writer;
pub mod metadata;
pub mod topology;
pub mod wav_writer;

pub use decoder::{AudioDecoder, AudioStreamInfo};
pub use dop::{
    DOP_DETECTION_THRESHOLD_FRAMES, DOP_MARKER_EVEN, DOP_MARKER_ODD, DSD_BYTES_PER_DOP_FRAME,
    DopContainerFormat, DopDetector, DopPacker, detect_dop_format_in_pcm_buffer,
    unpack_stereo_dop_frames,
};
pub use dsd_u32::{
    DSD_BYTES_PER_U32_WORD, STEREO_DSD_U32_FRAME_BYTES, fill_stereo_dsd_u32_be_silence,
    pack_stereo_dsd_u32_be, unpack_stereo_dsd_u32_be,
};
pub use dsf::{
    DSF_BLOCK_SIZE_PER_CHANNEL, DSF_HEADER_TOTAL_BYTES, DsfBitOrder, DsfHeader, DsfPadMode,
    pack_stereo_dsf_blocks, pack_stereo_dsf_file, parse_stereo_dsf_file,
};
pub use dsf_reader::DsfReader;
pub use dsf_writer::DsfFileWriter;
pub use flac_writer::FlacEncoder;
pub use metadata::{AudioMetadata, Picture, PictureType};
pub use topology::downmix_interleaved_to_stereo;
pub use wav_writer::PcmWavWriter;

use thiserror::Error;

/// Errors produced during audio container or hardware frame packing/unpacking/decoding/encoding.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("left ({left} bytes) and right ({right} bytes) DSD channel lengths do not match")]
    ChannelLengthMismatch { left: usize, right: usize },

    #[error("DSD byte count ({count}) must be a multiple of {required_multiple}")]
    UnalignedDsdByteCount {
        count: usize,
        required_multiple: usize,
    },

    #[error("output buffer too small: required {required} bytes, provided {provided} bytes")]
    OutputBufferTooSmall { required: usize, provided: usize },

    #[error(
        "invalid DoP marker at frame {frame_index}: left=0x{left_marker:02X}, right=0x{right_marker:02X}"
    )]
    InvalidDopMarker {
        frame_index: usize,
        left_marker: u8,
        right_marker: u8,
    },

    #[error("non-zero DoP padding byte in {container} container: got 0x{got:02X}")]
    InvalidDopPaddingByte { container: &'static str, got: u8 },

    #[error("invalid Sony DSF v1.01 header: {0}")]
    InvalidDsfHeader(&'static str),

    #[error("decoder error: {0}")]
    DecoderError(String),

    #[error("encoder error: {0}")]
    EncoderError(String),

    #[error("I/O error: {0}")]
    IoError(String),

    #[error("unsupported sample format: {0:?}")]
    UnsupportedSampleFormat(audigma_dsp::SampleFormat),
}

impl From<std::io::Error> for FormatError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err.to_string())
    }
}
