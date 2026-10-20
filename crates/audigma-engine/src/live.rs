//! Real-time live audio streaming pipeline runner (`Design.md §1`, `§3.1–§3.4`, `§4.5`).
//!
//! Orchestrates live streaming daemon execution:
//! - Connects audio input (`PipeWireVirtualSink` or `AlsaCapture`) to output (`PipeWirePcmOutput` or `AlsaAudioOutput`).
//! - Decoupled through an SPSC ring buffer (`AudioSpscQueue`) with monotonic generation tags (`u64`).
//! - DSP processing worker thread:
//!   - Pre-modulator gain and dynamic runtime volume.
//!   - Low-rate baseband polyphase ASRC drift tracking (`clock.mode = "adaptive-input"`).
//!   - Power-of-two half-band FIR interpolation cascade ($2^K$).
//!   - 5th-order CRFB Delta-Sigma Modulator or PCM quantizer.
//!   - Click-free lifecycle transitions (`LifecycleController`).
//! - Real-time thread priority acquisition with graceful fallback.
//! - Monitored live telemetry exported via shared state.

use crate::asrc::{AdaptiveClockDriftController, ClockDriftConfig};
use crate::lifecycle::{IdleCarrierFormat, LifecycleController};
use crate::traits::{AudioInput, AudioOutput, EngineError};
use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_dsp::fir::{HalfBandCascade, PolyphaseResampler};
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::modulator::CrfbModulator;
use audigma_dsp::rates::PcmRateHz;
use audigma_dsp::tables;
use audigma_format::dop::{DopContainerFormat, DopPacker};
use audigma_format::dsd_u32::{STEREO_DSD_U32_FRAME_BYTES, pack_stereo_dsd_u32_be};
use audigma_settings::{OutputTransport, ResolvedPipelinePlan, Settings};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Live engine runtime telemetry snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonTelemetry {
    pub state: String,
    pub input_rate_hz: u32,
    pub output_format: String,
    pub output_rate_hz: u32,
    pub preset: String,
    pub volume_db: f64,
    pub preamp_db: f64,
    pub total_input_frames: u64,
    pub total_output_frames: u64,
    pub queue_fill_pct: f64,
    pub drift_ppm: f64,
    pub overload_count: u64,
    pub xrun_count: u64,
    pub generation: u64,
    pub uptime_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disconnect_duration_ms: Option<u64>,
}

/// Shared control and telemetry state handle for the live daemon.
#[derive(Clone)]
pub struct DaemonControlHandle {
    pub running: Arc<AtomicBool>,
    pub generation: Arc<AtomicU64>,
    pub volume_db: Arc<Mutex<f64>>,
    pub target_preset: Arc<Mutex<Option<String>>>,
    pub telemetry: Arc<Mutex<DaemonTelemetry>>,
}

impl DaemonControlHandle {
    pub fn new(
        input_rate: u32,
        output_format: &str,
        output_rate: u32,
        preset: &str,
        preamp_db: f64,
    ) -> Self {
        let telemetry = DaemonTelemetry {
            state: "Stopped".to_string(),
            input_rate_hz: input_rate,
            output_format: output_format.to_string(),
            output_rate_hz: output_rate,
            preset: preset.to_string(),
            volume_db: 0.0,
            preamp_db,
            total_input_frames: 0,
            total_output_frames: 0,
            queue_fill_pct: 0.0,
            drift_ppm: 0.0,
            overload_count: 0,
            xrun_count: 0,
            generation: 1,
            uptime_secs: 0,
            device_target: None,
            disconnect_duration_ms: None,
        };

        Self {
            running: Arc::new(AtomicBool::new(true)),
            generation: Arc::new(AtomicU64::new(1)),
            volume_db: Arc::new(Mutex::new(0.0)),
            target_preset: Arc::new(Mutex::new(None)),
            telemetry: Arc::new(Mutex::new(telemetry)),
        }
    }

    #[inline]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    #[inline]
    pub fn get_volume(&self) -> f64 {
        *self.volume_db.lock().unwrap()
    }

    #[inline]
    pub fn set_volume(&self, db: f64) {
        let mut vol = self.volume_db.lock().unwrap();
        *vol = db;
        let mut telem = self.telemetry.lock().unwrap();
        telem.volume_db = db;
    }

    #[inline]
    pub fn set_preset(&self, name: &str) {
        let mut p = self.target_preset.lock().unwrap();
        *p = Some(name.to_string());
    }

    #[inline]
    pub fn get_telemetry(&self) -> DaemonTelemetry {
        self.telemetry.lock().unwrap().clone()
    }

    #[inline]
    pub fn set_state(&self, state: &str) {
        let mut telem = self.telemetry.lock().unwrap();
        telem.state = state.to_string();
    }

    #[inline]
    pub fn set_disconnect_info(&self, target: Option<String>, duration_ms: Option<u64>) {
        let mut telem = self.telemetry.lock().unwrap();
        telem.device_target = target;
        telem.disconnect_duration_ms = duration_ms;
    }
}

/// Prepared real-time live streaming pipeline.
#[derive(Clone)]
pub struct LivePipeline {
    settings: Settings,
    plan: ResolvedPipelinePlan,
    input_rate: PcmRateHz,
    intermediate_rate: PcmRateHz,
    cascade_stages: usize,
    carrier: IdleCarrierFormat,
}

impl LivePipeline {
    pub fn prepare(settings: Settings, input_rate: PcmRateHz) -> Result<Self, EngineError> {
        let plan = settings.explain_for_input_rate(input_rate)?;
        let target_spec = plan.stream_spec.target;

        let (intermediate_rate, stages) = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                let dsd_family = bit_rate.rate_family();
                let in_family = input_rate.rate_family();
                if in_family == dsd_family {
                    let mult = bit_rate.get() / input_rate.get();
                    let stages = (mult as f64).log2().round() as usize;
                    (input_rate, stages)
                } else {
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

        let carrier = match target_spec {
            TargetRateSpec::Dsd { .. } => {
                let transport = settings
                    .output
                    .as_ref()
                    .and_then(|o| o.transport)
                    .unwrap_or(OutputTransport::Native);
                match transport {
                    OutputTransport::Native => IdleCarrierFormat::NativeDsd,
                    OutputTransport::Dop => {
                        let fmt = settings
                            .output
                            .as_ref()
                            .and_then(|o| o.sample_format)
                            .unwrap_or(SampleFormat::S32Le);
                        let dop_fmt = match fmt {
                            SampleFormat::S32Le => DopContainerFormat::S32Le,
                            SampleFormat::S24Le => DopContainerFormat::S24Le,
                            _ => DopContainerFormat::S24_3Le,
                        };
                        IdleCarrierFormat::DoP(dop_fmt)
                    }
                    OutputTransport::Pcm => IdleCarrierFormat::Pcm,
                }
            }
            TargetRateSpec::Pcm { .. } => IdleCarrierFormat::Pcm,
        };

        Ok(Self {
            settings,
            plan,
            input_rate,
            intermediate_rate,
            cascade_stages: stages,
            carrier,
        })
    }

    /// Runs the streaming loop reading from `input`, converting, and writing to `output`.
    pub fn run_streaming(
        &self,
        input: &mut dyn AudioInput,
        output: &mut dyn AudioOutput,
        control: DaemonControlHandle,
    ) -> Result<(), EngineError> {
        let _ftz = ScopedFtzDaz::new();
        let target_spec = self.plan.stream_spec.target;
        let preset = self.plan.dsp_config.preset;

        const TILE_BASEBAND_FRAMES: usize = 256;
        let mut cascade_left =
            HalfBandCascade::new(preset, self.cascade_stages, TILE_BASEBAND_FRAMES);
        let mut cascade_right =
            HalfBandCascade::new(preset, self.cascade_stages, TILE_BASEBAND_FRAMES);
        let mult = cascade_left.rate_multiplier();

        // Clock mode & low-rate ASRC
        let is_adaptive = self
            .settings
            .clock
            .as_ref()
            .map(|c| c.mode == audigma_settings::ClockMode::AdaptiveInput)
            .unwrap_or(false);

        let mut asrc = if is_adaptive {
            let max_ppm = self
                .settings
                .clock
                .as_ref()
                .and_then(|c| c.max_correction_ppm)
                .map(|p| p as f64)
                .unwrap_or(500.0);
            Some((
                PolyphaseResampler::new_asrc(self.input_rate, self.intermediate_rate),
                PolyphaseResampler::new_asrc(self.input_rate, self.intermediate_rate),
                AdaptiveClockDriftController::new(ClockDriftConfig {
                    target_fill: 0.5,
                    max_correction_ppm: max_ppm,
                    ..Default::default()
                }),
            ))
        } else if self.input_rate != self.intermediate_rate {
            Some((
                PolyphaseResampler::new_rational(self.input_rate, self.intermediate_rate)?,
                PolyphaseResampler::new_rational(self.input_rate, self.intermediate_rate)?,
                AdaptiveClockDriftController::new(ClockDriftConfig::default()),
            ))
        } else {
            None
        };

        // Modulators for DSD
        let mut mod_left = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                let coeffs = tables::get_crfb_coefficients(bit_rate).ok_or_else(|| {
                    EngineError::Pipeline(format!(
                        "no verified CRFB coefficients for bit rate {bit_rate}"
                    ))
                })?;
                Some(CrfbModulator::new(
                    *coeffs,
                    self.plan.dsp_config.dither,
                    self.plan.dsp_config.overload_policy,
                ))
            }
            TargetRateSpec::Pcm { .. } => None,
        };

        let mut mod_right = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                let coeffs = tables::get_crfb_coefficients(bit_rate).ok_or_else(|| {
                    EngineError::Pipeline(format!(
                        "no verified CRFB coefficients for bit rate {bit_rate}"
                    ))
                })?;
                Some(CrfbModulator::new(
                    *coeffs,
                    self.plan.dsp_config.dither,
                    self.plan.dsp_config.overload_policy,
                ))
            }
            TargetRateSpec::Pcm { .. } => None,
        };

        let mut dop_packer = match self.carrier {
            IdleCarrierFormat::DoP(fmt) => Some(DopPacker::new(fmt)),
            _ => None,
        };

        let mut lifecycle = LifecycleController::new(self.carrier, 256, 256);
        let _ = lifecycle.start();

        let cascade_tile_len = TILE_BASEBAND_FRAMES * mult;
        let mut in_left = vec![0.0; TILE_BASEBAND_FRAMES];
        let mut in_right = vec![0.0; TILE_BASEBAND_FRAMES];
        let mut src_out_left = vec![0.0; TILE_BASEBAND_FRAMES * 2];
        let mut src_out_right = vec![0.0; TILE_BASEBAND_FRAMES * 2];
        let mut cascade_out_left = vec![0.0; cascade_tile_len];
        let mut cascade_out_right = vec![0.0; cascade_tile_len];
        let mut dsd_bytes_left = vec![0u8; cascade_tile_len / 8];
        let mut dsd_bytes_right = vec![0u8; cascade_tile_len / 8];
        let mut transport_staging = vec![0u8; cascade_tile_len * 4];

        let start_time = Instant::now();
        let mut total_input_frames = 0u64;
        let mut total_output_frames = 0u64;
        let mut overload_events = 0u64;

        let base_preamp = self.plan.dsp_config.preamp_db;
        let profile_headroom = self.plan.dsp_config.profile_headroom_db;

        {
            let mut telem = control.telemetry.lock().unwrap();
            telem.state = format!("{:?}", lifecycle.state());
        }

        while control.is_running() {
            // Read input frames
            let frames_read = input.read_stereo(&mut in_left, &mut in_right)?;
            if frames_read == 0 {
                // When input is temporarily idle or quiet in live streaming, sleep briefly to avoid spin
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            total_input_frames += frames_read as u64;

            // Compute current dynamic gain
            let user_vol = control.get_volume();
            let total_gain_db = base_preamp + profile_headroom + user_vol;
            let linear_gain = 10.0_f64.powf(total_gain_db / 20.0);

            // Apply pre-SRC gain and lifecycle cosine ramping
            let _action = lifecycle
                .process_input_block(&mut in_left[..frames_read], &mut in_right[..frames_read]);
            for i in 0..frames_read {
                in_left[i] *= linear_gain;
                in_right[i] *= linear_gain;
            }

            let left_slice = &in_left[..frames_read];
            let right_slice = &in_right[..frames_read];

            // Polyphase SRC / ASRC
            let (proc_left, proc_right, proc_len) =
                if let Some((ref mut src_l, ref mut src_r, ref mut drift_ctrl)) = asrc {
                    let (_, prod_l) = src_l.process(left_slice, &mut src_out_left);
                    let (_, prod_r) = src_r.process(right_slice, &mut src_out_right);
                    let prod = prod_l.min(prod_r);

                    // Update drift controller
                    let dt = frames_read as f64 / self.input_rate.get() as f64;
                    if let Ok(ppm) = drift_ctrl.update(128, 256, dt) {
                        src_l.set_drift_ppm(ppm);
                        src_r.set_drift_ppm(ppm);
                    }

                    (&src_out_left[..prod], &src_out_right[..prod], prod)
                } else {
                    (left_slice, right_slice, frames_read)
                };

            // Interpolation & Modulation in bounded tiles
            let mut offset = 0;
            while offset < proc_len {
                let tile = (proc_len - offset).min(TILE_BASEBAND_FRAMES);
                let req_out = tile * mult;

                cascade_left.process_tile(
                    &proc_left[offset..offset + tile],
                    &mut cascade_out_left[..req_out],
                )?;
                cascade_right.process_tile(
                    &proc_right[offset..offset + tile],
                    &mut cascade_out_right[..req_out],
                )?;

                match (&mut mod_left, &mut mod_right) {
                    (Some(ml), Some(mr)) => {
                        let bytes_req = req_out / 8;
                        let out_l = &mut dsd_bytes_left[..bytes_req];
                        let out_r = &mut dsd_bytes_right[..bytes_req];

                        let init_ov_l = ml.overload_count();
                        let init_ov_r = mr.overload_count();

                        ml.process_tile_bytes(&cascade_out_left[..req_out], out_l)?;
                        mr.process_tile_bytes(&cascade_out_right[..req_out], out_r)?;

                        if ml.overload_count() > init_ov_l || mr.overload_count() > init_ov_r {
                            lifecycle.report_overload_trip();
                            overload_events += 1;
                        }

                        // Pack and emit
                        let write_res: Result<(), EngineError> = match self.carrier {
                            IdleCarrierFormat::NativeDsd => {
                                let packed_bytes = pack_stereo_dsd_u32_be(
                                    out_l,
                                    out_r,
                                    false,
                                    &mut transport_staging,
                                )?;
                                output
                                    .write_all(&transport_staging[..packed_bytes])
                                    .map_err(|e| {
                                        let code = e.raw_os_error().unwrap_or(0);
                                        if e.kind() == std::io::ErrorKind::BrokenPipe
                                            || code == libc::ENODEV
                                            || code == -libc::ENODEV
                                            || code == libc::EBADFD
                                            || code == -libc::EBADFD
                                            || code == libc::EIO
                                            || code == -libc::EIO
                                        {
                                            EngineError::DeviceDisconnected(e.to_string())
                                        } else {
                                            EngineError::Io(e)
                                        }
                                    })?;

                                total_output_frames += req_out as u64;
                                Ok(())
                            }
                            IdleCarrierFormat::DoP(_) => {
                                if let Some(ref mut packer) = dop_packer {
                                    let packed_bytes = packer.pack_stereo_frames(
                                        out_l,
                                        out_r,
                                        &mut transport_staging,
                                    )?;
                                    output
                                        .write_all(&transport_staging[..packed_bytes])
                                        .map_err(|e| {
                                            let code = e.raw_os_error().unwrap_or(0);
                                            if e.kind() == std::io::ErrorKind::BrokenPipe
                                                || code == libc::ENODEV
                                                || code == -libc::ENODEV
                                                || code == libc::EBADFD
                                                || code == -libc::EBADFD
                                                || code == libc::EIO
                                                || code == -libc::EIO
                                            {
                                                EngineError::DeviceDisconnected(e.to_string())
                                            } else {
                                                EngineError::Io(e)
                                            }
                                        })?;

                                    total_output_frames += req_out as u64;
                                }
                                Ok(())
                            }
                            IdleCarrierFormat::Pcm => Ok(()),
                        };
                        write_res?;
                    }
                    _ => {
                        // High-rate PCM output
                        let mut pcm_bytes = Vec::with_capacity(req_out * 8);
                        for i in 0..req_out {
                            let fl = (cascade_out_left[i] as f32).to_le_bytes();
                            let fr = (cascade_out_right[i] as f32).to_le_bytes();
                            pcm_bytes.extend_from_slice(&fl);
                            pcm_bytes.extend_from_slice(&fr);
                        }
                        output.write_all(&pcm_bytes).map_err(|e| {
                            let code = e.raw_os_error().unwrap_or(0);
                            if e.kind() == std::io::ErrorKind::BrokenPipe
                                || code == libc::ENODEV
                                || code == -libc::ENODEV
                                || code == libc::EBADFD
                                || code == -libc::EBADFD
                                || code == libc::EIO
                                || code == -libc::EIO
                            {
                                EngineError::DeviceDisconnected(e.to_string())
                            } else {
                                EngineError::Io(e)
                            }
                        })?;

                        total_output_frames += req_out as u64;
                    }
                }

                offset += tile;
            }

            // Periodic telemetry update
            if total_input_frames % 8192 < frames_read as u64 {
                let mut telem = control.telemetry.lock().unwrap();
                telem.state = format!("{:?}", lifecycle.state());
                telem.total_input_frames = total_input_frames;
                telem.total_output_frames = total_output_frames;
                telem.overload_count = overload_events;
                telem.uptime_secs = start_time.elapsed().as_secs();
            }
        }

        // Clean planned stop
        lifecycle.request_stop();
        {
            let mut telem = control.telemetry.lock().unwrap();
            telem.state = "DrainingMuting".to_string();
        }

        // Emit silence/tail frames to guarantee zero clicks
        let tail_bytes = 1024
            * match self.carrier {
                IdleCarrierFormat::NativeDsd => STEREO_DSD_U32_FRAME_BYTES,
                IdleCarrierFormat::DoP(fmt) => fmt.bytes_per_stereo_frame(),
                IdleCarrierFormat::Pcm => 8,
            };
        let mut tail_buf = vec![0u8; tail_bytes];
        lifecycle.emit_idle_frames(&mut tail_buf);
        let _ = output.write_all(&tail_buf);
        let _ = output.flush();

        {
            let mut telem = control.telemetry.lock().unwrap();
            telem.state = "Stopped".to_string();
        }

        Ok(())
    }
}
