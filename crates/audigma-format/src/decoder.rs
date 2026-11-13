//! Generic PCM audio decoder using `symphonia` supporting WAV, FLAC, and raw formats
//! with channel topology conditioning into normalized `f64` stereo streams.

use crate::FormatError;
use crate::metadata::AudioMetadata;
use crate::topology::downmix_interleaved_to_stereo;
use std::io::SeekFrom;
use audigma_dsp::{DsdBitRateHz, PcmRateHz};
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
    pub dsd_bit_rate: Option<DsdBitRateHz>,
}

impl AudioStreamInfo {
    /// Constructs PCM audio stream info.
    pub const fn new_pcm(
        sample_rate: PcmRateHz,
        channels: u8,
        total_frames: Option<u64>,
        bits_per_sample: Option<u32>,
    ) -> Self {
        Self {
            sample_rate,
            original_channels: channels,
            total_frames,
            bits_per_sample,
            dsd_bit_rate: None,
        }
    }

    /// Constructs native DSD audio stream info.
    pub const fn new_dsd(
        dsd_bit_rate: DsdBitRateHz,
        channels: u8,
        total_samples: Option<u64>,
    ) -> Self {
        let intermediate_pcm = match dsd_bit_rate.rate_family() {
            audigma_dsp::RateFamily::Hz44100 => match dsd_bit_rate.dsd_multiplier() {
                Some(audigma_dsp::DsdMultiplier::Dsd64) => PcmRateHz::PCM_176400,
                _ => PcmRateHz::PCM_352800,
            },
            audigma_dsp::RateFamily::Hz48000 => match dsd_bit_rate.dsd_multiplier() {
                Some(audigma_dsp::DsdMultiplier::Dsd64) => PcmRateHz::PCM_192000,
                _ => PcmRateHz::PCM_384000,
            },
            audigma_dsp::RateFamily::Other => PcmRateHz::PCM_44100,
        };
        let stages = match dsd_bit_rate.dsd_multiplier() {
            Some(audigma_dsp::DsdMultiplier::Dsd64) => 4,
            Some(audigma_dsp::DsdMultiplier::Dsd128) => 4,
            Some(audigma_dsp::DsdMultiplier::Dsd256) => 5,
            Some(audigma_dsp::DsdMultiplier::Dsd512) => 6,
            None => 5,
        };
        let divider = 1u64 << stages;
        let total_frames = match total_samples {
            Some(s) => Some(s / divider),
            None => None,
        };
        Self {
            sample_rate: intermediate_pcm,
            original_channels: channels,
            total_frames,
            bits_per_sample: Some(1),
            dsd_bit_rate: Some(dsd_bit_rate),
        }
    }

    /// Constructs native DSD audio stream info with a custom intermediate PCM rate.
    pub fn new_dsd_with_intermediate(
        dsd_bit_rate: DsdBitRateHz,
        channels: u8,
        total_samples: Option<u64>,
        intermediate_pcm: PcmRateHz,
    ) -> Self {
        let stages = dsd_bit_rate
            .power_of_two_decimation_stages_to(intermediate_pcm)
            .unwrap_or(4);
        let divider = 1u64 << stages;
        let total_frames = total_samples.map(|s| s / divider);
        Self {
            sample_rate: intermediate_pcm,
            original_channels: channels,
            total_frames,
            bits_per_sample: Some(1),
            dsd_bit_rate: Some(dsd_bit_rate),
        }
    }

    /// Returns `true` if this audio stream carries 1-bit DSD audio.
    #[inline]
    pub const fn is_dsd(&self) -> bool {
        self.dsd_bit_rate.is_some()
    }
}

/// A streaming audio decoder that reads and decodes audio files into stereo `f64` PCM.
pub struct AudioDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaAudioDecoder>,
    track_id: u32,
    info: AudioStreamInfo,
    channels: usize,
    interleaved_scratch: Vec<f64>,
    metadata: Option<AudioMetadata>,
}

impl AudioDecoder {
    /// Opens an audio stream from any seekable media source (e.g., `std::fs::File` or `std::io::Cursor`).
    pub fn try_new<S: MediaSource + 'static>(
        mut source: S,
        extension_hint: Option<&str>,
    ) -> Result<Self, FormatError> {
        let is_wav = extension_hint
            .map(|e| e.trim_start_matches('.').eq_ignore_ascii_case("wav"))
            .unwrap_or(false);
        let wav_meta = if is_wav {
            let m = crate::metadata::parse_wav_riff_metadata(&mut source);
            let _ = source.seek(SeekFrom::Start(0));
            m
        } else {
            None
        };

        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext.trim_start_matches('.'));
        }

        let mss = MediaSourceStream::new(Box::new(source), Default::default());
        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();

        let mut format = symphonia::default::get_probe()
            .probe(&hint, mss, format_opts, metadata_opts)
            .map_err(|e| FormatError::DecoderError(format!("probe error: {e}")))?;

        let metadata = format
            .metadata()
            .current()
            .map(AudioMetadata::from_symphonia_revision)
            .filter(|m| !m.is_empty())
            .or(wav_meta);

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

        let info =
            AudioStreamInfo::new_pcm(sample_rate, channels as u8, total_frames, bits_per_sample);

        Ok(Self {
            format,
            decoder,
            track_id,
            info,
            channels,
            interleaved_scratch: Vec::new(),
            metadata,
        })
    }

    /// Returns stream metadata (sample rate, channel count, duration hint).
    #[inline]
    pub fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    /// Returns container audio metadata (tags and visual pictures) if present.
    #[inline]
    pub fn metadata(&self) -> Option<&AudioMetadata> {
        self.metadata.as_ref()
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
                    tracing::warn!(
                        target: "audigma::format::decoder",
                        event = "corrupted_packet_skipped",
                        error = %msg,
                        "skipped corrupted packet"
                    );
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
