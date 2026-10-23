//! Prepared offline audio converter pipeline (`Design.md §3.1`, `§3.4`, `§4.4`, `§5.2–§5.3`).
//!
//! Orchestrates:
//! - Channel validation & stereo conditioning
//! - Gain staging & two-pass true-peak analysis (`peak_analysis = "two-pass"`)
//! - Linear/minimum-phase half-band FIR cascade & polyphase rational SRC
//! - Modulator input peak meter & overload guard
//! - 5th-order CRFB Delta-Sigma Modulator or TPDF dithered PCM quantizer
//! - Tail management (`trim-to-duration` vs `preserve-tail`)
//! - Container and transport packing (DSF, WAV, FLAC, Native DSD, DoP, Raw)

use crate::peak::measure_true_peak_pass1;
use crate::tail::TailManager;
use crate::traits::{AudioInput, AudioOutput, EngineError};
use crate::worker::{ChannelWorkerPool, TargetDsdConfig};
use audigma_dsp::DSD_IDLE_BYTE;
use audigma_dsp::config::{SampleFormat, TargetFormat, TargetRateSpec};
use audigma_dsp::fir::PolyphaseResampler;
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::rates::PcmRateHz;
use audigma_format::dop::{DopContainerFormat, DopPacker};
use audigma_format::dsd_u32::{STEREO_DSD_U32_FRAME_BYTES, pack_stereo_dsd_u32_be};
use audigma_format::dsf::{DsfBitOrder, DsfPadMode};
use audigma_format::dsf_writer::DsfFileWriter;
use audigma_format::flac_writer::FlacEncoder;
use audigma_format::wav_writer::PcmWavWriter;
use audigma_settings::{
    OutputContainer, OutputTransport, PeakAnalysisMode, ResolvedPipelinePlan, Settings, TailPolicy,
};
use std::time::Instant;

/// Result report from executing an offline conversion pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineReport {
    /// Source PCM sample rate.
    pub input_rate: PcmRateHz,
    /// Destination format (`Pcm` or `Dsd`).
    pub target_format: TargetFormat,
    /// Total input baseband frames processed.
    pub total_input_frames: u64,
    /// Total output frames or DSD samples per channel written.
    pub total_output_frames: u64,
    /// Total output bytes written to sink.
    pub total_bytes_written: u64,
    /// Measured true peak in dBFS during Pass 1 (if `two-pass` was active).
    pub measured_peak_dbfs: Option<f64>,
    /// Final applied pre-modulator gain in dB.
    pub applied_gain_db: f64,
    /// Cumulative count of modulator overload events.
    pub overload_count: u64,
    /// Total conversion wall-clock time in milliseconds.
    pub elapsed_ms: u64,
}

/// Sink container wrapper encapsulating container writers (DSF, WAV, FLAC, Raw).
enum ContainerWriter<'a> {
    Dsf(DsfFileWriter<Box<dyn AudioOutput + 'a>>),
    Wav(PcmWavWriter<Box<dyn AudioOutput + 'a>>),
    Flac(FlacEncoder<Box<dyn AudioOutput + 'a>>),
    RawDsdNative {
        output: Box<dyn AudioOutput + 'a>,
        staging_u32: Vec<u8>,
        residue_left: Vec<u8>,
        residue_right: Vec<u8>,
        frames_written: u64,
        bytes_written: u64,
    },
    RawDsdDop {
        output: Box<dyn AudioOutput + 'a>,
        packer: DopPacker,
        staging_dop: Vec<u8>,
        residue_left: Vec<u8>,
        residue_right: Vec<u8>,
        frames_written: u64,
        bytes_written: u64,
    },
    RawPcm {
        output: Box<dyn AudioOutput + 'a>,
        format: SampleFormat,
        _valid_bits: u8,
        staging_pcm: Vec<u8>,
        frames_written: u64,
        bytes_written: u64,
    },
}

impl<'a> ContainerWriter<'a> {
    fn new(
        settings: &Settings,
        target_spec: &TargetRateSpec,
        output: Box<dyn AudioOutput + 'a>,
    ) -> Result<Self, EngineError> {
        let container = settings
            .output
            .as_ref()
            .and_then(|o| o.container)
            .unwrap_or(match target_spec {
                TargetRateSpec::Dsd { .. } => OutputContainer::Dsf,
                TargetRateSpec::Pcm { .. } => OutputContainer::Wav,
            });

        match container {
            OutputContainer::Dsf => {
                if !output.can_seek() {
                    return Err(EngineError::SeekableOutputRequired { container: "dsf" });
                }
                let bit_rate = match target_spec {
                    TargetRateSpec::Dsd { bit_rate } => *bit_rate,
                    TargetRateSpec::Pcm { .. } => {
                        return Err(EngineError::Pipeline(
                            "cannot write DSF container for PCM target format".into(),
                        ));
                    }
                };
                let pad_mode = settings
                    .offline
                    .as_ref()
                    .map(|o| o.dsf_pad_mode)
                    .unwrap_or(DsfPadMode::SpecZero);
                let writer = DsfFileWriter::new(output, bit_rate, DsfBitOrder::LsbFirst, pad_mode)?;
                Ok(Self::Dsf(writer))
            }
            OutputContainer::Wav => {
                if !output.can_seek() {
                    return Err(EngineError::SeekableOutputRequired { container: "wav" });
                }
                let (pcm_rate, sample_format, valid_bits) = match target_spec {
                    TargetRateSpec::Pcm {
                        rate,
                        sample_format,
                        valid_bits,
                    } => (*rate, *sample_format, *valid_bits),
                    TargetRateSpec::Dsd { .. } => {
                        return Err(EngineError::Pipeline(
                            "cannot write WAV container for DSD target format".into(),
                        ));
                    }
                };
                let writer = PcmWavWriter::new(output, pcm_rate, sample_format, valid_bits)?;
                Ok(Self::Wav(writer))
            }
            OutputContainer::Flac => {
                let (pcm_rate, sample_format) = match target_spec {
                    TargetRateSpec::Pcm {
                        rate,
                        sample_format,
                        ..
                    } => (*rate, *sample_format),
                    TargetRateSpec::Dsd { .. } => {
                        return Err(EngineError::Pipeline(
                            "cannot write FLAC container for DSD target format".into(),
                        ));
                    }
                };
                let bits_per_sample = match sample_format {
                    SampleFormat::S16Le => 16,
                    SampleFormat::S24Le | SampleFormat::S24_3Le | SampleFormat::S32Le => 24,
                    _ => 24,
                };
                let writer = FlacEncoder::new(output, pcm_rate, bits_per_sample)?;
                Ok(Self::Flac(writer))
            }
            OutputContainer::Raw => {
                let transport = settings
                    .output
                    .as_ref()
                    .and_then(|o| o.transport)
                    .unwrap_or(OutputTransport::Pcm);
                match transport {
                    OutputTransport::Native => Ok(Self::RawDsdNative {
                        output,
                        staging_u32: Vec::new(),
                        residue_left: Vec::new(),
                        residue_right: Vec::new(),
                        frames_written: 0,
                        bytes_written: 0,
                    }),
                    OutputTransport::Dop => {
                        let sample_format = settings
                            .output
                            .as_ref()
                            .and_then(|o| o.sample_format)
                            .unwrap_or(SampleFormat::S32Le);
                        let dop_fmt = match sample_format {
                            SampleFormat::S32Le => DopContainerFormat::S32Le,
                            SampleFormat::S24Le => DopContainerFormat::S24Le,
                            SampleFormat::S24_3Le => DopContainerFormat::S24_3Le,
                            _ => DopContainerFormat::S32Le,
                        };
                        Ok(Self::RawDsdDop {
                            output,
                            packer: DopPacker::new(dop_fmt),
                            staging_dop: Vec::new(),
                            residue_left: Vec::new(),
                            residue_right: Vec::new(),
                            frames_written: 0,
                            bytes_written: 0,
                        })
                    }
                    OutputTransport::Pcm => {
                        let sample_format = settings
                            .output
                            .as_ref()
                            .and_then(|o| o.sample_format)
                            .unwrap_or(SampleFormat::S16Le);
                        Ok(Self::RawPcm {
                            output,
                            format: sample_format,
                            _valid_bits: 24,
                            staging_pcm: Vec::new(),
                            frames_written: 0,
                            bytes_written: 0,
                        })
                    }
                }
            }
        }
    }

    fn write_stereo_pcm(&mut self, left: &[f64], right: &[f64]) -> Result<(), EngineError> {
        match self {
            Self::Wav(w) => {
                w.write_stereo_samples(left, right)?;
            }
            Self::Flac(w) => {
                w.write_stereo_samples(left, right)?;
            }
            Self::RawPcm {
                output,
                format,
                staging_pcm,
                frames_written,
                bytes_written,
                ..
            } => {
                staging_pcm.clear();
                for (&l, &r) in left.iter().zip(right.iter()) {
                    let l_clamped = l.clamp(-1.0, 1.0);
                    let r_clamped = r.clamp(-1.0, 1.0);
                    match format {
                        SampleFormat::S16Le => {
                            let l_i16 = (l_clamped * 32767.0).round() as i16;
                            let r_i16 = (r_clamped * 32767.0).round() as i16;
                            staging_pcm.extend_from_slice(&l_i16.to_le_bytes());
                            staging_pcm.extend_from_slice(&r_i16.to_le_bytes());
                        }
                        SampleFormat::S24_3Le => {
                            let l_i24 = (l_clamped * 8388607.0).round() as i32;
                            let r_i24 = (r_clamped * 8388607.0).round() as i32;
                            let l_b = l_i24.to_le_bytes();
                            let r_b = r_i24.to_le_bytes();
                            staging_pcm.extend_from_slice(&l_b[..3]);
                            staging_pcm.extend_from_slice(&r_b[..3]);
                        }
                        SampleFormat::S24Le | SampleFormat::S32Le => {
                            let l_i32 = (l_clamped * 2147483647.0).round() as i32;
                            let r_i32 = (r_clamped * 2147483647.0).round() as i32;
                            staging_pcm.extend_from_slice(&l_i32.to_le_bytes());
                            staging_pcm.extend_from_slice(&r_i32.to_le_bytes());
                        }
                        SampleFormat::F32 => {
                            let l_f32 = l as f32;
                            let r_f32 = r as f32;
                            staging_pcm.extend_from_slice(&l_f32.to_le_bytes());
                            staging_pcm.extend_from_slice(&r_f32.to_le_bytes());
                        }
                        _ => {}
                    }
                }
                output.write_all(staging_pcm)?;
                *frames_written += left.len() as u64;
                *bytes_written += staging_pcm.len() as u64;
            }
            _ => {
                return Err(EngineError::Pipeline(
                    "attempted to write PCM samples to DSD container".into(),
                ));
            }
        }
        Ok(())
    }

    fn write_stereo_dsd(
        &mut self,
        left_bytes: &[u8],
        right_bytes: &[u8],
    ) -> Result<(), EngineError> {
        match self {
            Self::Dsf(w) => {
                w.write_stereo_dsd_bytes(left_bytes, right_bytes)?;
            }
            Self::RawDsdNative {
                output,
                staging_u32,
                residue_left,
                residue_right,
                frames_written,
                bytes_written,
            } => {
                let (combined_l, combined_r) = if !residue_left.is_empty() {
                    let mut l = std::mem::take(residue_left);
                    let mut r = std::mem::take(residue_right);
                    l.extend_from_slice(left_bytes);
                    r.extend_from_slice(right_bytes);
                    (l, r)
                } else {
                    (left_bytes.to_vec(), right_bytes.to_vec())
                };

                let whole_u32_words = combined_l.len() / 4;
                let whole_bytes = whole_u32_words * 4;

                if whole_u32_words > 0 {
                    let required_bytes = whole_u32_words * STEREO_DSD_U32_FRAME_BYTES;
                    staging_u32.resize(required_bytes, 0);

                    let bytes_packed = pack_stereo_dsd_u32_be(
                        &combined_l[..whole_bytes],
                        &combined_r[..whole_bytes],
                        false,
                        staging_u32,
                    )?;
                    output.write_all(&staging_u32[..bytes_packed])?;
                    *frames_written += (whole_bytes * 8) as u64;
                    *bytes_written += bytes_packed as u64;
                }

                if whole_bytes < combined_l.len() {
                    residue_left.extend_from_slice(&combined_l[whole_bytes..]);
                    residue_right.extend_from_slice(&combined_r[whole_bytes..]);
                }
            }
            Self::RawDsdDop {
                output,
                packer,
                staging_dop,
                residue_left,
                residue_right,
                frames_written,
                bytes_written,
            } => {
                let (combined_l, combined_r) = if !residue_left.is_empty() {
                    let mut l = std::mem::take(residue_left);
                    let mut r = std::mem::take(residue_right);
                    l.extend_from_slice(left_bytes);
                    r.extend_from_slice(right_bytes);
                    (l, r)
                } else {
                    (left_bytes.to_vec(), right_bytes.to_vec())
                };

                let whole_dop_frames = combined_l.len() / 2;
                let whole_bytes = whole_dop_frames * 2;

                if whole_dop_frames > 0 {
                    let frame_bytes = packer.format().bytes_per_stereo_frame();
                    let required_bytes = whole_dop_frames * frame_bytes;
                    staging_dop.resize(required_bytes, 0);

                    let dop_bytes_packed = packer.pack_stereo_frames(
                        &combined_l[..whole_bytes],
                        &combined_r[..whole_bytes],
                        staging_dop,
                    )?;
                    output.write_all(&staging_dop[..dop_bytes_packed])?;
                    *frames_written += (whole_bytes * 8) as u64;
                    *bytes_written += dop_bytes_packed as u64;
                }

                if whole_bytes < combined_l.len() {
                    residue_left.extend_from_slice(&combined_l[whole_bytes..]);
                    residue_right.extend_from_slice(&combined_r[whole_bytes..]);
                }
            }
            _ => {
                return Err(EngineError::Pipeline(
                    "attempted to write DSD bytes to PCM container".into(),
                ));
            }
        }
        Ok(())
    }

    fn finalize(self) -> Result<(u64, u64), EngineError> {
        match self {
            Self::Dsf(w) => {
                let (samples, total_bytes) = w.finalize()?;
                Ok((samples, total_bytes))
            }
            Self::Wav(w) => {
                let frames = w.finalize()?;
                Ok((frames, frames * 8))
            }
            Self::Flac(w) => {
                let frames = w.finalize()?;
                Ok((frames, frames * 8))
            }
            Self::RawDsdNative {
                mut output,
                mut staging_u32,
                mut residue_left,
                mut residue_right,
                mut frames_written,
                mut bytes_written,
            } => {
                if !residue_left.is_empty() {
                    let pad = 4 - residue_left.len();
                    residue_left.resize(4, DSD_IDLE_BYTE);
                    residue_right.resize(4, DSD_IDLE_BYTE);

                    let bytes_packed = pack_stereo_dsd_u32_be(
                        &residue_left,
                        &residue_right,
                        false,
                        &mut staging_u32,
                    )?;
                    output.write_all(&staging_u32[..bytes_packed])?;
                    frames_written += ((4 - pad) * 8) as u64;
                    bytes_written += bytes_packed as u64;
                }
                output.flush()?;
                Ok((frames_written, bytes_written))
            }
            Self::RawDsdDop {
                mut output,
                mut packer,
                mut staging_dop,
                mut residue_left,
                mut residue_right,
                mut frames_written,
                mut bytes_written,
            } => {
                if !residue_left.is_empty() {
                    let pad = 2 - residue_left.len();
                    residue_left.resize(2, DSD_IDLE_BYTE);
                    residue_right.resize(2, DSD_IDLE_BYTE);

                    let frame_bytes = packer.format().bytes_per_stereo_frame();
                    staging_dop.resize(frame_bytes, 0);
                    let dop_bytes_packed = packer.pack_stereo_frames(
                        &residue_left,
                        &residue_right,
                        &mut staging_dop,
                    )?;
                    output.write_all(&staging_dop[..dop_bytes_packed])?;
                    frames_written += ((2 - pad) * 8) as u64;
                    bytes_written += dop_bytes_packed as u64;
                }
                output.flush()?;
                Ok((frames_written, bytes_written))
            }
            Self::RawPcm {
                mut output,
                frames_written,
                bytes_written,
                ..
            } => {
                output.flush()?;
                Ok((frames_written, bytes_written))
            }
        }
    }
}

/// Offline audio conversion pipeline runner.
pub struct OfflinePipeline {
    plan: ResolvedPipelinePlan,
    settings: Settings,
    rational_src: Option<(PolyphaseResampler, PolyphaseResampler)>,
    worker_pool: ChannelWorkerPool,
    tile_size: usize,
    mult: usize,
}

impl OfflinePipeline {
    /// Prepares a new offline pipeline for the given input sample rate and settings.
    pub fn prepare(settings: Settings, input_rate: PcmRateHz) -> Result<Self, EngineError> {
        let plan = settings.explain_for_input_rate(input_rate)?;
        let preset = plan.dsp_config.preset;

        // Determine intermediate baseband rate and power-of-two stages
        let (intermediate_rate, stages) = match plan.stream_spec.target {
            TargetRateSpec::Dsd { bit_rate } => {
                let dsd_family = bit_rate.rate_family();
                let in_family = input_rate.rate_family();
                if in_family == dsd_family {
                    if bit_rate.get() >= input_rate.get() {
                        let mult = bit_rate.get() / input_rate.get();
                        let stages = (mult as f64).log2().round() as usize;
                        (input_rate, stages)
                    } else {
                        return Err(EngineError::Pipeline(format!(
                            "input rate {input_rate} exceeds DSD bit rate {bit_rate}"
                        )));
                    }
                } else {
                    // Cross-family conversion (e.g. 192k 48-family -> 44.1-family DSD):
                    // Resample at matched tier (192k -> 176.4k) to preserve ultrasonic content,
                    // then upsample via power-of-two half-band FIR stages to target DSD bit rate.
                    let matched_tier_rate = input_rate
                        .corresponding_rate_in_family(dsd_family)
                        .filter(|r| bit_rate.get() >= r.get())
                        .or_else(|| dsd_family.base_pcm_rate())
                        .ok_or_else(|| {
                            EngineError::Pipeline(format!(
                                "invalid rate family for DSD bit rate {bit_rate}"
                            ))
                        })?;
                    let mult = bit_rate.get() / matched_tier_rate.get();
                    let stages = (mult as f64).log2().round() as usize;
                    (matched_tier_rate, stages)
                }
            }
            TargetRateSpec::Pcm { rate, .. } => {
                let target_family = rate.rate_family();
                let in_family = input_rate.rate_family();
                if in_family == target_family {
                    let mult = rate.get() / input_rate.get();
                    let stages = if mult >= 1 {
                        (mult as f64).log2().round() as usize
                    } else {
                        0
                    };
                    (input_rate, stages)
                } else {
                    // Cross-family PCM conversion:
                    // If target rate allows, resample at matched tier first, then power-of-two stages
                    let matched_tier_rate = input_rate
                        .corresponding_rate_in_family(target_family)
                        .filter(|r| rate.get() >= r.get())
                        .or_else(|| target_family.base_pcm_rate())
                        .ok_or_else(|| {
                            EngineError::Pipeline(format!("invalid target rate family for {rate}"))
                        })?;
                    let mult = rate.get() / matched_tier_rate.get();
                    let stages = if mult >= 1 {
                        (mult as f64).log2().round() as usize
                    } else {
                        0
                    };
                    (matched_tier_rate, stages)
                }
            }
        };

        // Determine if cross-family rational SRC is required
        let rational_src = if input_rate != intermediate_rate {
            let src_l = PolyphaseResampler::new_rational(input_rate, intermediate_rate)?;
            let src_r = PolyphaseResampler::new_rational(input_rate, intermediate_rate)?;
            Some((src_l, src_r))
        } else {
            None
        };

        const TILE_BASEBAND_FRAMES: usize = 512;
        let target_dsd = match plan.stream_spec.target {
            TargetRateSpec::Dsd { bit_rate } => {
                let seed = settings
                    .offline
                    .as_ref()
                    .and_then(|o| o.seed)
                    .unwrap_or(12345);
                Some(TargetDsdConfig {
                    bit_rate,
                    dither: plan.dsp_config.dither,
                    overload_policy: plan.dsp_config.overload_policy,
                    seed_base: seed,
                })
            }
            TargetRateSpec::Pcm { .. } => None,
        };

        let worker_pool = ChannelWorkerPool::new(
            2,
            preset,
            plan.dsp_config.phase,
            plan.dsp_config.apodization,
            stages,
            TILE_BASEBAND_FRAMES,
            target_dsd,
            None,
        )?;
        let mult = worker_pool.rate_multiplier();

        Ok(Self {
            plan,
            settings,
            rational_src,
            worker_pool,
            tile_size: TILE_BASEBAND_FRAMES,
            mult,
        })
    }

    /// Returns the resolved pipeline plan.
    pub fn plan(&self) -> &ResolvedPipelinePlan {
        &self.plan
    }

    /// Layers a runtime user-volume attenuation (dB) on top of `[gain] preamp_db`.
    ///
    /// Mirrors the daemon's live `set volume`: the value participates in
    /// `total_configured_gain_db()`, feeds the two-pass gain solver, and disables
    /// bit-exact DSD passthrough when non-zero. Must be called before `run`.
    pub fn with_user_volume_db(mut self, db: f64) -> Self {
        self.plan.dsp_config.user_volume_db = db;
        self
    }

    /// Executes the prepared offline conversion pipeline from `input` to `output`.
    pub fn run<'a>(
        mut self,
        input: &mut dyn AudioInput,
        output: Box<dyn AudioOutput + 'a>,
    ) -> Result<OfflineReport, EngineError> {
        let start_time = Instant::now();
        let _ftz = ScopedFtzDaz::new();

        let input_rate = input.info().sample_rate;
        let target_spec = self.plan.stream_spec.target;
        let is_dsd = matches!(target_spec, TargetRateSpec::Dsd { .. });

        let is_input_dsd = input.info().is_dsd();
        let is_target_matching_dsd = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => input.info().dsd_bit_rate == Some(bit_rate),
            _ => false,
        };
        let is_volume_unity = self.plan.dsp_config.user_volume_db.abs() < 1e-6
            && self.plan.dsp_config.preamp_db.abs() < 1e-6;

        if is_input_dsd && is_target_matching_dsd && is_volume_unity {
            // Automatic Opportunistic Bit-Exact DSD Passthrough (Design.md §1.6):
            // The input is DSD, target matches input rate, and software volume attenuation is unity (100%).
            // Direct bit-exact transfer bypassing decimation, filtering, and remodulation.
            let mut container_writer = ContainerWriter::new(&self.settings, &target_spec, output)?;
            let chunk_bytes = 4096;
            let mut dsd_l = vec![0u8; chunk_bytes];
            let mut dsd_r = vec![0u8; chunk_bytes];
            let mut total_input_frames = 0u64;

            loop {
                let bytes_read = input.read_dsd_bytes(&mut [&mut dsd_l, &mut dsd_r])?;
                if bytes_read == 0 {
                    break;
                }
                total_input_frames += (bytes_read * 8) as u64;
                container_writer.write_stereo_dsd(&dsd_l[..bytes_read], &dsd_r[..bytes_read])?;
            }

            let (total_output_frames, total_bytes_written) = container_writer.finalize()?;
            let elapsed_ms = start_time.elapsed().as_millis() as u64;

            return Ok(OfflineReport {
                input_rate,
                target_format: TargetFormat::Dsd,
                total_input_frames,
                total_output_frames,
                total_bytes_written,
                overload_count: 0,
                applied_gain_db: 0.0,
                measured_peak_dbfs: None,
                elapsed_ms,
            });
        }

        let peak_mode = self
            .settings
            .gain
            .as_ref()
            .map(|g| g.peak_analysis)
            .unwrap_or(PeakAnalysisMode::SinglePass);

        // Step 1: Gain Staging (Single-Pass vs. Two-Pass)
        let (applied_gain_db, measured_peak_dbfs) = match peak_mode {
            PeakAnalysisMode::TwoPass => {
                let peak_result = measure_true_peak_pass1(
                    input,
                    self.rational_src.clone(),
                    &mut self.worker_pool,
                    &self.plan.dsp_config,
                    self.tile_size,
                )?;
                (peak_result.calibrated_gain_db, Some(peak_result.peak_dbfs))
            }
            PeakAnalysisMode::SinglePass => (self.plan.dsp_config.total_configured_gain_db(), None),
        };

        let linear_gain = 10.0f64.powf(applied_gain_db / 20.0);

        // Step 2: Tail Policy & Duration Setup
        let tail_policy = self
            .settings
            .offline
            .as_ref()
            .and_then(|o| o.tail_policy)
            .unwrap_or(TailPolicy::TrimToDuration);

        let group_delay_output_samples = self.worker_pool.total_group_delay_output_samples();
        let target_output_samples = input.info().total_frames.map(|frames| match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                (frames as f64 * (bit_rate.get() as f64 / input_rate.get() as f64)).round() as u64
            }
            TargetRateSpec::Pcm { rate, .. } => {
                (frames as f64 * (rate.get() as f64 / input_rate.get() as f64)).round() as u64
            }
        });

        // Round group delay to nearest whole byte (multiple of 8) for seamless DSD streaming
        let aligned_group_delay = if is_dsd {
            ((group_delay_output_samples + 4) / 8) * 8
        } else {
            group_delay_output_samples
        };

        let mut tail_manager =
            TailManager::new(tail_policy, aligned_group_delay, target_output_samples);

        let mut container_writer = ContainerWriter::new(&self.settings, &target_spec, output)?;

        // Pre-allocate bounded buffers
        let tile_size = self.tile_size;
        let mut in_left = vec![0.0; tile_size];
        let mut in_right = vec![0.0; tile_size];

        let mut src_out_left = vec![0.0; tile_size * 2];
        let mut src_out_right = vec![0.0; tile_size * 2];

        let mut total_input_frames = 0u64;

        // Step 3: Stream Processing Loop
        loop {
            let frames_read = input.read_stereo(&mut in_left, &mut in_right)?;
            if frames_read == 0 {
                break;
            }
            total_input_frames += frames_read as u64;

            // Apply pre-modulator gain
            for i in 0..frames_read {
                in_left[i] *= linear_gain;
                in_right[i] *= linear_gain;
            }

            let left_slice = &in_left[..frames_read];
            let right_slice = &in_right[..frames_read];

            match self.rational_src {
                Some((ref mut src_l, ref mut src_r)) => {
                    let (_, prod_l) = src_l.process(left_slice, &mut src_out_left);
                    let (_, prod_r) = src_r.process(right_slice, &mut src_out_right);
                    let prod = prod_l.min(prod_r);

                    let mut offset = 0;
                    while offset < prod {
                        let chunk = (prod - offset).min(tile_size);
                        let in_l = &src_out_left[offset..offset + chunk];
                        let in_r = &src_out_right[offset..offset + chunk];

                        if is_dsd {
                            self.worker_pool.process_tile_dsd(&[in_l, in_r])?;
                        } else {
                            self.worker_pool.process_tile_pcm(&[in_l, in_r])?;
                        }

                        self.process_interpolated_block(
                            chunk,
                            &mut tail_manager,
                            &mut container_writer,
                            &target_spec,
                        )?;

                        offset += chunk;
                    }
                }
                None => {
                    if is_dsd {
                        self.worker_pool
                            .process_tile_dsd(&[left_slice, right_slice])?;
                    } else {
                        self.worker_pool
                            .process_tile_pcm(&[left_slice, right_slice])?;
                    }

                    self.process_interpolated_block(
                        frames_read,
                        &mut tail_manager,
                        &mut container_writer,
                        &target_spec,
                    )?;
                }
            }
        }

        // Set exact target output samples now that input EOF total is known
        let computed_target_output_samples = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => (total_input_frames as f64
                * (bit_rate.get() as f64 / input_rate.get() as f64))
                .round() as u64,
            TargetRateSpec::Pcm { rate, .. } => (total_input_frames as f64
                * (rate.get() as f64 / input_rate.get() as f64))
                .round() as u64,
        };
        tail_manager.set_target_output_samples(computed_target_output_samples);

        // Step 4: Flush Filter Tail with Digital Silence
        let flush_delay_frames = (aligned_group_delay / self.mult) + 48;
        in_left.fill(0.0);
        in_right.fill(0.0);

        let mut flushed = 0;
        while flushed < flush_delay_frames && !tail_manager.is_target_satisfied() {
            let chunk = (flush_delay_frames - flushed).min(tile_size);
            let in_l = &in_left[..chunk];
            let in_r = &in_right[..chunk];

            if is_dsd {
                self.worker_pool.process_tile_dsd(&[in_l, in_r])?;
            } else {
                self.worker_pool.process_tile_pcm(&[in_l, in_r])?;
            }

            self.process_interpolated_block(
                chunk,
                &mut tail_manager,
                &mut container_writer,
                &target_spec,
            )?;

            flushed += chunk;
        }

        // Step 5: Finalize Container
        let (total_output_frames, total_bytes_written) = container_writer.finalize()?;
        let overload_count = self.worker_pool.total_overload_count();
        let elapsed_ms = start_time.elapsed().as_millis() as u64;

        Ok(OfflineReport {
            input_rate,
            target_format: match target_spec {
                TargetRateSpec::Dsd { .. } => TargetFormat::Dsd,
                TargetRateSpec::Pcm { .. } => TargetFormat::Pcm,
            },
            total_input_frames,
            total_output_frames,
            total_bytes_written,
            measured_peak_dbfs,
            applied_gain_db,
            overload_count,
            elapsed_ms,
        })
    }

    #[inline]
    fn process_interpolated_block(
        &mut self,
        chunk_frames: usize,
        tail_manager: &mut TailManager,
        container_writer: &mut ContainerWriter,
        target_spec: &TargetRateSpec,
    ) -> Result<(), EngineError> {
        match target_spec {
            TargetRateSpec::Dsd { .. } => {
                let req_bytes = (chunk_frames * self.mult) / 8;
                let dsd_l = self.worker_pool.channel_dsd(0);
                let dsd_r = self.worker_pool.channel_dsd(1);

                // Discard ramp-up / trim at byte boundary
                if let Some((start_b, end_b)) = tail_manager
                    .process_slice_range(req_bytes * 8)
                    .map(|(s, e)| (s / 8, e / 8))
                {
                    if end_b > start_b {
                        container_writer
                            .write_stereo_dsd(&dsd_l[start_b..end_b], &dsd_r[start_b..end_b])?;
                    }
                }
            }
            TargetRateSpec::Pcm { .. } => {
                let pcm_l = self.worker_pool.channel_pcm(0);
                let pcm_r = self.worker_pool.channel_pcm(1);

                if let Some((start, end)) = tail_manager.process_slice_range(pcm_l.len()) {
                    if end > start {
                        container_writer
                            .write_stereo_pcm(&pcm_l[start..end], &pcm_r[start..end])?;
                    }
                }
            }
        }

        Ok(())
    }
}
