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
use crate::lifecycle::{BlockAction, IdleCarrierFormat, LifecycleController, LifecycleState};
use crate::traits::{AudioInput, AudioOutput, EngineError};
use crate::worker::{ChannelWorkerPool, TargetDsdConfig};
use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_dsp::fir::PolyphaseResampler;
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::modulator::TpdfDither;
use audigma_dsp::rates::PcmRateHz;
use audigma_format::dop::{DopContainerFormat, DopPacker};
use audigma_format::dsd_u32::{STEREO_DSD_U32_FRAME_BYTES, pack_stereo_dsd_u32_be};
use audigma_settings::{OutputTransport, ResolvedPipelinePlan, Settings};
use smallvec::SmallVec;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Number of channel slices stored inline by the live loop's scratch lists.
///
/// Typical 2/6/8-channel streams stay entirely on the stack; larger counts spill
/// to the heap rather than imposing a hard channel limit.
const INLINE_CHANNELS: usize = 16;

type ChannelMutSlices<'a> = SmallVec<[&'a mut [f64]; INLINE_CHANNELS]>;
type ChannelSlices<'a> = SmallVec<[&'a [f64]; INLINE_CHANNELS]>;

/// Sentinel stored in `volume_percent` when the user selected a dB value instead
/// of a percentage. `NaN` is never a valid percentage.
const PERCENT_UNSET_BITS: u64 = f64::NAN.to_bits();

/// Sentinel stored in `disconnect_duration_ms` when the daemon is connected.
const DISCONNECT_NONE: u64 = u64::MAX;

#[inline]
fn load_f64(value: &AtomicU64) -> f64 {
    f64::from_bits(value.load(Ordering::Relaxed))
}

#[inline]
fn store_f64(value: &AtomicU64, v: f64) {
    value.store(v.to_bits(), Ordering::Relaxed);
}

/// High-level daemon state exposed over telemetry and the control socket.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DaemonState {
    Stopped,
    WaitingForDevice,
    Preparing,
    Priming,
    Running,
    DrainingMuting,
    Reconfiguring,
    Recovering,
    DeviceLost,
}

impl DaemonState {
    /// Stable human/JSON-facing name for this state.
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::WaitingForDevice => "WaitingForDevice",
            Self::Preparing => "Preparing",
            Self::Priming => "Priming",
            Self::Running => "Running",
            Self::DrainingMuting => "DrainingMuting",
            Self::Reconfiguring => "Reconfiguring",
            Self::Recovering => "Recovering",
            Self::DeviceLost => "DeviceLost",
        }
    }

    #[inline]
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::WaitingForDevice,
            2 => Self::Preparing,
            3 => Self::Priming,
            4 => Self::Running,
            5 => Self::DrainingMuting,
            6 => Self::Reconfiguring,
            7 => Self::Recovering,
            8 => Self::DeviceLost,
            _ => Self::Stopped,
        }
    }
}

impl std::fmt::Display for DaemonState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<LifecycleState> for DaemonState {
    fn from(state: LifecycleState) -> Self {
        match state {
            LifecycleState::Stopped => Self::Stopped,
            LifecycleState::Preparing => Self::Preparing,
            LifecycleState::Priming => Self::Priming,
            LifecycleState::Running => Self::Running,
            LifecycleState::DrainingMuting => Self::DrainingMuting,
            LifecycleState::Reconfiguring => Self::Reconfiguring,
            LifecycleState::Recovering => Self::Recovering,
            LifecycleState::DeviceLost => Self::DeviceLost,
        }
    }
}

/// Active volume control strategy for the live daemon.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonVolumeMode {
    Software,
    Hardware,
    None,
}

impl DaemonVolumeMode {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Hardware => "hardware",
            Self::None => "none",
        }
    }

    #[inline]
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Hardware,
            2 => Self::None,
            _ => Self::Software,
        }
    }
}

impl std::fmt::Display for DaemonVolumeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Active output transport format for the live daemon.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonOutputFormat {
    Pcm,
    Dsd,
}

impl DaemonOutputFormat {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pcm => "pcm",
            Self::Dsd => "dsd",
        }
    }

    #[inline]
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Dsd,
            _ => Self::Pcm,
        }
    }
}

impl std::fmt::Display for DaemonOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Live engine runtime telemetry snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DaemonTelemetry {
    pub state: DaemonState,
    pub input_rate_hz: u32,
    pub output_format: DaemonOutputFormat,
    pub output_rate_hz: u32,
    pub preset: String,
    pub volume_db: f64,
    pub volume_percent: f64,
    pub volume_mode: DaemonVolumeMode,
    pub is_muted: bool,
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

/// Lock-free telemetry storage shared with the IPC thread.
///
/// The live audio loop only ever touches atomics here. The one remaining mutex
/// guards the device target string, which is written by the daemon supervisor
/// and read by the IPC thread, never by the streaming loop.
struct TelemetryState {
    state: AtomicU8,
    input_rate_hz: AtomicU32,
    output_format: AtomicU8,
    output_rate_hz: AtomicU32,
    preset: String,
    preamp_db: AtomicU64,
    volume_db: AtomicU64,
    volume_percent: AtomicU64,
    total_input_frames: AtomicU64,
    total_output_frames: AtomicU64,
    queue_fill_pct: AtomicU64,
    drift_ppm: AtomicU64,
    overload_count: AtomicU64,
    xrun_count: AtomicU64,
    uptime_secs: AtomicU64,
    device_target: Mutex<Option<String>>,
    disconnect_duration_ms: AtomicU64,
}

/// Shared control and telemetry state handle for the live daemon.
#[derive(Clone)]
pub struct DaemonControlHandle {
    pub running: Arc<AtomicBool>,
    pub generation: Arc<AtomicU64>,
    volume_db: Arc<AtomicU64>,
    volume_percent: Arc<AtomicU64>,
    volume_mode: Arc<AtomicU8>,
    is_muted: Arc<AtomicBool>,
    target_preset: Arc<Mutex<Option<String>>>,
    telemetry: Arc<TelemetryState>,
}

impl DaemonControlHandle {
    pub fn new(
        input_rate: u32,
        output_format: DaemonOutputFormat,
        output_rate: u32,
        preset: &str,
        preamp_db: f64,
    ) -> Self {
        let telemetry = TelemetryState {
            state: AtomicU8::new(DaemonState::Stopped as u8),
            input_rate_hz: AtomicU32::new(input_rate),
            output_format: AtomicU8::new(output_format as u8),
            output_rate_hz: AtomicU32::new(output_rate),
            preset: preset.to_string(),
            preamp_db: AtomicU64::new(preamp_db.to_bits()),
            volume_db: AtomicU64::new(0.0f64.to_bits()),
            volume_percent: AtomicU64::new(100.0f64.to_bits()),
            total_input_frames: AtomicU64::new(0),
            total_output_frames: AtomicU64::new(0),
            queue_fill_pct: AtomicU64::new(0.0f64.to_bits()),
            drift_ppm: AtomicU64::new(0.0f64.to_bits()),
            overload_count: AtomicU64::new(0),
            xrun_count: AtomicU64::new(0),
            uptime_secs: AtomicU64::new(0),
            device_target: Mutex::new(None),
            disconnect_duration_ms: AtomicU64::new(DISCONNECT_NONE),
        };

        Self {
            running: Arc::new(AtomicBool::new(true)),
            generation: Arc::new(AtomicU64::new(1)),
            volume_db: Arc::new(AtomicU64::new(0.0f64.to_bits())),
            volume_percent: Arc::new(AtomicU64::new(100.0f64.to_bits())),
            volume_mode: Arc::new(AtomicU8::new(DaemonVolumeMode::Software as u8)),
            is_muted: Arc::new(AtomicBool::new(false)),
            target_preset: Arc::new(Mutex::new(None)),
            telemetry: Arc::new(telemetry),
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
        load_f64(&self.volume_db)
    }

    #[inline]
    pub fn get_volume_percent(&self) -> Option<f64> {
        let pct = load_f64(&self.volume_percent);
        (!pct.is_nan()).then_some(pct)
    }

    /// Sets the live volume in dB, clamped to the representable range.
    ///
    /// Clamping happens daemon-side (not only in the CLI) because IPC clients
    /// and desktop volume sliders can supply arbitrary values.
    #[inline]
    pub fn set_volume(&self, db: f64) {
        let db = if db.is_nan() {
            audigma_dsp::MIN_VOLUME_DB
        } else {
            db.clamp(audigma_dsp::MIN_VOLUME_DB, audigma_dsp::MAX_VOLUME_DB)
        };
        store_f64(&self.volume_db, db);
        self.volume_percent
            .store(PERCENT_UNSET_BITS, Ordering::Relaxed);
        store_f64(&self.telemetry.volume_db, db);
    }

    #[inline]
    pub fn set_volume_percent(&self, pct: f64) {
        if pct.is_nan() {
            self.volume_percent
                .store(PERCENT_UNSET_BITS, Ordering::Relaxed);
            return;
        }
        store_f64(&self.volume_percent, pct);
        store_f64(&self.telemetry.volume_percent, pct);
    }

    #[inline]
    pub fn sync_telemetry(&self, db: f64, pct: f64) {
        store_f64(&self.telemetry.volume_db, db);
        store_f64(&self.telemetry.volume_percent, pct);
    }

    #[inline]
    pub fn get_volume_mode(&self) -> DaemonVolumeMode {
        DaemonVolumeMode::from_u8(self.volume_mode.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_volume_mode(&self, mode: DaemonVolumeMode) {
        self.volume_mode.store(mode as u8, Ordering::Relaxed);
    }

    #[inline]
    pub fn is_muted(&self) -> bool {
        self.is_muted.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn set_muted(&self, muted: bool) {
        self.is_muted.store(muted, Ordering::SeqCst);
    }

    #[inline]
    pub fn set_preset(&self, name: &str) {
        let mut p = self.target_preset.lock().unwrap();
        *p = Some(name.to_string());
    }

    #[inline]
    pub fn state(&self) -> DaemonState {
        DaemonState::from_u8(self.telemetry.state.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn set_state(&self, state: DaemonState) {
        self.telemetry.state.store(state as u8, Ordering::Relaxed);
    }

    #[inline]
    pub fn set_lifecycle_state(&self, state: LifecycleState) {
        self.set_state(state.into());
    }

    /// Returns a consistent snapshot of the current daemon telemetry.
    #[inline]
    pub fn get_telemetry(&self) -> DaemonTelemetry {
        let device_target = self.telemetry.device_target.lock().unwrap().clone();
        let disconnect_duration_ms = match self
            .telemetry
            .disconnect_duration_ms
            .load(Ordering::Relaxed)
        {
            DISCONNECT_NONE => None,
            ms => Some(ms),
        };

        DaemonTelemetry {
            state: self.state(),
            input_rate_hz: self.telemetry.input_rate_hz.load(Ordering::Relaxed),
            output_format: DaemonOutputFormat::from_u8(
                self.telemetry.output_format.load(Ordering::Relaxed),
            ),
            output_rate_hz: self.telemetry.output_rate_hz.load(Ordering::Relaxed),
            preset: self.telemetry.preset.clone(),
            volume_db: load_f64(&self.telemetry.volume_db),
            volume_percent: load_f64(&self.telemetry.volume_percent),
            volume_mode: self.get_volume_mode(),
            is_muted: self.is_muted(),
            preamp_db: load_f64(&self.telemetry.preamp_db),
            total_input_frames: self.telemetry.total_input_frames.load(Ordering::Relaxed),
            total_output_frames: self.telemetry.total_output_frames.load(Ordering::Relaxed),
            queue_fill_pct: load_f64(&self.telemetry.queue_fill_pct),
            drift_ppm: load_f64(&self.telemetry.drift_ppm),
            overload_count: self.telemetry.overload_count.load(Ordering::Relaxed),
            xrun_count: self.telemetry.xrun_count.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
            uptime_secs: self.telemetry.uptime_secs.load(Ordering::Relaxed),
            device_target,
            disconnect_duration_ms,
        }
    }

    #[inline]
    pub fn set_disconnect_info(&self, target: Option<String>, duration_ms: Option<u64>) {
        *self.telemetry.device_target.lock().unwrap() = target;
        self.telemetry
            .disconnect_duration_ms
            .store(duration_ms.unwrap_or(DISCONNECT_NONE), Ordering::Relaxed);
    }

    /// Updates the input/output stream rates and target format after a reconfiguration.
    #[inline]
    pub fn set_stream_config(
        &self,
        input_rate_hz: u32,
        output_format: DaemonOutputFormat,
        output_rate_hz: u32,
    ) {
        self.telemetry
            .input_rate_hz
            .store(input_rate_hz, Ordering::Relaxed);
        self.telemetry
            .output_format
            .store(output_format as u8, Ordering::Relaxed);
        self.telemetry
            .output_rate_hz
            .store(output_rate_hz, Ordering::Relaxed);
    }

    /// Updates runtime counters without taking any locks.
    #[inline]
    pub fn update_runtime_stats(
        &self,
        total_input_frames: u64,
        total_output_frames: u64,
        overload_count: u64,
        queue_fill_pct: Option<f64>,
        uptime_secs: u64,
    ) {
        self.telemetry
            .total_input_frames
            .store(total_input_frames, Ordering::Relaxed);
        self.telemetry
            .total_output_frames
            .store(total_output_frames, Ordering::Relaxed);
        self.telemetry
            .overload_count
            .store(overload_count, Ordering::Relaxed);
        if let Some(pct) = queue_fill_pct {
            store_f64(&self.telemetry.queue_fill_pct, pct);
        }
        self.telemetry
            .uptime_secs
            .store(uptime_secs, Ordering::Relaxed);
    }
}

/// High-level reason why `run_streaming` exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamExitReason {
    /// Playback was stopped via `control.stop()`.
    Stopped,
    /// An upstream sample rate transition was detected and filter tails have been smoothly drained.
    RateChanged { new_rate: PcmRateHz, channels: u8 },
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
    thread_init_hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
}

impl LivePipeline {
    pub fn prepare(settings: Settings, input_rate: PcmRateHz) -> Result<Self, EngineError> {
        Self::prepare_with_hook(settings, input_rate, None)
    }

    pub fn prepare_with_hook(
        settings: Settings,
        input_rate: PcmRateHz,
        thread_init_hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    ) -> Result<Self, EngineError> {
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
            thread_init_hook,
        })
    }

    /// Sets or replaces the thread initialization hook for channel worker threads.
    pub fn set_thread_init_hook(&mut self, hook: Option<Arc<dyn Fn(usize) + Send + Sync>>) {
        self.thread_init_hook = hook;
    }

    /// Runs the streaming loop reading from `input`, converting, and writing to `output`.
    pub fn run_streaming(
        &self,
        input: &mut dyn AudioInput,
        output: &mut dyn AudioOutput,
        control: DaemonControlHandle,
    ) -> Result<StreamExitReason, EngineError> {
        self.run_streaming_with_bridge(input, output, control, None)
    }

    /// Runs the streaming loop with an optional hardware volume bridge (`Design.md §1.8`).
    pub fn run_streaming_with_bridge(
        &self,
        input: &mut dyn AudioInput,
        output: &mut dyn AudioOutput,
        control: DaemonControlHandle,
        mut hw_bridge: Option<&mut dyn crate::traits::HardwareVolumeBridge>,
    ) -> Result<StreamExitReason, EngineError> {
        let _ftz = ScopedFtzDaz::new();
        let target_spec = self.plan.stream_spec.target;
        let preset = self.plan.dsp_config.preset;
        let num_channels = self.plan.stream_spec.channels as usize;

        const TILE_BASEBAND_FRAMES: usize = 256;
        let is_dsd = matches!(target_spec, TargetRateSpec::Dsd { .. });
        let target_dsd = match target_spec {
            TargetRateSpec::Dsd { bit_rate } => Some(TargetDsdConfig {
                bit_rate,
                dither: self.plan.dsp_config.dither,
                overload_policy: self.plan.dsp_config.overload_policy,
                seed_base: 12345,
            }),
            TargetRateSpec::Pcm { .. } => None,
        };

        let mut worker_pool = ChannelWorkerPool::new(
            num_channels,
            preset,
            self.plan.dsp_config.phase,
            self.plan.dsp_config.apodization,
            self.cascade_stages,
            TILE_BASEBAND_FRAMES,
            target_dsd,
            self.thread_init_hook.clone(),
        )?;
        let mult = worker_pool.rate_multiplier();

        let pcm_sample_format = match target_spec {
            TargetRateSpec::Pcm { sample_format, .. } => sample_format,
            _ => SampleFormat::F32,
        };
        let mut pcm_dithers: Vec<TpdfDither> = if matches!(
            pcm_sample_format,
            SampleFormat::S16Le | SampleFormat::S24Le | SampleFormat::S24_3Le | SampleFormat::S32Le
        ) {
            (0..num_channels)
                .map(|c| TpdfDither::new(0x1234_5678_9abc_def0 + c as u64, 0.5))
                .collect()
        } else {
            Vec::new()
        };
        // Clock mode & low-rate ASRC
        let is_adaptive = self
            .settings
            .clock
            .as_ref()
            .map(|c| c.mode == audigma_settings::ClockMode::AdaptiveInput)
            .unwrap_or(false);

        let mut asrc_resamplers: Vec<PolyphaseResampler> = Vec::new();
        let mut asrc_drift_ctrl: Option<AdaptiveClockDriftController> = None;

        if is_adaptive {
            let max_ppm = self
                .settings
                .clock
                .as_ref()
                .and_then(|c| c.max_correction_ppm)
                .map(|p| p as f64)
                .unwrap_or(500.0);
            for _ in 0..num_channels {
                asrc_resamplers.push(PolyphaseResampler::new_asrc(
                    self.input_rate,
                    self.intermediate_rate,
                ));
            }
            asrc_drift_ctrl = Some(AdaptiveClockDriftController::new(ClockDriftConfig {
                target_fill: 0.5,
                max_correction_ppm: max_ppm,
                ..Default::default()
            }));
        } else if self.input_rate != self.intermediate_rate {
            for _ in 0..num_channels {
                asrc_resamplers.push(PolyphaseResampler::new_rational(
                    self.input_rate,
                    self.intermediate_rate,
                )?);
            }
        }

        let mut dop_packer = match self.carrier {
            IdleCarrierFormat::DoP(fmt) => Some(DopPacker::new(fmt)),
            _ => None,
        };

        let mut lifecycle = LifecycleController::new(self.carrier, 256, 256);
        let _ = lifecycle.start();

        let cascade_tile_len = TILE_BASEBAND_FRAMES * mult;
        let mut in_channels: Vec<Vec<f64>> = (0..num_channels)
            .map(|_| vec![0.0; TILE_BASEBAND_FRAMES])
            .collect();
        let mut src_out_channels: Vec<Vec<f64>> = (0..num_channels)
            .map(|_| vec![0.0; TILE_BASEBAND_FRAMES * 2])
            .collect();

        let staging_len = (cascade_tile_len * 4 * num_channels.max(2)).max(16384);
        let mut transport_staging = vec![0u8; staging_len];

        // Pre-allocate the high-rate PCM output staging buffer so the streaming
        // loop never allocates per tile.
        let pcm_bytes_per_sample = match pcm_sample_format {
            SampleFormat::S16Le => 2,
            SampleFormat::S24Le | SampleFormat::S24_3Le | SampleFormat::S32Le => {
                if pcm_sample_format == SampleFormat::S24_3Le {
                    3
                } else {
                    4
                }
            }
            SampleFormat::F32 => 4,
            SampleFormat::F64 => 8,
            _ => 4,
        };
        let mut pcm_staging =
            Vec::with_capacity(cascade_tile_len * pcm_bytes_per_sample * num_channels);

        let start_time = Instant::now();
        let mut total_input_frames = 0u64;
        let mut total_output_frames = 0u64;
        let mut overload_events = 0u64;
        let mut consecutive_empty_tiles = 0usize;
        let mut is_idle = false;
        // Require at least ~70-100 ms of continuous zero frames before declaring the stream stopped/idle.
        let idle_threshold_tiles =
            (self.intermediate_rate.get() as usize / (TILE_BASEBAND_FRAMES * 10)).max(20);
        let mut consecutive_empty_polls = 0usize;
        let mut pending_rate_switch: Option<PcmRateHz> = None;

        let base_preamp = self.plan.dsp_config.preamp_db;
        let profile_headroom = self.plan.dsp_config.profile_headroom_db;

        let mut reported_state = lifecycle.state();
        control.set_lifecycle_state(reported_state);

        let mut last_hw_vol: Option<f64> = None;
        let mut last_hw_mute: Option<bool> = None;

        while control.is_running() {
            // Check for dynamic rate changes
            if pending_rate_switch.is_none() {
                if let Some(new_rate) = input.check_rate_change() {
                    if new_rate != self.input_rate {
                        pending_rate_switch = Some(new_rate);
                        lifecycle.request_stop();
                    }
                }
            }

            // Check dynamic volume / mute updates from input driver
            if let Some(vol_upd) = input.check_volume_change() {
                if let Some(pct) = vol_upd.volume_percent {
                    control.set_volume_percent(pct);
                } else if let Some(db) = vol_upd.volume_db {
                    control.set_volume(db);
                }
                if let Some(muted) = vol_upd.is_muted {
                    control.set_muted(muted);
                }
            }

            // Read input frames
            let frames_read = if pending_rate_switch.is_some() && lifecycle.is_draining() {
                for ch in in_channels.iter_mut() {
                    ch.fill(0.0);
                }
                TILE_BASEBAND_FRAMES
            } else {
                let mut slices: ChannelMutSlices<'_> =
                    in_channels.iter_mut().map(|v| v.as_mut_slice()).collect();
                input.read_channels(&mut slices)?
            };

            if frames_read == 0 && pending_rate_switch.is_none() {
                consecutive_empty_polls = consecutive_empty_polls.saturating_add(1);
                // During active streaming, allow a brief grace window (~15 ms, well within the
                // ALSA hardware buffer cushion) for the next PipeWire quantum to arrive before
                // synthesizing and writing idle silence tiles into the middle of the stream.
                if total_input_frames > 0 && !is_idle && consecutive_empty_polls <= 30 {
                    std::thread::sleep(Duration::from_micros(500));
                    continue;
                }

                // When input is temporarily idle or quiet in live streaming:
                // Emit continuous idle / silence frames to keep DAC clock locked and prevent ALSA XRuns.
                let idle_tile_frames = TILE_BASEBAND_FRAMES;
                let idle_bytes = match self.carrier {
                    IdleCarrierFormat::NativeDsd => {
                        let dsd_u32_frames = (idle_tile_frames * mult) / 32;
                        dsd_u32_frames * STEREO_DSD_U32_FRAME_BYTES
                    }
                    IdleCarrierFormat::DoP(ref fmt) => {
                        let dop_frames = (idle_tile_frames * mult) / 16;
                        dop_frames * fmt.bytes_per_stereo_frame()
                    }
                    IdleCarrierFormat::Pcm => idle_tile_frames * mult * 4 * num_channels,
                };

                let to_write = idle_bytes.min(transport_staging.len());
                let written = lifecycle.emit_idle_frames(&mut transport_staging[..to_write]);
                if written > 0 {
                    output
                        .write_all(&transport_staging[..written])
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
                }
                consecutive_empty_tiles = consecutive_empty_tiles.saturating_add(1);
                if consecutive_empty_tiles >= idle_threshold_tiles {
                    is_idle = true;
                }
                let chunk_dur = Duration::from_secs_f64(
                    idle_tile_frames as f64 / self.intermediate_rate.get() as f64,
                );
                std::thread::sleep((chunk_dur / 2).max(Duration::from_millis(1)));
                continue;
            }

            if is_idle {
                is_idle = false;
                let _ = worker_pool.reset();
                let _ = lifecycle.start();
            }
            consecutive_empty_polls = 0;
            consecutive_empty_tiles = 0;
            total_input_frames += frames_read as u64;

            // Compute current dynamic gain based on active volume mode
            let is_muted = control.is_muted();
            let vol_mode = control.get_volume_mode();
            let cur_pct = control.get_volume_percent();

            let (target_db, resolved_pct) = match cur_pct {
                Some(pct) => {
                    let min_db = if vol_mode == DaemonVolumeMode::Hardware {
                        hw_bridge
                            .as_ref()
                            .and_then(|b| b.get_db_range())
                            .map(|(min, _)| min)
                            .unwrap_or(-50.0)
                    } else {
                        audigma_dsp::DEFAULT_SOFTWARE_MIN_DB
                    };
                    let db = audigma_dsp::alsamixer_percentage_to_db(pct, min_db);
                    (db, pct)
                }
                None => {
                    let db = control.get_volume();
                    let min_db = if vol_mode == DaemonVolumeMode::Hardware {
                        hw_bridge
                            .as_ref()
                            .and_then(|b| b.get_db_range())
                            .map(|(min, _)| min)
                            .unwrap_or(-50.0)
                    } else {
                        audigma_dsp::DEFAULT_SOFTWARE_MIN_DB
                    };
                    let pct = audigma_dsp::alsamixer_db_to_percentage(db, min_db);
                    (db, pct)
                }
            };
            control.sync_telemetry(target_db, resolved_pct);

            // Synchronize hardware volume if bridge is present and mode is hardware
            if vol_mode == DaemonVolumeMode::Hardware {
                if let Some(ref mut bridge) = hw_bridge {
                    if last_hw_vol != Some(target_db) {
                        let _ = bridge.set_volume_db(target_db);
                        last_hw_vol = Some(target_db);
                    }
                    if last_hw_mute != Some(is_muted) {
                        let _ = bridge.set_mute(is_muted);
                        last_hw_mute = Some(is_muted);
                    }
                }
            }

            let user_vol = match vol_mode {
                DaemonVolumeMode::Hardware | DaemonVolumeMode::None => 0.0,
                DaemonVolumeMode::Software => target_db,
            };
            let linear_gain =
                if (is_muted && vol_mode != DaemonVolumeMode::Hardware) || target_db <= -119.0 {
                    0.0
                } else {
                    let total_gain_db = base_preamp + profile_headroom + user_vol;
                    10.0_f64.powf(total_gain_db / 20.0)
                };

            // Apply pre-SRC gain and lifecycle cosine ramping
            let mut active_slices: ChannelMutSlices<'_> = in_channels
                .iter_mut()
                .map(|v| &mut v[..frames_read])
                .collect();
            let action = lifecycle.process_input_channels(&mut active_slices);
            drop(active_slices);
            for ch in in_channels.iter_mut() {
                for sample in ch.iter_mut().take(frames_read) {
                    *sample *= linear_gain;
                }
            }

            // Check if muting drain finished
            if action == BlockAction::Drained {
                if let Some(new_rate) = pending_rate_switch {
                    return Ok(StreamExitReason::RateChanged {
                        new_rate,
                        channels: self.plan.stream_spec.channels,
                    });
                }
                if !control.is_running() {
                    return Ok(StreamExitReason::Stopped);
                }
            }

            // Polyphase SRC / ASRC
            let proc_len = if !asrc_resamplers.is_empty() {
                let mut min_prod = usize::MAX;
                for c in 0..num_channels {
                    let in_slice = &in_channels[c][..frames_read];
                    let (_, prod) = asrc_resamplers[c].process(in_slice, &mut src_out_channels[c]);
                    min_prod = min_prod.min(prod);
                }

                // Update drift controller
                let dt = frames_read as f64 / self.input_rate.get() as f64;
                if let Some(ref mut drift_ctrl) = asrc_drift_ctrl {
                    if let Some((fill, cap)) = input.buffer_occupancy() {
                        if let Ok(ppm) = drift_ctrl.update(fill, cap, dt) {
                            for r in asrc_resamplers.iter_mut() {
                                r.set_drift_ppm(ppm);
                            }
                        }
                    }
                }

                min_prod
            } else {
                frames_read
            };

            // Interpolation & Modulation in bounded tiles via ChannelWorkerPool
            let mut offset = 0;
            while offset < proc_len {
                let tile = (proc_len - offset).min(TILE_BASEBAND_FRAMES);
                let req_out = tile * mult;

                let tile_slices: ChannelSlices<'_> = if !asrc_resamplers.is_empty() {
                    src_out_channels
                        .iter()
                        .map(|ch| &ch[offset..offset + tile])
                        .collect()
                } else {
                    in_channels
                        .iter()
                        .map(|ch| &ch[offset..offset + tile])
                        .collect()
                };

                if is_dsd {
                    worker_pool.process_tile_dsd(&tile_slices)?;
                    let out_l = worker_pool.channel_dsd(0);
                    let out_r = worker_pool.channel_dsd(1);

                    let ov = worker_pool.total_overload_count();
                    if ov > overload_events {
                        lifecycle.report_overload_trip();
                        overload_events = ov;
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
                } else {
                    worker_pool.process_tile_pcm(&tile_slices)?;

                    // High-rate PCM output. Quantize to the configured target sample
                    // format. Writing f32 bit patterns into an integer ALSA stream
                    // previously caused extreme clipping.
                    pcm_staging.clear();
                    for i in 0..req_out {
                        for c in 0..num_channels {
                            let out_c = worker_pool.channel_pcm(c);
                            let sample = out_c[i];
                            match pcm_sample_format {
                                SampleFormat::F32 => {
                                    pcm_staging.extend_from_slice(&(sample as f32).to_le_bytes());
                                }
                                SampleFormat::F64 => {
                                    pcm_staging.extend_from_slice(&sample.to_le_bytes());
                                }
                                SampleFormat::S16Le => {
                                    let d = if c < pcm_dithers.len() {
                                        pcm_dithers[c].next_sample()
                                    } else {
                                        0.0
                                    };
                                    let q = (sample * 32767.0 + d).clamp(-32768.0, 32767.0).round()
                                        as i16;
                                    pcm_staging.extend_from_slice(&q.to_le_bytes());
                                }
                                SampleFormat::S24Le => {
                                    let d = if c < pcm_dithers.len() {
                                        pcm_dithers[c].next_sample()
                                    } else {
                                        0.0
                                    };
                                    let q = (sample * 8388607.0 + d)
                                        .clamp(-8388608.0, 8388607.0)
                                        .round() as i32;
                                    pcm_staging.extend_from_slice(&q.to_le_bytes());
                                }
                                SampleFormat::S24_3Le => {
                                    let d = if c < pcm_dithers.len() {
                                        pcm_dithers[c].next_sample()
                                    } else {
                                        0.0
                                    };
                                    let q = (sample * 8388607.0 + d)
                                        .clamp(-8388608.0, 8388607.0)
                                        .round() as i32;
                                    pcm_staging.push((q & 0xFF) as u8);
                                    pcm_staging.push(((q >> 8) & 0xFF) as u8);
                                    pcm_staging.push(((q >> 16) & 0xFF) as u8);
                                }
                                SampleFormat::S32Le => {
                                    let d = if c < pcm_dithers.len() {
                                        pcm_dithers[c].next_sample()
                                    } else {
                                        0.0
                                    };
                                    let q = (sample * 2147483647.0 + d)
                                        .clamp(-2147483648.0, 2147483647.0)
                                        .round() as i32;
                                    pcm_staging.extend_from_slice(&q.to_le_bytes());
                                }
                                _ => {
                                    pcm_staging.extend_from_slice(&(sample as f32).to_le_bytes());
                                }
                            }
                        }
                    }
                    output.write_all(&pcm_staging).map_err(|e| {
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

                offset += tile;
            }

            // Periodic telemetry update
            if total_input_frames % 8192 < frames_read as u64 {
                let state = lifecycle.state();
                if state != reported_state {
                    control.set_lifecycle_state(state);
                    reported_state = state;
                }
                let queue_fill_pct = input
                    .buffer_occupancy()
                    .and_then(|(fill, cap)| (cap > 0).then(|| fill as f64 / cap as f64 * 100.0));
                control.update_runtime_stats(
                    total_input_frames,
                    total_output_frames,
                    overload_events,
                    queue_fill_pct,
                    start_time.elapsed().as_secs(),
                );
            }
        }

        // Clean planned stop
        lifecycle.request_stop();
        control.set_state(DaemonState::DrainingMuting);

        // Emit silence/tail frames to guarantee zero clicks
        let tail_bytes = 1024
            * match self.carrier {
                IdleCarrierFormat::NativeDsd => STEREO_DSD_U32_FRAME_BYTES,
                IdleCarrierFormat::DoP(fmt) => fmt.bytes_per_stereo_frame(),
                IdleCarrierFormat::Pcm => 4 * num_channels,
            };
        let mut tail_buf = vec![0u8; tail_bytes];
        lifecycle.emit_idle_frames(&mut tail_buf);
        let _ = output.write_all(&tail_buf);
        let _ = output.flush();

        control.set_state(DaemonState::Stopped);

        Ok(StreamExitReason::Stopped)
    }
}
