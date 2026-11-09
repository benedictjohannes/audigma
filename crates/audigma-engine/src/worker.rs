//! Persistent real-time multi-channel worker pool (`Design.md §1.2`).
//!
//! Provides:
//! - Dedicated persistent worker threads (1 thread per channel) spawned upon pipeline preparation.
//! - Lock-free synchronization barrier using cache-line aligned atomic flags and Linux futex.
//! - POSIX real-time scheduling priority (`SCHED_FIFO` via `pthread_setschedparam` / `RLIMIT_RTPRIO`)
//!   and thread-local hardware FTZ/DAZ denormal guards.
//! - Zero heap allocation and zero lock contention during steady-state processing.
//! - Isolated per-channel FIR cascade and CRFB modulator states.
//! - Bounded internal DSD subtiles without additional worker synchronization.
//! - Optional worker-local input decimation, gain, and SRC in the same dispatch.

mod input;

pub use input::{InputDspConfig, InputResampling, InputTileControls};

use crate::cpu::ThreadPlacement;
use crate::lifecycle::FrameGain;
use crate::timing::PoolTimings;
use crate::traits::EngineError;
use audigma_dsp::DcBlocker;
use audigma_dsp::config::{Apodization, DitherType, FilterPhase, OverloadPolicy};
use audigma_dsp::fir::HalfBandCascade;
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::modulator::Modulator;
use audigma_dsp::rates::DsdBitRateHz;
use input::{InputBounds, InputProcessor};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

const CMD_IDLE: u32 = 0;
const CMD_PCM: u32 = 1;
const CMD_DSD: u32 = 2;
const CMD_RESET: u32 = 3;
const CMD_STOP: u32 = 4;
const CMD_INPUT_PCM: u32 = 5;
const CMD_INPUT_DSD: u32 = 6;
const CMD_RESET_OUTPUT: u32 = 7;
const CMD_RECONFIGURE: u32 = 8;
/// Input PCM through the input stage and interpolation cascade only (no modulator):
/// two-pass peak analysis measures the cascade output before modulation.
const CMD_INPUT_PCM_INTERP: u32 = 9;

// At DSD512 this is 32 baseband frames; at DSD256 it is 64. Keep the
// externally scheduled tile intact and interpolate/modulate each slice while
// it is still hot, rather than rereading the whole high-rate PCM tile later.
const DSD_SUBTILE_SAMPLES: usize = 16_384;

/// DC blocker ahead of the cascade (DSD targets only, `Design.md §4.1`) with its subtile scratch.
struct DcStage {
    blocker: DcBlocker,
    scratch: Vec<f64>,
}

impl DcStage {
    /// Returns `tile` with DC removed (in the scratch), or `tile` itself without a blocker.
    #[inline]
    fn filter<'a>(dc: &'a mut Option<&mut DcStage>, tile: &'a [f64]) -> &'a [f64] {
        match dc {
            Some(stage) => {
                let out = &mut stage.scratch[..tile.len()];
                stage.blocker.process(tile, out);
                out
            }
            None => tile,
        }
    }
}

fn process_pcm_tile(
    cascade: &mut HalfBandCascade,
    mut dc: Option<&mut DcStage>,
    input: &[f64],
    output: &mut [f64],
    subtile_frames: usize,
) -> Result<(), EngineError> {
    let subtile_samples = subtile_frames * cascade.rate_multiplier();
    for (tile, out) in input
        .chunks(subtile_frames)
        .zip(output.chunks_mut(subtile_samples))
    {
        cascade.process_tile(DcStage::filter(&mut dc, tile), out)?;
    }
    Ok(())
}

fn process_dsd_tile(
    cascade: &mut HalfBandCascade,
    modulator: &mut Modulator,
    mut dc: Option<&mut DcStage>,
    input: &[f64],
    pcm: &mut [f64],
    dsd: &mut [u8],
    subtile_frames: usize,
) -> Result<(), EngineError> {
    // Preserve the original validation/error ordering: the FIR advances through
    // the entire tile even if the modulator rejects an unaligned input length.
    if pcm.len() % 8 != 0 {
        process_pcm_tile(cascade, dc, input, pcm, subtile_frames)?;
        return Ok(modulator.process_tile_bytes(pcm, dsd)?);
    }
    if input.is_empty() {
        return Ok(());
    }

    let subtile_samples = subtile_frames * cascade.rate_multiplier();
    let mut result = Ok(());
    for ((tile, out_pcm), out_dsd) in input
        .chunks(subtile_frames)
        .zip(pcm.chunks_mut(subtile_samples))
        .zip(dsd.chunks_mut(subtile_samples / 8))
    {
        cascade.process_tile(DcStage::filter(&mut dc, tile), out_pcm)?;
        // On OverloadPolicy::Error, finish the FIR but stop advancing the
        // modulator. Both states and the PCM view then match whole-tile execution.
        if result.is_ok() {
            result = modulator.process_tile_bytes(out_pcm, out_dsd);
        }
    }
    Ok(result?)
}

/// Target DSD configuration for channel workers.
#[derive(Debug, Clone)]
pub struct TargetDsdConfig {
    pub bit_rate: DsdBitRateHz,
    pub modulator_order: u8,
    pub dither: DitherType,
    pub overload_policy: OverloadPolicy,
    pub seed_base: u64,
}

#[cfg(target_os = "linux")]
const FUTEX_WAIT_PRIVATE: libc::c_int = 128; // FUTEX_WAIT (0) | FUTEX_PRIVATE_FLAG (128)
#[cfg(target_os = "linux")]
const FUTEX_WAKE_PRIVATE: libc::c_int = 129; // FUTEX_WAKE (1) | FUTEX_PRIVATE_FLAG (128)

#[inline]
fn futex_wait(atomic: &AtomicU32, expected: u32) {
    #[cfg(target_os = "linux")]
    unsafe {
        let ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 20_000_000, // 20 ms fail-safe timeout
        };
        libc::syscall(
            libc::SYS_futex,
            atomic.as_ptr(),
            FUTEX_WAIT_PRIVATE,
            expected,
            &ts as *const libc::timespec,
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (atomic, expected);
        std::thread::yield_now();
    }
}

#[inline]
fn futex_wake(atomic: &AtomicU32) {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::syscall(libc::SYS_futex, atomic.as_ptr(), FUTEX_WAKE_PRIVATE, 1);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = atomic;
    }
}

// Remove OS-specific thread RT / TID implementations from audigma-engine

/// Dedicated 64-byte aligned synchronization and telemetry slot for one channel worker.
#[repr(align(64))]
struct WorkerSync {
    target_generation: AtomicU32,
    completed_generation: AtomicU32,
    command: AtomicU32,
    input_len: AtomicUsize,
    output_len: AtomicUsize,
    has_error: AtomicBool,
}

impl WorkerSync {
    fn new() -> Self {
        Self {
            target_generation: AtomicU32::new(0),
            completed_generation: AtomicU32::new(0),
            command: AtomicU32::new(CMD_IDLE),
            input_len: AtomicUsize::new(0),
            output_len: AtomicUsize::new(0),
            has_error: AtomicBool::new(false),
        }
    }
}

/// Pre-allocated, zero-copy shared buffers for one channel worker.
struct WorkerBuffers {
    in_buf: UnsafeCell<Vec<f64>>,
    dsd_in_buf: UnsafeCell<Vec<u8>>,
    pcm_out_buf: UnsafeCell<Vec<f64>>,
    dsd_out_buf: UnsafeCell<Vec<u8>>,
    /// Modulator overload count of this channel (written by its worker).
    overload_count: AtomicU64,
}

unsafe impl Sync for WorkerBuffers {}

/// One immutable control block shared by all channel workers. Publication and
/// reclamation use the same generation barrier as the per-channel audio buffers.
struct InputControlBuffer {
    frame_gains: UnsafeCell<Vec<FrameGain>>,
    frames: AtomicUsize,
    linear_gain: AtomicU64,
    drift_ppm: AtomicU64,
}

// SAFETY: The coordinator writes only while every worker is stopped at the
// completion barrier. Workers read after acquiring their target generation and
// release all references before publishing their completed generation.
unsafe impl Sync for InputControlBuffer {}

impl InputControlBuffer {
    fn new(frames: usize) -> Self {
        Self {
            frame_gains: UnsafeCell::new(vec![FrameGain::Preserve; frames]),
            frames: AtomicUsize::new(0),
            linear_gain: AtomicU64::new(1.0f64.to_bits()),
            drift_ppm: AtomicU64::new(0.0f64.to_bits()),
        }
    }

    /// SAFETY: Every worker must have completed the preceding dispatch.
    unsafe fn publish(&self, controls: InputTileControls<'_>) {
        let gains = unsafe { &mut *self.frame_gains.get() };
        gains[..controls.frame_gains.len()].copy_from_slice(controls.frame_gains);
        self.frames
            .store(controls.frame_gains.len(), Ordering::Relaxed);
        self.linear_gain
            .store(controls.linear_gain.to_bits(), Ordering::Relaxed);
        self.drift_ppm
            .store(controls.drift_ppm.to_bits(), Ordering::Relaxed);
    }

    /// SAFETY: Called only by a worker between its acquire/release barriers.
    unsafe fn controls(&self) -> InputTileControls<'_> {
        let gains = unsafe { &*self.frame_gains.get() };
        InputTileControls {
            frame_gains: &gains[..self.frames.load(Ordering::Relaxed)],
            linear_gain: f64::from_bits(self.linear_gain.load(Ordering::Relaxed)),
            drift_ppm: f64::from_bits(self.drift_ppm.load(Ordering::Relaxed)),
        }
    }
}

/// One worker thread. It owns the states of the channels assigned to it
/// (`channel % workers`, in channel order).
struct WorkerSlot {
    sync: Arc<WorkerSync>,
    error: Arc<Mutex<Option<EngineError>>>,
    /// Staging area for `CMD_RECONFIGURE`: the coordinator parks the new states here,
    /// the worker swaps them with its own and leaves the retired states behind.
    mailbox: Arc<Mutex<Option<Vec<ChannelState>>>>,
}

/// Everything one channel worker owns that depends on the active rate and filter.
struct ChannelState {
    cascade: HalfBandCascade,
    /// Present exactly when `modulator` is (DSD targets).
    dc: Option<DcStage>,
    modulator: Option<Modulator>,
    input_processor: Option<InputProcessor>,
    buffers: Arc<WorkerBuffers>,
    input_controls: Option<Arc<InputControlBuffer>>,
    mult: usize,
    subtile_frames: usize,
}

impl ChannelState {
    /// Executes one dispatched command; returns the result and produced frame count.
    fn run(&mut self, cmd: u32, input_len: usize) -> (Result<(), EngineError>, usize) {
        let mut output_frames = 0;
        let mult = self.mult;
        let subtile_frames = self.subtile_frames;
        let result = match cmd {
            CMD_RESET | CMD_RESET_OUTPUT => {
                self.cascade.reset();
                if let Some(ref mut dc) = self.dc {
                    dc.blocker.reset();
                }
                if let Some(ref mut m) = self.modulator {
                    m.reset();
                }
                if cmd == CMD_RESET {
                    if let Some(ref mut processor) = self.input_processor {
                        processor.reset();
                    }
                }
                Ok(())
            }
            CMD_PCM | CMD_DSD => {
                let in_len = input_len;
                output_frames = in_len;
                let in_buf = unsafe { &*self.buffers.in_buf.get() };
                let in_slice = &in_buf[..in_len];
                let pcm_buf = unsafe { &mut *self.buffers.pcm_out_buf.get() };
                let pcm_out = &mut pcm_buf[..in_len * mult];
                if let Some(m) = self.modulator.as_mut().filter(|_| cmd == CMD_DSD) {
                    let req_bytes = pcm_out.len() / 8;
                    let dsd_buf = unsafe { &mut *self.buffers.dsd_out_buf.get() };
                    let dsd_out = &mut dsd_buf[..req_bytes];
                    let result = process_dsd_tile(
                        &mut self.cascade,
                        m,
                        self.dc.as_mut(),
                        in_slice,
                        pcm_out,
                        dsd_out,
                        subtile_frames,
                    );
                    self.buffers
                        .overload_count
                        .store(m.overload_count(), Ordering::Relaxed);
                    result
                } else {
                    process_pcm_tile(
                        &mut self.cascade,
                        self.dc.as_mut(),
                        in_slice,
                        pcm_out,
                        subtile_frames,
                    )
                }
            }
            CMD_INPUT_PCM | CMD_INPUT_DSD | CMD_INPUT_PCM_INTERP => (|| {
                let processor =
                    self.input_processor
                        .as_mut()
                        .ok_or_else(|| EngineError::InvalidTopology {
                            message: "channel worker has no input DSP configuration".into(),
                        })?;
                let control_buffer = self.input_controls.as_ref().unwrap();
                let controls = unsafe { control_buffer.controls() };
                let in_len = input_len;
                let prepared = if cmd == CMD_INPUT_DSD {
                    let input = unsafe { &*self.buffers.dsd_in_buf.get() };
                    processor.process_dsd(&input[..in_len], controls)?
                } else {
                    let input = unsafe { &*self.buffers.in_buf.get() };
                    processor.process_pcm(&input[..in_len], controls)?
                };
                let frames = prepared.len();
                output_frames = frames;
                let pcm_buf = unsafe { &mut *self.buffers.pcm_out_buf.get() };
                let pcm_out = &mut pcm_buf[..frames * mult];
                if let Some(m) = self
                    .modulator
                    .as_mut()
                    .filter(|_| cmd != CMD_INPUT_PCM_INTERP)
                {
                    let dsd_buf = unsafe { &mut *self.buffers.dsd_out_buf.get() };
                    let dsd_bytes = pcm_out.len() / 8;
                    let result = process_dsd_tile(
                        &mut self.cascade,
                        m,
                        self.dc.as_mut(),
                        prepared,
                        pcm_out,
                        &mut dsd_buf[..dsd_bytes],
                        subtile_frames,
                    );
                    self.buffers
                        .overload_count
                        .store(m.overload_count(), Ordering::Relaxed);
                    result
                } else {
                    process_pcm_tile(
                        &mut self.cascade,
                        self.dc.as_mut(),
                        prepared,
                        pcm_out,
                        subtile_frames,
                    )
                }
            })(),
            _ => Ok(()),
        };
        (result, output_frames)
    }
}

/// Splits per-channel states into per-worker groups: channel `c` goes to worker `c % workers`.
fn distribute_states(states: Vec<ChannelState>, workers: usize) -> Vec<Vec<ChannelState>> {
    let mut groups: Vec<Vec<ChannelState>> = (0..workers).map(|_| Vec::new()).collect();
    for (ch, state) in states.into_iter().enumerate() {
        groups[ch % workers].push(state);
    }
    groups
}

/// Runs one command over every channel a worker owns. Returns the first error and the
/// produced frame count (all of a worker's channels must agree).
fn run_channels(
    states: &mut [ChannelState],
    cmd: u32,
    input_len: usize,
) -> (Result<(), EngineError>, usize) {
    let mut result = Ok(());
    let mut frames = None;
    for state in states.iter_mut() {
        let (r, f) = state.run(cmd, input_len);
        if let Err(e) = r
            && result.is_ok()
        {
            result = Err(e);
        }
        match frames {
            None => frames = Some(f),
            Some(prev) if prev != f && result.is_ok() => {
                result = Err(EngineError::Pipeline(
                    "channels of one worker produced different frame counts".into(),
                ));
            }
            _ => {}
        }
    }
    (result, frames.unwrap_or(0))
}

/// Page granularity used for pre-touching (the smallest common page size).
const PRETOUCH_STRIDE_BYTES: usize = 4096;

/// Writes one element per page so first-touch page faults happen at construction, not in
/// the first tiles. Leaves the contents unchanged (the buffers are zero-filled).
fn pretouch<T: Copy + Default>(buf: &mut [T]) {
    let elem = std::mem::size_of::<T>().max(1);
    let step = (PRETOUCH_STRIDE_BYTES / elem).max(1);
    let mut i = 0;
    while i < buf.len() {
        // SAFETY: `i < buf.len()`.
        unsafe { std::ptr::write_volatile(buf.as_mut_ptr().add(i), T::default()) };
        i += step;
    }
    if let Some(last) = buf.last_mut() {
        // SAFETY: valid exclusive reference.
        unsafe { std::ptr::write_volatile(last, T::default()) };
    }
}

/// Pool-level facts derived while building the per-channel states.
struct PoolShape {
    input_bounds: Option<InputBounds>,
    input_controls: Option<Arc<InputControlBuffer>>,
    mult: usize,
    group_delay_output_samples: usize,
}

#[allow(clippy::too_many_arguments)]
fn build_channel_states(
    channels: usize,
    phase: FilterPhase,
    apodization: Apodization,
    stages: usize,
    tile_baseband_frames: usize,
    target_dsd: Option<&TargetDsdConfig>,
    input_config: Option<InputDspConfig>,
) -> Result<(Vec<ChannelState>, PoolShape), EngineError> {
    if channels == 0 {
        return Err(EngineError::InvalidTopology {
            message: "ChannelWorkerPool requires at least 1 channel".into(),
        });
    }
    if tile_baseband_frames == 0 {
        return Err(EngineError::InvalidTopology {
            message: "ChannelWorkerPool requires a nonzero tile capacity".into(),
        });
    }

    let input_bounds = input_config
        .map(|config| config.bounds(tile_baseband_frames))
        .transpose()?;
    let input_frames = input_bounds.map_or(tile_baseband_frames, |bounds| bounds.pcm_frames);
    let input_dsd_bytes = input_bounds.map_or(0, |bounds| bounds.dsd_bytes);
    // Output-only commands remain valid on input-configured pools, even
    // when SRC downsampling produces fewer frames than the outer tile.
    let output_frames = input_bounds.map_or(tile_baseband_frames, |bounds| {
        bounds.output_frames.max(tile_baseband_frames)
    });
    let input_controls =
        input_config.map(|_| Arc::new(InputControlBuffer::new(tile_baseband_frames)));

    let mult = 1usize << stages;
    let subtile_frames = if target_dsd.is_some() {
        tile_baseband_frames.min((DSD_SUBTILE_SAMPLES / mult).max(1))
    } else {
        tile_baseband_frames
    };
    let cascade_probe = HalfBandCascade::new(phase, apodization, stages, subtile_frames);
    let mult = cascade_probe.rate_multiplier();
    let group_delay_output_samples = cascade_probe.total_group_delay_output_samples();
    let cascade_tile_len =
        output_frames
            .checked_mul(mult)
            .ok_or_else(|| EngineError::InvalidTopology {
                message: "worker output tile capacity overflow".into(),
            })?;
    let dsd_tile_bytes = cascade_tile_len / 8;

    let mut states = Vec::with_capacity(channels);
    for ch in 0..channels {
        let mut in_buf = vec![0.0f64; input_frames];
        let mut dsd_in_buf = vec![0u8; input_dsd_bytes];
        let mut pcm_out_buf = vec![0.0f64; cascade_tile_len];
        let mut dsd_out_buf = vec![0u8; dsd_tile_bytes.max(1)];
        // Zeroed allocations are lazily mapped; fault the pages in now, off the hot path.
        pretouch(&mut in_buf);
        pretouch(&mut dsd_in_buf);
        pretouch(&mut pcm_out_buf);
        pretouch(&mut dsd_out_buf);
        let buffers = Arc::new(WorkerBuffers {
            in_buf: UnsafeCell::new(in_buf),
            dsd_in_buf: UnsafeCell::new(dsd_in_buf),
            pcm_out_buf: UnsafeCell::new(pcm_out_buf),
            dsd_out_buf: UnsafeCell::new(dsd_out_buf),
            overload_count: AtomicU64::new(0),
        });
        let cascade = HalfBandCascade::new(phase, apodization, stages, subtile_frames);
        let modulator = if let Some(dsd_cfg) = target_dsd {
            let mut m = Modulator::new(
                dsd_cfg.modulator_order,
                dsd_cfg.bit_rate,
                dsd_cfg.dither,
                dsd_cfg.overload_policy,
            )?;
            m.set_dither_seed(dsd_cfg.seed_base.wrapping_add(ch as u64));
            Some(m)
        } else {
            None
        };
        let input_processor = input_config
            .zip(input_bounds)
            .map(|(config, bounds)| InputProcessor::new(tile_baseband_frames, config, bounds))
            .transpose()?;
        // The cascade runs at f_bit / mult, so that is the blocker's baseband rate.
        let dc = target_dsd.map(|dsd_cfg| DcStage {
            blocker: DcBlocker::production(dsd_cfg.bit_rate.get() as f64 / mult as f64),
            scratch: vec![0.0; subtile_frames],
        });
        states.push(ChannelState {
            cascade,
            dc,
            modulator,
            input_processor,
            buffers,
            input_controls: input_controls.clone(),
            mult,
            subtile_frames,
        });
    }

    Ok((
        states,
        PoolShape {
            input_bounds,
            input_controls,
            mult,
            group_delay_output_samples,
        },
    ))
}

/// Spin iterations before a waiting thread falls back to `futex_wait`.
const WORKER_SPIN_ITERATIONS: u32 = 2048;

/// Per-worker OS setup callback: `(worker index, placement)`. Runs on the worker thread after
/// `ScopedFtzDaz`, before the worker loop. The engine performs no syscalls itself.
pub type ThreadInitHook = Arc<dyn Fn(usize, &ThreadPlacement) -> Result<(), String> + Send + Sync>;

/// Placement and OS-setup hook for a pool's worker threads.
///
/// `workers` is the number of worker threads (`None` = one per channel, capped at the channel
/// count). Channel `c` runs on worker `c % workers`. `Some(0)` selects inline mode: no threads,
/// no hand-off; every command runs sequentially on the calling thread (which must already have
/// its own placement applied; hook and `placements` are ignored).
/// `placements[w]` describes worker `w` (missing entries use `ThreadPlacement::default()`:
/// unpinned, normal priority, spinning allowed). The worker reads its own `spin` flag; the
/// coordinator only spins at the barrier when every worker may spin.
#[derive(Clone, Default)]
pub struct WorkerSetup {
    pub workers: Option<usize>,
    pub placements: Vec<ThreadPlacement>,
    pub hook: Option<ThreadInitHook>,
    /// Startup handshake: when true, `new_with_input` waits for every worker's hook result and
    /// fails (stopping the workers) if any hook returned an error. When false, hook errors are
    /// only logged by the hook itself.
    pub require_hook_ok: bool,
}

/// Persistent real-time worker pool for multi-channel audio DSP.
pub struct ChannelWorkerPool {
    /// One per worker thread (empty in inline mode).
    slots: Vec<WorkerSlot>,
    /// Inline mode: per-channel states run on the calling thread (empty when workers exist).
    inline_states: Vec<ChannelState>,
    inline: bool,
    /// One per channel.
    channel_buffers: Vec<Arc<WorkerBuffers>>,
    thread_handles: Vec<JoinHandle<()>>,
    failed_worker: Option<usize>,
    current_generation: u32,
    last_frames: usize,
    tile_baseband_frames: usize,
    mult: usize,
    group_delay_output_samples: usize,
    input_config: Option<InputDspConfig>,
    input_bounds: Option<InputBounds>,
    input_controls: Option<Arc<InputControlBuffer>>,
    pcm_residue_frames: usize,
    dsd_residue_bytes: usize,
    coordinator_spin: bool,
    timings: Arc<PoolTimings>,
}

impl ChannelWorkerPool {
    /// Spawns $N$ persistent channel worker threads upon pipeline preparation.
    // The constructor mirrors the prepared pipeline configuration explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: usize,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        tile_baseband_frames: usize,
        target_dsd: Option<TargetDsdConfig>,
        worker_setup: Option<WorkerSetup>,
    ) -> Result<Self, EngineError> {
        Self::new_with_input(
            channels,
            phase,
            apodization,
            stages,
            tile_baseband_frames,
            target_dsd,
            None,
            worker_setup,
        )
    }

    /// Prepares the entire channel DSP chain, without adding another worker pool
    /// or an intermediate decimation/SRC barrier. `None` retains the original
    /// interpolation/modulation-only worker API and buffer capacities.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_input(
        channels: usize,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        tile_baseband_frames: usize,
        target_dsd: Option<TargetDsdConfig>,
        input_config: Option<InputDspConfig>,
        worker_setup: Option<WorkerSetup>,
    ) -> Result<Self, EngineError> {
        let (states, shape) = build_channel_states(
            channels,
            phase,
            apodization,
            stages,
            tile_baseband_frames,
            target_dsd.as_ref(),
            input_config,
        )?;

        let setup = worker_setup.unwrap_or_default();
        let inline = setup.workers == Some(0);
        let worker_count = if inline {
            0
        } else {
            setup.workers.unwrap_or(channels).clamp(1, channels)
        };
        let mut inline_states = Vec::new();
        let channel_buffers: Vec<Arc<WorkerBuffers>> =
            states.iter().map(|s| Arc::clone(&s.buffers)).collect();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<(usize, Result<(), String>)>();
        let coordinator_spin =
            (0..worker_count).all(|w| setup.placements.get(w).is_none_or(|p| p.spin));
        let timings = PoolTimings::new(worker_count.max(1));
        let mut slots = Vec::with_capacity(worker_count);
        let mut thread_handles = Vec::with_capacity(worker_count);

        let groups = if inline {
            inline_states = states;
            Vec::new()
        } else {
            distribute_states(states, worker_count)
        };
        for (w, mut states) in groups.into_iter().enumerate() {
            let sync = Arc::new(WorkerSync::new());
            let error = Arc::new(Mutex::new(None));
            let mailbox: Arc<Mutex<Option<Vec<ChannelState>>>> = Arc::new(Mutex::new(None));
            slots.push(WorkerSlot {
                sync: Arc::clone(&sync),
                error: Arc::clone(&error),
                mailbox: Arc::clone(&mailbox),
            });

            let thread_hook = setup.hook.clone();
            let ready = ready_tx.clone();
            let worker_timings = Arc::clone(&timings);
            let placement = setup
                .placements
                .get(w)
                .cloned()
                .unwrap_or_else(ThreadPlacement::default);
            let spin_limit: u32 = if placement.spin {
                WORKER_SPIN_ITERATIONS
            } else {
                0
            };
            let handle = std::thread::Builder::new()
                .name(format!("audigma-dsp-{w}"))
                .spawn(move || {
                    let _ftz = ScopedFtzDaz::new();

                    let hook_result = match thread_hook {
                        Some(ref hook) => hook(w, &placement),
                        None => Ok(()),
                    };
                    if let Err(ref e) = hook_result {
                        tracing::debug!(worker = w, error = %e, "worker placement hook failed");
                    }
                    // Report and release the sender so the constructor never waits on a
                    // worker that already finished its setup (or died before it).
                    let _ = ready.send((w, hook_result));
                    drop(ready);

                    let mut current_gen = 0u32;
                    loop {
                        let mut spins = 0;
                        while sync.target_generation.load(Ordering::Acquire) == current_gen
                            && spins < spin_limit
                        {
                            core::hint::spin_loop();
                            spins += 1;
                        }
                        while sync.target_generation.load(Ordering::Acquire) == current_gen {
                            futex_wait(&sync.target_generation, current_gen);
                        }

                        current_gen = sync.target_generation.load(Ordering::Acquire);
                        let cmd = sync.command.load(Ordering::Acquire);

                        let (result, output_frames) = match cmd {
                            CMD_STOP => break,
                            CMD_RECONFIGURE => {
                                // Swap in the coordinator-built states and hand the old
                                // ones back through the same Vec: no allocation here, and
                                // the coordinator frees them off the real-time thread.
                                let mut slot = mailbox.lock().unwrap();
                                let result = match slot.as_mut() {
                                    Some(next) => {
                                        std::mem::swap(&mut states, next);
                                        Ok(())
                                    }
                                    None => Err(EngineError::Pipeline(
                                        "worker reconfigure without staged state".into(),
                                    )),
                                };
                                (result, 0)
                            }
                            CMD_PCM | CMD_DSD | CMD_INPUT_PCM | CMD_INPUT_DSD
                            | CMD_INPUT_PCM_INTERP => {
                                // Execution time only: the wake-up is already behind us.
                                let t0 = Instant::now();
                                let out = run_channels(
                                    &mut states,
                                    cmd,
                                    sync.input_len.load(Ordering::Relaxed),
                                );
                                worker_timings.record_worker(w, t0.elapsed().as_nanos() as u64);
                                out
                            }
                            _ => run_channels(
                                &mut states,
                                cmd,
                                sync.input_len.load(Ordering::Relaxed),
                            ),
                        };
                        sync.output_len.store(output_frames, Ordering::Relaxed);
                        if let Err(e) = result {
                            sync.has_error.store(true, Ordering::Release);
                            let mut err = error.lock().unwrap();
                            *err = Some(e);
                        }

                        sync.completed_generation
                            .store(current_gen, Ordering::Release);
                        futex_wake(&sync.completed_generation);
                    }
                })
                .map_err(|e| EngineError::WorkerThread {
                    channel: w,
                    reason: format!("failed to spawn worker thread {w}: {e}"),
                })?;

            thread_handles.push(handle);
        }

        drop(ready_tx);

        let pool = Self {
            slots,
            inline_states,
            inline,
            channel_buffers,
            thread_handles,
            failed_worker: None,
            current_generation: 0,
            last_frames: 0,
            tile_baseband_frames,
            mult: shape.mult,
            group_delay_output_samples: shape.group_delay_output_samples,
            input_config,
            input_bounds: shape.input_bounds,
            input_controls: shape.input_controls,
            pcm_residue_frames: 0,
            dsd_residue_bytes: 0,
            coordinator_spin,
            timings,
        };

        // Startup handshake: collect every worker's setup result. A worker that died before
        // reporting simply disconnects its sender; that surfaces on first dispatch instead.
        let mut first_failure = None;
        while let Ok((ch, result)) = ready_rx.recv() {
            if let Err(reason) = result
                && first_failure.is_none()
            {
                first_failure = Some((ch, reason));
            }
        }
        if setup.require_hook_ok
            && let Some((channel, reason)) = first_failure
        {
            // Dropping `pool` stops and joins the workers.
            return Err(EngineError::WorkerThread {
                channel,
                reason: format!("worker {channel} setup failed: {reason}"),
            });
        }
        Ok(pool)
    }

    /// Rebuilds every channel's per-rate DSP state (cascade, modulator, input
    /// processor, buffers) and swaps it into the running workers, keeping the
    /// threads (and their placement) alive. All allocation happens on the
    /// calling thread before the swap; the result is equivalent to a freshly
    /// constructed pool (filter history, residues and overload counts reset).
    /// The channel count is fixed for the pool's lifetime.
    #[allow(clippy::too_many_arguments)]
    pub fn reconfigure(
        &mut self,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        tile_baseband_frames: usize,
        target_dsd: Option<TargetDsdConfig>,
        input_config: Option<InputDspConfig>,
    ) -> Result<(), EngineError> {
        self.ensure_ready()?;
        let (mut states, shape) = build_channel_states(
            self.channel_buffers.len(),
            phase,
            apodization,
            stages,
            tile_baseband_frames,
            target_dsd.as_ref(),
            input_config,
        )?;
        let new_buffers: Vec<Arc<WorkerBuffers>> =
            states.iter().map(|s| Arc::clone(&s.buffers)).collect();
        if self.inline {
            // Nothing runs concurrently: swap directly and free the old states here.
            std::mem::swap(&mut self.inline_states, &mut states);
            drop(states);
        } else {
            for (slot, group) in self
                .slots
                .iter()
                .zip(distribute_states(states, self.slots.len()))
            {
                *slot.mailbox.lock().unwrap() = Some(group);
            }

            let result = self.dispatch_command(CMD_RECONFIGURE, 0);
            // Workers are idle at the barrier: reclaim the previous state here.
            let retired: Vec<_> = self
                .slots
                .iter()
                .map(|slot| slot.mailbox.lock().unwrap().take())
                .collect();
            result?;
            drop(retired);
        }

        self.channel_buffers = new_buffers;
        self.tile_baseband_frames = tile_baseband_frames;
        self.mult = shape.mult;
        self.group_delay_output_samples = shape.group_delay_output_samples;
        self.input_config = input_config;
        self.input_bounds = shape.input_bounds;
        self.input_controls = shape.input_controls;
        self.pcm_residue_frames = 0;
        self.dsd_residue_bytes = 0;
        self.last_frames = 0;
        Ok(())
    }

    /// Shared timing histograms (persist across `reconfigure`).
    pub fn timings(&self) -> Arc<PoolTimings> {
        Arc::clone(&self.timings)
    }

    /// Number of audio channels.
    #[inline]
    pub fn channels(&self) -> usize {
        self.channel_buffers.len()
    }

    /// Number of worker threads (at most `channels()`; 0 in inline mode).
    #[inline]
    pub fn worker_count(&self) -> usize {
        self.slots.len()
    }

    /// Maximum PCM frames per channel accepted by an input-DSP dispatch.
    pub fn input_frame_capacity(&self) -> usize {
        self.input_bounds
            .map_or(self.tile_baseband_frames, |bounds| bounds.pcm_frames)
    }

    /// Maximum raw DSD bytes per channel accepted by an input-DSP dispatch.
    pub fn input_dsd_byte_capacity(&self) -> usize {
        self.input_bounds.map_or(0, |bounds| bounds.dsd_bytes)
    }

    /// Maximum post-SRC baseband frames produced by a single dispatch.
    pub fn output_frame_capacity(&self) -> usize {
        self.input_bounds
            .map_or(self.tile_baseband_frames, |bounds| bounds.output_frames)
    }

    /// Predicts the PCM-decimated length using a channel-independent residue
    /// count. Worker FIR history and audio data remain private to each channel.
    pub fn pcm_baseband_frames(&self, frames: usize) -> Result<usize, EngineError> {
        let bounds = self
            .input_bounds
            .ok_or_else(|| EngineError::InvalidTopology {
                message: "worker pool has no input DSP configuration".into(),
            })?;
        if frames > bounds.pcm_frames {
            return Err(EngineError::InvalidTopology {
                message: "PCM input exceeds worker capacity".into(),
            });
        }
        Ok((self.pcm_residue_frames + frames) / bounds.pcm_divider)
    }

    /// Predicts the DSD-decimated length without unpacking or filtering input.
    pub fn dsd_baseband_frames(&self, bytes: usize) -> Result<usize, EngineError> {
        self.pcm_baseband_frames(self.dsd_decoded_frames(bytes)?)
    }

    /// PCM frames produced by raw DSD decoding, before optional PCM downsampling.
    pub fn dsd_decoded_frames(&self, bytes: usize) -> Result<usize, EngineError> {
        let bounds = self
            .input_bounds
            .filter(|bounds| bounds.dsd_bytes > 0)
            .ok_or_else(|| EngineError::InvalidTopology {
                message: "worker pool has no raw DSD input configuration".into(),
            })?;
        if bytes > bounds.dsd_bytes {
            return Err(EngineError::InvalidTopology {
                message: "DSD input exceeds worker capacity".into(),
            });
        }
        let alignment = bounds.dsd_divider.div_ceil(8);
        let aligned = (self.dsd_residue_bytes + bytes) / alignment * alignment;
        Ok(aligned * 8 / bounds.dsd_divider)
    }

    /// Rate multiplier from baseband input to output ($2^K$).
    #[inline]
    pub fn rate_multiplier(&self) -> usize {
        self.mult
    }

    /// Total filter group delay in output samples.
    #[inline]
    pub fn total_group_delay_output_samples(&self) -> usize {
        self.group_delay_output_samples
    }

    /// Cumulative overload count across all channels.
    #[inline]
    pub fn total_overload_count(&self) -> u64 {
        self.channel_buffers
            .iter()
            .map(|b| b.overload_count.load(Ordering::Relaxed))
            .sum()
    }

    /// Overload count for a specific channel.
    #[inline]
    pub fn channel_overload_count(&self, channel: usize) -> u64 {
        self.channel_buffers[channel]
            .overload_count
            .load(Ordering::Relaxed)
    }

    fn ensure_ready(&self) -> Result<(), EngineError> {
        if let Some(channel) = self.failed_worker {
            return Err(EngineError::WorkerThread {
                channel,
                reason: format!("worker thread {channel} terminated unexpectedly"),
            });
        }
        Ok(())
    }

    fn dispatch_command(&mut self, cmd: u32, frames: usize) -> Result<(), EngineError> {
        self.ensure_ready()?;
        let timed = matches!(
            cmd,
            CMD_PCM | CMD_DSD | CMD_INPUT_PCM | CMD_INPUT_DSD | CMD_INPUT_PCM_INTERP
        );
        if self.inline {
            // The guard restores the caller's FP environment on return.
            let _ftz = ScopedFtzDaz::new();
            let t0 = timed.then(Instant::now);
            let (result, output_frames) = run_channels(&mut self.inline_states, cmd, frames);
            if let Some(t0) = t0 {
                let ns = t0.elapsed().as_nanos() as u64;
                self.timings.record_worker(0, ns);
                self.timings.record_dispatch(ns);
            }
            self.last_frames = if result.is_ok() { output_frames } else { 0 };
            return result;
        }
        let next_gen = self.current_generation.wrapping_add(1);
        let dispatch_start = timed.then(Instant::now);

        for slot in &self.slots {
            slot.sync.input_len.store(frames, Ordering::Relaxed);
            slot.sync.command.store(cmd, Ordering::Relaxed);
            slot.sync
                .target_generation
                .store(next_gen, Ordering::Release);
            futex_wake(&slot.sync.target_generation);
        }

        let coordinator_spin_limit = if self.coordinator_spin {
            WORKER_SPIN_ITERATIONS
        } else {
            0
        };
        let mut failed_worker = None;
        for (ch, slot) in self.slots.iter().enumerate() {
            let mut spins = 0;
            while slot.sync.completed_generation.load(Ordering::Acquire) != next_gen
                && spins < coordinator_spin_limit
            {
                core::hint::spin_loop();
                spins += 1;
            }
            while slot.sync.completed_generation.load(Ordering::Acquire) != next_gen {
                if self.thread_handles[ch].is_finished() {
                    failed_worker.get_or_insert(ch);
                    break;
                }
                futex_wait(&slot.sync.completed_generation, self.current_generation);
            }
        }

        self.current_generation = next_gen;
        if let Some(start) = dispatch_start
            && failed_worker.is_none()
        {
            self.timings
                .record_dispatch(start.elapsed().as_nanos() as u64);
        }
        if failed_worker.is_some() {
            // Reclaim shared buffers only after all surviving workers have
            // completed, and reject later dispatches before publishing controls.
            self.failed_worker = failed_worker;
            self.last_frames = 0;
            return self.ensure_ready();
        }

        let mut first_error = None;
        for slot in &self.slots {
            if slot.sync.has_error.load(Ordering::Acquire) {
                slot.sync.has_error.store(false, Ordering::Release);
                if let Some(err) = slot.error.lock().unwrap().take() {
                    first_error.get_or_insert(err);
                }
            }
        }

        let output_frames = self.slots[0].sync.output_len.load(Ordering::Relaxed);
        let aligned = self
            .slots
            .iter()
            .all(|slot| slot.sync.output_len.load(Ordering::Relaxed) == output_frames);
        self.last_frames = if aligned { output_frames } else { 0 };
        if let Some(err) = first_error {
            return Err(err);
        }
        if !aligned {
            return Err(EngineError::Pipeline(
                "channel workers produced different frame counts".into(),
            ));
        }
        Ok(())
    }

    /// Processes one multi-channel tile through the half-band FIR cascade only (PCM output).
    pub fn process_tile_pcm(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        self.ensure_ready()?;
        if inputs.len() != self.channel_buffers.len() {
            return Err(EngineError::Pipeline(format!(
                "ChannelWorkerPool channel count mismatch: expected {}, got {}",
                self.channel_buffers.len(),
                inputs.len()
            )));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let frames = inputs[0].len();
        for (i, in_ch) in inputs.iter().enumerate() {
            if in_ch.len() != frames {
                return Err(EngineError::Pipeline(format!(
                    "ChannelWorkerPool input channel {i} length {} differs from channel 0 length {frames}",
                    in_ch.len()
                )));
            }
            if frames > self.tile_baseband_frames {
                return Err(EngineError::Pipeline(format!(
                    "ChannelWorkerPool input frames {frames} exceeds capacity {}",
                    self.tile_baseband_frames
                )));
            }
            unsafe {
                let buf = &mut *self.channel_buffers[i].in_buf.get();
                buf[..frames].copy_from_slice(in_ch);
            }
        }

        self.dispatch_command(CMD_PCM, frames)
    }

    /// Processes one multi-channel tile through the half-band cascade and CRFB modulator (DSD output).
    pub fn process_tile_dsd(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        self.ensure_ready()?;
        if inputs.len() != self.channel_buffers.len() {
            return Err(EngineError::Pipeline(format!(
                "ChannelWorkerPool channel count mismatch: expected {}, got {}",
                self.channel_buffers.len(),
                inputs.len()
            )));
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let frames = inputs[0].len();
        for (i, in_ch) in inputs.iter().enumerate() {
            if in_ch.len() != frames {
                return Err(EngineError::Pipeline(format!(
                    "ChannelWorkerPool input channel {i} length {} differs from channel 0 length {frames}",
                    in_ch.len()
                )));
            }
            if frames > self.tile_baseband_frames {
                return Err(EngineError::Pipeline(format!(
                    "ChannelWorkerPool input frames {frames} exceeds capacity {}",
                    self.tile_baseband_frames
                )));
            }
            unsafe {
                let buf = &mut *self.channel_buffers[i].in_buf.get();
                buf[..frames].copy_from_slice(in_ch);
            }
        }

        self.dispatch_command(CMD_DSD, frames)
    }

    fn validate_input_channels<T>(
        &self,
        inputs: &[&[T]],
        capacity: usize,
    ) -> Result<usize, EngineError> {
        if inputs.len() != self.channel_buffers.len() {
            return Err(EngineError::Pipeline(
                "input DSP channel count mismatch".into(),
            ));
        }
        let frames = inputs[0].len();
        if frames > capacity || inputs.iter().any(|input| input.len() != frames) {
            return Err(EngineError::Pipeline(
                "input DSP channel lengths differ or exceed capacity".into(),
            ));
        }
        Ok(frames)
    }

    fn publish_input_controls(
        &self,
        controls: InputTileControls<'_>,
        frames: usize,
    ) -> Result<(), EngineError> {
        let config = self
            .input_config
            .ok_or_else(|| EngineError::InvalidTopology {
                message: "worker pool has no input DSP configuration".into(),
            })?;
        if controls.frame_gains.len() != frames {
            return Err(EngineError::Pipeline(
                "input DSP lifecycle frame count mismatch".into(),
            ));
        }
        config.validate_controls(controls)?;
        // The synchronous dispatch API guarantees the previous job is complete.
        unsafe { self.input_controls.as_ref().unwrap().publish(controls) };
        Ok(())
    }

    /// Processes raw PCM through decimation, lifecycle gain, SRC, and output DSP
    /// in one dispatch. Returns the common post-SRC frame count.
    /// Dispatched input advances residue even if output modulation fails; input
    /// stage errors require a full reset before continuing.
    pub fn process_input_pcm(
        &mut self,
        inputs: &[&[f64]],
        controls: InputTileControls<'_>,
    ) -> Result<usize, EngineError> {
        self.process_input_pcm_cmd(CMD_INPUT_PCM, inputs, controls)
    }

    /// Like [`process_input_pcm`](Self::process_input_pcm) but stops after the
    /// interpolation cascade: DSD targets are not modulated and `channel_pcm`
    /// holds the cascade output. Used to measure true peaks before pass 2.
    pub fn process_input_pcm_interpolated(
        &mut self,
        inputs: &[&[f64]],
        controls: InputTileControls<'_>,
    ) -> Result<usize, EngineError> {
        self.process_input_pcm_cmd(CMD_INPUT_PCM_INTERP, inputs, controls)
    }

    fn process_input_pcm_cmd(
        &mut self,
        cmd: u32,
        inputs: &[&[f64]],
        controls: InputTileControls<'_>,
    ) -> Result<usize, EngineError> {
        self.ensure_ready()?;
        let frames = self.validate_input_channels(inputs, self.input_frame_capacity())?;
        let baseband_frames = self.pcm_baseband_frames(frames)?;
        self.publish_input_controls(controls, baseband_frames)?;
        for (buffers, input) in self.channel_buffers.iter().zip(inputs) {
            let buffer = unsafe { &mut *buffers.in_buf.get() };
            buffer[..frames].copy_from_slice(input);
        }
        let result = self.dispatch_command(cmd, frames);
        self.pcm_residue_frames =
            (self.pcm_residue_frames + frames) % self.input_bounds.unwrap().pcm_divider;
        result?;
        Ok(self.last_frames)
    }

    /// Processes raw MSB-first DSD through unpacking/decimation, lifecycle gain,
    /// SRC, and output DSP in one dispatch. Incomplete decimation groups are
    /// retained by each worker, not discarded or padded at block boundaries.
    /// Dispatched input advances residue even if output modulation fails; input
    /// stage errors require a full reset before continuing.
    pub fn process_input_dsd(
        &mut self,
        inputs: &[&[u8]],
        controls: InputTileControls<'_>,
    ) -> Result<usize, EngineError> {
        self.ensure_ready()?;
        let bytes = self.validate_input_channels(inputs, self.input_dsd_byte_capacity())?;
        let decoded_frames = self.dsd_decoded_frames(bytes)?;
        let baseband_frames = self.dsd_baseband_frames(bytes)?;
        self.publish_input_controls(controls, baseband_frames)?;
        for (buffers, input) in self.channel_buffers.iter().zip(inputs) {
            let buffer = unsafe { &mut *buffers.dsd_in_buf.get() };
            buffer[..bytes].copy_from_slice(input);
        }
        let result = self.dispatch_command(CMD_INPUT_DSD, bytes);
        self.pcm_residue_frames =
            (self.pcm_residue_frames + decoded_frames) % self.input_bounds.unwrap().pcm_divider;
        self.dsd_residue_bytes =
            (self.dsd_residue_bytes + bytes) % self.input_bounds.unwrap().dsd_divider.div_ceil(8);
        result?;
        Ok(self.last_frames)
    }

    /// Returns the interpolated PCM output slice for a given channel from the last processed tile.
    #[inline]
    pub fn channel_pcm(&self, channel: usize) -> &[f64] {
        let out_len = self.last_frames * self.mult;
        unsafe {
            let buf = &*self.channel_buffers[channel].pcm_out_buf.get();
            &buf[..out_len]
        }
    }

    /// Returns the 1-bit packed DSD output byte slice for a given channel from the last processed tile.
    #[inline]
    pub fn channel_dsd(&self, channel: usize) -> &[u8] {
        let out_bytes = (self.last_frames * self.mult) / 8;
        unsafe {
            let buf = &*self.channel_buffers[channel].dsd_out_buf.get();
            &buf[..out_bytes]
        }
    }

    /// Resets input residue, SRC phase, filter delay lines, and modulators together.
    pub fn reset(&mut self) -> Result<(), EngineError> {
        self.dispatch_command(CMD_RESET, 0)?;
        self.pcm_residue_frames = 0;
        self.dsd_residue_bytes = 0;
        Ok(())
    }

    /// Reprimes output DSP after idle without discarding queued input residue or
    /// changing the input SRC phase. This preserves the live idle-resume contract.
    pub fn reset_output(&mut self) -> Result<(), EngineError> {
        self.dispatch_command(CMD_RESET_OUTPUT, 0)
    }
}

impl Drop for ChannelWorkerPool {
    fn drop(&mut self) {
        let next_gen = self.current_generation.wrapping_add(1);
        for slot in &self.slots {
            slot.sync.command.store(CMD_STOP, Ordering::Release);
            slot.sync
                .target_generation
                .store(next_gen, Ordering::Release);
            futex_wake(&slot.sync.target_generation);
        }
        for handle in self.thread_handles.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn test_dsd_subtile_errors_preserve_whole_tile_state() {
        for stages in [0, 1, 2, 3, 9] {
            let mult = 1 << stages;
            let subtile_frames = 32;
            for order in [5, 7] {
                for policy in [OverloadPolicy::Error, OverloadPolicy::MuteRecover] {
                    let mut whole =
                        HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, stages, 97);
                    let mut tiled = HalfBandCascade::new(
                        FilterPhase::Linear,
                        Apodization::Off,
                        stages,
                        subtile_frames,
                    );
                    let mut expected_mod =
                        Modulator::new(order, DsdBitRateHz::DSD512, DitherType::Tpdf, policy)
                            .unwrap();
                    let mut actual_mod = expected_mod.clone();
                    let mut expected_pcm = vec![0.0; 97 * mult];
                    let mut actual_pcm = expected_pcm.clone();
                    let mut expected_dsd = vec![0xa5; 97 * mult / 8];
                    let mut actual_dsd = expected_dsd.clone();
                    for (frames, amplitude) in
                        [(96, 0.0), (96, 5.0), (97, 0.0), (96, f64::NAN), (96, 0.0)]
                    {
                        let input = vec![amplitude; frames];
                        let expected_pcm = &mut expected_pcm[..frames * mult];
                        let actual_pcm = &mut actual_pcm[..frames * mult];
                        let expected_dsd = &mut expected_dsd[..frames * mult / 8];
                        let actual_dsd = &mut actual_dsd[..frames * mult / 8];
                        whole.process_tile(&input, expected_pcm).unwrap();
                        let expected = expected_mod.process_tile_bytes(expected_pcm, expected_dsd);
                        let actual = process_dsd_tile(
                            &mut tiled,
                            &mut actual_mod,
                            None,
                            &input,
                            actual_pcm,
                            actual_dsd,
                            subtile_frames,
                        );
                        match (expected, actual) {
                            (Ok(()), Ok(())) => {}
                            (Err(e), Err(EngineError::Modulator(a))) => {
                                // NaNs are not PartialEq, so compare diagnostic text and
                                // the exact persisted state/dither progression separately.
                                assert_eq!(format!("{a:?}"), format!("{e:?}"));
                            }
                            (e, a) => panic!("error mismatch: {e:?} vs {a:?}"),
                        }
                        for (a, e) in actual_pcm.iter().zip(expected_pcm) {
                            assert_eq!(a.to_bits(), e.to_bits());
                        }
                        assert_eq!(actual_dsd, expected_dsd);
                        assert_eq!(actual_mod.overload_count(), expected_mod.overload_count());
                        for (a, e) in actual_mod
                            .state_slice()
                            .iter()
                            .zip(expected_mod.state_slice())
                        {
                            assert_eq!(a.to_bits(), e.to_bits());
                        }
                    }
                    // Remove the NaN-contaminated FIR history without reseeding
                    // dither, and verify the PRNG continuation after error paths.
                    whole.reset();
                    tiled.reset();
                    expected_mod.reset();
                    actual_mod.reset();
                    let input = [0.0; 96];
                    whole
                        .process_tile(&input, &mut expected_pcm[..96 * mult])
                        .unwrap();
                    expected_mod
                        .process_tile_bytes(
                            &expected_pcm[..96 * mult],
                            &mut expected_dsd[..96 * mult / 8],
                        )
                        .unwrap();
                    process_dsd_tile(
                        &mut tiled,
                        &mut actual_mod,
                        None,
                        &input,
                        &mut actual_pcm[..96 * mult],
                        &mut actual_dsd[..96 * mult / 8],
                        subtile_frames,
                    )
                    .unwrap();
                    assert_eq!(&actual_dsd[..96 * mult / 8], &expected_dsd[..96 * mult / 8]);
                }
            }
        }
    }

    #[test]
    fn test_zero_tile_capacity_is_rejected() {
        assert!(matches!(
            ChannelWorkerPool::new(1, FilterPhase::Linear, Apodization::Off, 9, 0, None, None),
            Err(EngineError::InvalidTopology { .. })
        ));
    }

    #[test]
    fn test_worker_pool_bit_exact_with_sequential() {
        const CHANNELS: usize = 2;
        const STAGES: usize = 6; // 64x upsampling
        const FRAMES: usize = 256;
        const BIT_RATE: DsdBitRateHz = DsdBitRateHz::DSD64;

        let dsd_cfg = TargetDsdConfig {
            bit_rate: BIT_RATE,
            modulator_order: 5,
            dither: DitherType::Tpdf,
            overload_policy: OverloadPolicy::MuteRecover,
            seed_base: 12345,
        };

        let mut pool = ChannelWorkerPool::new(
            CHANNELS,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            Some(dsd_cfg.clone()),
            None,
        )
        .expect("worker pool creation failed");

        // Sequential reference
        let mut seq_cascade_l =
            HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);
        let mut seq_cascade_r =
            HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);
        let mut seq_mod_l = Modulator::new(
            dsd_cfg.modulator_order,
            BIT_RATE,
            dsd_cfg.dither,
            dsd_cfg.overload_policy,
        )
        .unwrap();
        seq_mod_l.set_dither_seed(12345);
        let mut seq_mod_r = Modulator::new(
            dsd_cfg.modulator_order,
            BIT_RATE,
            dsd_cfg.dither,
            dsd_cfg.overload_policy,
        )
        .unwrap();
        seq_mod_r.set_dither_seed(12346);
        // The pool removes DC ahead of the cascade on DSD targets.
        let baseband_hz = BIT_RATE.get() as f64 / (1usize << STAGES) as f64;
        let mut seq_dc_l = DcBlocker::production(baseband_hz);
        let mut seq_dc_r = DcBlocker::production(baseband_hz);
        let mut dc_left = vec![0.0f64; FRAMES];
        let mut dc_right = vec![0.0f64; FRAMES];

        // Generate synthetic stereo signal
        let mut left = vec![0.0f64; FRAMES];
        let mut right = vec![0.0f64; FRAMES];
        for i in 0..FRAMES {
            left[i] = 0.5 * (2.0 * PI * 1000.0 * (i as f64 / 44100.0)).sin();
            right[i] = 0.4 * (2.0 * PI * 2000.0 * (i as f64 / 44100.0)).sin();
        }

        let mult = pool.rate_multiplier();
        let req_out = FRAMES * mult;
        let mut seq_pcm_l = vec![0.0f64; req_out];
        let mut seq_pcm_r = vec![0.0f64; req_out];
        let mut seq_dsd_l = vec![0u8; req_out / 8];
        let mut seq_dsd_r = vec![0u8; req_out / 8];

        for _ in 0..10 {
            // Sequential step
            seq_dc_l.process(&left, &mut dc_left);
            seq_dc_r.process(&right, &mut dc_right);
            seq_cascade_l
                .process_tile(&dc_left, &mut seq_pcm_l)
                .unwrap();
            seq_cascade_r
                .process_tile(&dc_right, &mut seq_pcm_r)
                .unwrap();
            seq_mod_l
                .process_tile_bytes(&seq_pcm_l, &mut seq_dsd_l)
                .unwrap();
            seq_mod_r
                .process_tile_bytes(&seq_pcm_r, &mut seq_dsd_r)
                .unwrap();

            // Parallel worker pool step
            pool.process_tile_dsd(&[&left, &right]).unwrap();
            let pool_pcm_l = pool.channel_pcm(0);
            let pool_pcm_r = pool.channel_pcm(1);
            let pool_dsd_l = pool.channel_dsd(0);
            let pool_dsd_r = pool.channel_dsd(1);

            // Verify PCM bit-exact equivalence
            assert_eq!(seq_pcm_l.as_slice(), pool_pcm_l);
            assert_eq!(seq_pcm_r.as_slice(), pool_pcm_r);

            // Verify DSD bit-exact equivalence
            assert_eq!(seq_dsd_l.as_slice(), pool_dsd_l);
            assert_eq!(seq_dsd_r.as_slice(), pool_dsd_r);
        }
    }

    #[test]
    fn test_worker_pool_dsd7_bit_exact() {
        const CHANNELS: usize = 2;
        const STAGES: usize = 7; // 128x upsampling -> DSD128
        const FRAMES: usize = 256;
        const BIT_RATE: DsdBitRateHz = DsdBitRateHz::DSD128;

        let dsd_cfg = TargetDsdConfig {
            bit_rate: BIT_RATE,
            modulator_order: 7,
            dither: DitherType::Tpdf,
            overload_policy: OverloadPolicy::MuteRecover,
            seed_base: 98765,
        };

        let mut pool = ChannelWorkerPool::new(
            CHANNELS,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            Some(dsd_cfg.clone()),
            None,
        )
        .expect("worker pool creation failed");

        let mut seq_cascade_l =
            HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);
        let mut seq_cascade_r =
            HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);
        let mut seq_mod_l = Modulator::new(
            dsd_cfg.modulator_order,
            BIT_RATE,
            dsd_cfg.dither,
            dsd_cfg.overload_policy,
        )
        .unwrap();
        seq_mod_l.set_dither_seed(98765);
        let mut seq_mod_r = Modulator::new(
            dsd_cfg.modulator_order,
            BIT_RATE,
            dsd_cfg.dither,
            dsd_cfg.overload_policy,
        )
        .unwrap();
        seq_mod_r.set_dither_seed(98766);
        // The pool removes DC ahead of the cascade on DSD targets.
        let baseband_hz = BIT_RATE.get() as f64 / (1usize << STAGES) as f64;
        let mut seq_dc_l = DcBlocker::production(baseband_hz);
        let mut seq_dc_r = DcBlocker::production(baseband_hz);
        let mut dc_left = vec![0.0f64; FRAMES];
        let mut dc_right = vec![0.0f64; FRAMES];

        let mut left = vec![0.0f64; FRAMES];
        let mut right = vec![0.0f64; FRAMES];
        for i in 0..FRAMES {
            left[i] = 0.5 * (2.0 * PI * 1000.0 * (i as f64 / 44100.0)).sin();
            right[i] = 0.4 * (2.0 * PI * 2000.0 * (i as f64 / 44100.0)).sin();
        }

        let mult = pool.rate_multiplier();
        let req_out = FRAMES * mult;
        let mut seq_pcm_l = vec![0.0f64; req_out];
        let mut seq_pcm_r = vec![0.0f64; req_out];
        let mut seq_dsd_l = vec![0u8; req_out / 8];
        let mut seq_dsd_r = vec![0u8; req_out / 8];

        for _ in 0..10 {
            seq_dc_l.process(&left, &mut dc_left);
            seq_dc_r.process(&right, &mut dc_right);
            seq_cascade_l
                .process_tile(&dc_left, &mut seq_pcm_l)
                .unwrap();
            seq_cascade_r
                .process_tile(&dc_right, &mut seq_pcm_r)
                .unwrap();
            seq_mod_l
                .process_tile_bytes(&seq_pcm_l, &mut seq_dsd_l)
                .unwrap();
            seq_mod_r
                .process_tile_bytes(&seq_pcm_r, &mut seq_dsd_r)
                .unwrap();

            pool.process_tile_dsd(&[&left, &right]).unwrap();

            let pool_dsd_l = pool.channel_dsd(0);
            let pool_dsd_r = pool.channel_dsd(1);

            assert_eq!(seq_dsd_l.as_slice(), pool_dsd_l);
            assert_eq!(seq_dsd_r.as_slice(), pool_dsd_r);
        }
    }

    #[test]
    fn test_worker_pool_multichannel_5_1_pcm() {
        const CHANNELS: usize = 6;
        const STAGES: usize = 2; // 4x
        const FRAMES: usize = 128;

        let mut pool = ChannelWorkerPool::new(
            CHANNELS,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            None,
            None,
        )
        .expect("multichannel worker pool failed");

        assert_eq!(pool.channels(), 6);
        assert_eq!(pool.rate_multiplier(), 4);

        let channels_data: Vec<Vec<f64>> = (0..CHANNELS)
            .map(|ch| vec![(ch as f64 + 1.0) * 0.1; FRAMES])
            .collect();
        let input_slices: Vec<&[f64]> = channels_data.iter().map(|v| v.as_slice()).collect();

        pool.process_tile_pcm(&input_slices).unwrap();

        for ch in 0..CHANNELS {
            let pcm = pool.channel_pcm(ch);
            assert_eq!(pcm.len(), FRAMES * 4);
        }
    }

    #[test]
    fn test_worker_pool_reset() {
        const CHANNELS: usize = 2;
        const STAGES: usize = 2;
        const FRAMES: usize = 128;

        let mut pool = ChannelWorkerPool::new(
            CHANNELS,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            None,
            None,
        )
        .expect("worker pool creation failed");

        let input_a = vec![1.0f64; FRAMES];
        let input_b = vec![1.0f64; FRAMES];

        pool.process_tile_pcm(&[&input_a, &input_b]).unwrap();
        let out_dirty = pool.channel_pcm(0).to_vec();

        pool.reset().unwrap();

        pool.process_tile_pcm(&[&input_a, &input_b]).unwrap();
        let out_reset = pool.channel_pcm(0).to_vec();

        // After reset, processing the exact same initial block must yield the exact same output as the initial clean state
        assert_eq!(out_dirty, out_reset);
    }

    #[test]
    fn test_worker_pool_zero_channels_returns_invalid_topology() {
        let res =
            ChannelWorkerPool::new(0, FilterPhase::Linear, Apodization::Off, 2, 128, None, None);
        match res {
            Err(EngineError::InvalidTopology { message }) => {
                assert!(message.contains("requires at least 1 channel"));
            }
            _ => panic!("expected InvalidTopology"),
        }
    }
}
