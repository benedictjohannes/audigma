//! Generic PCM audio decoder using `symphonia` supporting WAV, FLAC, and raw formats
//! with channel topology conditioning into normalized `f64` stereo streams.

use crate::FormatError;
use crate::topology::downmix_interleaved_to_stereo;
use audigma_dsp::PcmRateHz;
use symphonia::core::codecs::audio::{
    AudioDecoder as SymphoniaAudioDecoder, AudioDecoderOptions, CODEC_ID_NULL_AUDIO,
};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

/// Decoded stream metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioStreamInfo {
    pub sample_rate: PcmRateHz,
    pub original_channels: u8,
    pub total_frames: Option<u64>,
    pub bits_per_sample: Option<u32>,
}

/// A streaming audio decoder that reads and decodes audio files into stereo `f64` PCM.
pub struct AudioDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaAudioDecoder>,
    track_id: u32,
    info: AudioStreamInfo,
    channels: usize,
    interleaved_scratch: Vec<f64>,
}

impl AudioDecoder {
    /// Opens an audio stream from any seekable media source (e.g., `std::fs::File` or `std::io::Cursor`).
    pub fn try_new<S: MediaSource + 'static>(
        source: S,
        extension_hint: Option<&str>,
    ) -> Result<Self, FormatError> {
        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext.trim_start_matches('.'));
        }

        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();

        let format = symphonia::default::get_probe()
            .probe(&hint, mss, format_opts, metadata_opts)
            .map_err(|e| FormatError::DecoderError(format!("probe error: {e}")))?;

        // Find first supported audio track
        let track = format
            .tracks()
            .iter()
            .find(|t| {
                t.codec_params
                    .as_ref()
                    .and_then(|p| p.audio())
                    .map(|a| a.codec != CODEC_ID_NULL_AUDIO)
                    .unwrap_or(false)
            })
            .ok_or_else(|| FormatError::DecoderError("no valid audio track found".into()))?;

        let track_id = track.id;
        let audio_params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .cloned()
            .ok_or_else(|| FormatError::DecoderError("audio codec parameters missing".into()))?;

        let sample_rate_val = audio_params
            .sample_rate
            .ok_or_else(|| FormatError::DecoderError("audio track has no sample rate".into()))?;

        let sample_rate = PcmRateHz::new(sample_rate_val).ok_or_else(|| {
            FormatError::DecoderError(format!("unsupported sample rate {sample_rate_val} Hz"))
        })?;

        let channels = audio_params
            .channels
            .as_ref()
            .map(|c| c.count())
            .unwrap_or(2);

        let total_frames = track.num_frames;
        let bits_per_sample = audio_params.bits_per_sample;

        let decoder_opts = AudioDecoderOptions::default();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&audio_params, &decoder_opts)
            .map_err(|e| FormatError::DecoderError(format!("failed to initialize decoder: {e}")))?;

        let info = AudioStreamInfo {
            sample_rate,
            original_channels: channels as u8,
            total_frames,
            bits_per_sample,
        };

        Ok(Self {
            format,
            decoder,
            track_id,
            info,
            channels,
            interleaved_scratch: Vec::new(),
        })
    }

    /// Returns stream metadata (sample rate, channel count, duration hint).
    #[inline]
    pub fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    /// Decodes the next chunk of frames into separate left and right stereo buffers.
    ///
    /// Returns `Ok(frames_read)`. When EOF is reached, returns `Ok(0)`.
    pub fn read_stereo_chunk(
        &mut self,
        out_left: &mut Vec<f64>,
        out_right: &mut Vec<f64>,
    ) -> Result<usize, FormatError> {
        let initial_left_len = out_left.len();

        loop {
            let packet_opt = match self.format.next_packet() {
                Ok(opt) => opt,
                Err(SymphoniaError::IoError(ref err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    // Clean EOF
                    break;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(FormatError::DecoderError(format!("read error: {e}")));
                }
            };

            let packet = match packet_opt {
                Some(p) => p,
                None => break, // EOF reached
            };

            if packet.track_id != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let n_frames = decoded.frames();
                    if n_frames == 0 {
                        continue;
                    }

                    // Copy all channels interleaved into scratch Vec<f64>
                    self.interleaved_scratch.clear();
                    decoded.copy_to_vec_interleaved(&mut self.interleaved_scratch);

                    downmix_interleaved_to_stereo(
                        self.channels,
                        &self.interleaved_scratch,
                        out_left,
                        out_right,
                    );

                    let frames_added = out_left.len() - initial_left_len;
                    if frames_added > 0 {
                        return Ok(frames_added);
                    }
                }
                Err(SymphoniaError::IoError(ref err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(SymphoniaError::DecodeError(msg)) => {
                    // Recoverable packet decode error: log and skip
                    eprintln!("audigma warning: skipped corrupted packet: {msg}");
                    continue;
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(e) => {
                    return Err(FormatError::DecoderError(format!("decode error: {e}")));
                }
            }
        }

        Ok(out_left.len() - initial_left_len)
    }

    /// Rewinds the decoder to the beginning of the audio track for two-pass processing.
    pub fn rewind(&mut self) -> Result<(), FormatError> {
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: symphonia::core::units::Timestamp::from(0),
                    track_id: self.track_id,
                },
            )
            .map_err(|e| FormatError::DecoderError(format!("seek error: {e}")))?;

        self.decoder.reset();
        Ok(())
    }
}
