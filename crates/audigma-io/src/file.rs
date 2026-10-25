//! File I/O driver (`Design.md §2`, `§5.3`).
//!
//! Provides seekable file-backed implementations of `AudioInput` and `AudioOutput`.

use audigma_dsp::fir::HalfBandDecimationCascade;
use audigma_dsp::rates::DsdMultiplier;
use audigma_engine::traits::{AudioInput, AudioOutput, DecoderInput, EngineError};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_format::dsf_reader::DsfReader;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

#[allow(clippy::large_enum_variant)]
enum FileInputSource {
    Pcm(DecoderInput),
    Dsf {
        reader: DsfReader<File>,
        info: AudioStreamInfo,
        decimator_l: HalfBandDecimationCascade,
        decimator_r: HalfBandDecimationCascade,
        dsd_scratch_l: Vec<u8>,
        dsd_scratch_r: Vec<u8>,
        divider: usize,
    },
}

/// File-backed streaming audio input that decodes WAV, FLAC, or Sony DSF files.
pub struct FileAudioInput {
    source: FileInputSource,
}

impl FileAudioInput {
    /// Opens an audio file for decoding with default DSD decimation normalization gain (+6.0 dB).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, EngineError> {
        Self::open_with_dsd_gain(path, 6.0)
    }

    /// Opens an audio file for decoding with specified DSD decimation gain in dB.
    pub fn open_with_dsd_gain<P: AsRef<Path>>(
        path: P,
        dsd_gain_db: f64,
    ) -> Result<Self, EngineError> {
        let path_ref = path.as_ref();
        let file = File::open(path_ref).map_err(|e| {
            EngineError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to open input file '{}': {e}", path_ref.display()),
            ))
        })?;

        let ext = path_ref
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase());

        if ext.as_deref() == Some("dsf") {
            let reader = DsfReader::try_new(file)?;
            let bit_rate = reader.header().bit_rate;
            let channels = reader.header().channels as u8;
            let sample_count = reader.total_samples_per_channel();
            let info = AudioStreamInfo::new_dsd(bit_rate, channels, Some(sample_count));

            let stages = match bit_rate.dsd_multiplier() {
                Some(DsdMultiplier::Dsd64) => 4,  // 16x decimation -> 176.4k
                Some(DsdMultiplier::Dsd128) => 4, // 16x decimation -> 352.8k
                Some(DsdMultiplier::Dsd256) => 5, // 32x decimation -> 352.8k
                Some(DsdMultiplier::Dsd512) => 6, // 64x decimation -> 352.8k
                None => 5,
            };
            let divider = 1 << stages;
            let preset = audigma_dsp::FilterPreset::Balanced;
            let max_frames = 65536;

            Ok(Self {
                source: FileInputSource::Dsf {
                    reader,
                    info,
                    decimator_l: HalfBandDecimationCascade::new(preset, stages, max_frames)
                        .with_gain_db(dsd_gain_db),
                    decimator_r: HalfBandDecimationCascade::new(preset, stages, max_frames)
                        .with_gain_db(dsd_gain_db),
                    dsd_scratch_l: vec![0; max_frames / 8],
                    dsd_scratch_r: vec![0; max_frames / 8],
                    divider,
                },
            })
        } else {
            let decoder = AudioDecoder::try_new(file, ext.as_deref())?;
            Ok(Self {
                source: FileInputSource::Pcm(DecoderInput::new(decoder)),
            })
        }
    }
}

impl AudioInput for FileAudioInput {
    #[inline]
    fn info(&self) -> &AudioStreamInfo {
        match &self.source {
            FileInputSource::Pcm(dec) => dec.info(),
            FileInputSource::Dsf { info, .. } => info,
        }
    }

    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        match &mut self.source {
            FileInputSource::Pcm(dec) => dec.read_stereo(left, right),
            FileInputSource::Dsf {
                reader,
                decimator_l,
                decimator_r,
                dsd_scratch_l,
                dsd_scratch_r,
                divider,
                ..
            } => {
                let max_pcm = left.len().min(right.len());
                if max_pcm == 0 {
                    return Ok(0);
                }
                // Each PCM frame requires `divider` DSD bits = `divider / 8` DSD bytes
                let dsd_bytes_per_pcm = (*divider).div_ceil(8);
                let needed_dsd_bytes = (max_pcm * dsd_bytes_per_pcm).min(dsd_scratch_l.len());
                let pcm_frames_to_produce = needed_dsd_bytes / dsd_bytes_per_pcm;
                let actual_dsd_bytes = pcm_frames_to_produce * dsd_bytes_per_pcm;

                if actual_dsd_bytes == 0 {
                    return Ok(0);
                }

                let bytes_read = reader.read_dsd_bytes(&mut [
                    &mut dsd_scratch_l[..actual_dsd_bytes],
                    &mut dsd_scratch_r[..actual_dsd_bytes],
                ])?;
                if bytes_read == 0 {
                    return Ok(0);
                }

                let frames_l = decimator_l.process_dsd_bytes(
                    &dsd_scratch_l[..bytes_read],
                    &mut left[..pcm_frames_to_produce],
                )?;
                let frames_r = decimator_r.process_dsd_bytes(
                    &dsd_scratch_r[..bytes_read],
                    &mut right[..pcm_frames_to_produce],
                )?;

                Ok(frames_l.min(frames_r))
            }
        }
    }

    fn read_dsd_bytes(&mut self, channels: &mut [&mut [u8]]) -> Result<usize, EngineError> {
        match &mut self.source {
            FileInputSource::Pcm(_) => Err(EngineError::InvalidTopology {
                message: "PCM file input does not provide raw DSD bytes".into(),
            }),
            FileInputSource::Dsf { reader, .. } => Ok(reader.read_dsd_bytes(channels)?),
        }
    }

    #[inline]
    fn can_rewind(&self) -> bool {
        match &self.source {
            FileInputSource::Pcm(dec) => dec.can_rewind(),
            FileInputSource::Dsf { .. } => true,
        }
    }

    #[inline]
    fn rewind(&mut self) -> Result<(), EngineError> {
        match &mut self.source {
            FileInputSource::Pcm(dec) => dec.rewind(),
            FileInputSource::Dsf {
                reader,
                decimator_l,
                decimator_r,
                ..
            } => {
                reader.rewind()?;
                decimator_l.reset();
                decimator_r.reset();
                Ok(())
            }
        }
    }
}

/// File-backed seekable audio output sink.
pub struct FileAudioOutput {
    file: File,
}

impl FileAudioOutput {
    /// Creates or truncates a file for output writing.
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self, EngineError> {
        let path_ref = path.as_ref();
        let file = File::create(path_ref).map_err(|e| {
            EngineError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to create output file '{}': {e}", path_ref.display()),
            ))
        })?;
        Ok(Self { file })
    }
}

impl Write for FileAudioOutput {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.write(buf)
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl AudioOutput for FileAudioOutput {
    #[inline]
    fn can_seek(&self) -> bool {
        true
    }

    #[inline]
    fn seek_sink(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}
