//! Persistent real-time multi-channel worker pool (`Design.md §1.2`, Phase 5 Step 5.3).
//!
//! Provides:
//! - Dedicated persistent worker threads (1 thread per channel) spawned upon pipeline preparation.
//! - Lock-free synchronization barrier using cache-line aligned atomic flags and Linux futex.
//! - POSIX real-time scheduling priority (`SCHED_FIFO` via `pthread_setschedparam` / `RLIMIT_RTPRIO`)
//!   and thread-local hardware FTZ/DAZ denormal guards.
//! - Zero heap allocation and zero lock contention during steady-state processing.
//! - Isolated per-channel FIR cascade and CRFB modulator states.
//! - Bounded internal DSD subtiles without additional worker synchronization.

use crate::traits::EngineError;
use audigma_dsp::config::{Apodization, DitherType, FilterPhase, FilterPreset, OverloadPolicy};
use audigma_dsp::fir::HalfBandCascade;
use audigma_dsp::ftz::ScopedFtzDaz;
use audigma_dsp::modulator::Modulator;
use audigma_dsp::rates::DsdBitRateHz;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const CMD_IDLE: u32 = 0;
const CMD_PCM: u32 = 1;
const CMD_DSD: u32 = 2;
const CMD_RESET: u32 = 3;
const CMD_STOP: u32 = 4;

// At DSD512 this is 32 baseband frames; at DSD256 it is 64. Keep the
// externally scheduled tile intact and interpolate/modulate each slice while
// it is still hot, rather than rereading the whole high-rate PCM tile later.
const DSD_SUBTILE_SAMPLES: usize = 16_384;

fn process_pcm_tile(
    cascade: &mut HalfBandCascade,
    input: &[f64],
    output: &mut [f64],
    subtile_frames: usize,
) -> Result<(), EngineError> {
    let subtile_samples = subtile_frames * cascade.rate_multiplier();
    for (tile, out) in input
        .chunks(subtile_frames)
        .zip(output.chunks_mut(subtile_samples))
    {
        cascade.process_tile(tile, out)?;
    }
    Ok(())
}

fn process_dsd_tile(
    cascade: &mut HalfBandCascade,
    modulator: &mut Modulator,
    input: &[f64],
    pcm: &mut [f64],
    dsd: &mut [u8],
    subtile_frames: usize,
) -> Result<(), EngineError> {
    // Preserve the original validation/error ordering: the FIR advances through
    // the entire tile even if the modulator rejects an unaligned input length.
    if pcm.len() % 8 != 0 {
        process_pcm_tile(cascade, input, pcm, subtile_frames)?;
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
        cascade.process_tile(tile, out_pcm)?;
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
    has_error: AtomicBool,
    overload_count: AtomicU64,
}

impl WorkerSync {
    fn new() -> Self {
        Self {
            target_generation: AtomicU32::new(0),
            completed_generation: AtomicU32::new(0),
            command: AtomicU32::new(CMD_IDLE),
            input_len: AtomicUsize::new(0),
            has_error: AtomicBool::new(false),
            overload_count: AtomicU64::new(0),
        }
    }
}

/// Pre-allocated, zero-copy shared buffers for one channel worker.
struct WorkerBuffers {
    in_buf: UnsafeCell<Vec<f64>>,
    pcm_out_buf: UnsafeCell<Vec<f64>>,
    dsd_out_buf: UnsafeCell<Vec<u8>>,
}

unsafe impl Sync for WorkerBuffers {}

struct WorkerSlot {
    sync: Arc<WorkerSync>,
    buffers: Arc<WorkerBuffers>,
    error: Arc<Mutex<Option<EngineError>>>,
}

/// Persistent real-time worker pool for multi-channel audio DSP.
pub struct ChannelWorkerPool {
    slots: Vec<WorkerSlot>,
    thread_handles: Vec<JoinHandle<()>>,
    current_generation: u32,
    last_frames: usize,
    tile_baseband_frames: usize,
    mult: usize,
    group_delay_output_samples: usize,
}

impl ChannelWorkerPool {
    /// Spawns $N$ persistent channel worker threads upon pipeline preparation.
    // The constructor mirrors the prepared pipeline configuration explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: usize,
        preset: FilterPreset,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        tile_baseband_frames: usize,
        target_dsd: Option<TargetDsdConfig>,
        thread_init_hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    ) -> Result<Self, EngineError> {
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

        let mult = 1usize << stages;
        let subtile_frames = if target_dsd.is_some() {
            tile_baseband_frames.min((DSD_SUBTILE_SAMPLES / mult).max(1))
        } else {
            tile_baseband_frames
        };
        let cascade_probe =
            HalfBandCascade::new(preset, phase, apodization, stages, subtile_frames);
        let mult = cascade_probe.rate_multiplier();
        let group_delay_output_samples = cascade_probe.total_group_delay_output_samples();
        let cascade_tile_len = tile_baseband_frames * mult;
        let dsd_tile_bytes = cascade_tile_len / 8;

        let mut slots = Vec::with_capacity(channels);
        let mut thread_handles = Vec::with_capacity(channels);

        for ch in 0..channels {
            let sync = Arc::new(WorkerSync::new());
            let buffers = Arc::new(WorkerBuffers {
                in_buf: UnsafeCell::new(vec![0.0f64; tile_baseband_frames]),
                pcm_out_buf: UnsafeCell::new(vec![0.0f64; cascade_tile_len]),
                dsd_out_buf: UnsafeCell::new(vec![0u8; dsd_tile_bytes.max(1)]),
            });
            let error = Arc::new(Mutex::new(None));

            let slot = WorkerSlot {
                sync: Arc::clone(&sync),
                buffers: Arc::clone(&buffers),
                error: Arc::clone(&error),
            };
            slots.push(slot);

            // Construct isolated cascade and modulator for this worker
            let mut cascade =
                HalfBandCascade::new(preset, phase, apodization, stages, subtile_frames);
            let mut modulator = if let Some(ref dsd_cfg) = target_dsd {
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

            let thread_sync = Arc::clone(&sync);
            let thread_buffers = Arc::clone(&buffers);
            let thread_error = Arc::clone(&error);
            let thread_hook = thread_init_hook.clone();

            let handle = std::thread::Builder::new()
                .name(format!("audigma-ch{ch}"))
                .spawn(move || {
                    let _ftz = ScopedFtzDaz::new();

                    if let Some(ref hook) = thread_hook {
                        hook(ch);
                    }

                    let mut current_gen = 0u32;
                    loop {
                        let mut spins = 0;
                        while thread_sync.target_generation.load(Ordering::Acquire) == current_gen
                            && spins < 2048
                        {
                            core::hint::spin_loop();
                            spins += 1;
                        }
                        while thread_sync.target_generation.load(Ordering::Acquire) == current_gen {
                            futex_wait(&thread_sync.target_generation, current_gen);
                        }

                        current_gen = thread_sync.target_generation.load(Ordering::Acquire);
                        let cmd = thread_sync.command.load(Ordering::Acquire);

                        match cmd {
                            CMD_STOP => break,
                            CMD_RESET => {
                                cascade.reset();
                                if let Some(ref mut m) = modulator {
                                    m.reset();
                                }
                            }
                            CMD_PCM => {
                                let in_len = thread_sync.input_len.load(Ordering::Relaxed);
                                let in_buf = unsafe { &*thread_buffers.in_buf.get() };
                                let in_slice = &in_buf[..in_len];
                                let pcm_buf = unsafe { &mut *thread_buffers.pcm_out_buf.get() };
                                let pcm_out = &mut pcm_buf[..in_len * mult];
                                if let Err(e) = process_pcm_tile(
                                    &mut cascade,
                                    in_slice,
                                    pcm_out,
                                    subtile_frames,
                                ) {
                                    thread_sync.has_error.store(true, Ordering::Release);
                                    let mut err = thread_error.lock().unwrap();
                                    *err = Some(e);
                                }
                            }
                            CMD_DSD => {
                                let in_len = thread_sync.input_len.load(Ordering::Relaxed);
                                let in_buf = unsafe { &*thread_buffers.in_buf.get() };
                                let in_slice = &in_buf[..in_len];
                                let req_pcm = in_len * mult;
                                let pcm_buf = unsafe { &mut *thread_buffers.pcm_out_buf.get() };
                                let pcm_out = &mut pcm_buf[..req_pcm];
                                let result = if let Some(ref mut m) = modulator {
                                    let req_bytes = req_pcm / 8;
                                    let dsd_buf = unsafe { &mut *thread_buffers.dsd_out_buf.get() };
                                    let dsd_out = &mut dsd_buf[..req_bytes];
                                    let result = process_dsd_tile(
                                        &mut cascade,
                                        m,
                                        in_slice,
                                        pcm_out,
                                        dsd_out,
                                        subtile_frames,
                                    );
                                    thread_sync
                                        .overload_count
                                        .store(m.overload_count(), Ordering::Relaxed);
                                    result
                                } else {
                                    process_pcm_tile(
                                        &mut cascade,
                                        in_slice,
                                        pcm_out,
                                        subtile_frames,
                                    )
                                };
                                if let Err(e) = result {
                                    thread_sync.has_error.store(true, Ordering::Release);
                                    let mut err = thread_error.lock().unwrap();
                                    *err = Some(e);
                                }
                            }
                            _ => {}
                        }

                        thread_sync
                            .completed_generation
                            .store(current_gen, Ordering::Release);
                        futex_wake(&thread_sync.completed_generation);
                    }
                })
                .map_err(|e| EngineError::WorkerThread {
                    channel: ch,
                    reason: format!("failed to spawn worker thread {ch}: {e}"),
                })?;

            thread_handles.push(handle);
        }

        Ok(Self {
            slots,
            thread_handles,
            current_generation: 0,
            last_frames: 0,
            tile_baseband_frames,
            mult,
            group_delay_output_samples,
        })
    }

    /// Number of active channel workers.
    #[inline]
    pub fn channels(&self) -> usize {
        self.slots.len()
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
        self.slots
            .iter()
            .map(|s| s.sync.overload_count.load(Ordering::Relaxed))
            .sum()
    }

    /// Overload count for a specific channel.
    #[inline]
    pub fn channel_overload_count(&self, channel: usize) -> u64 {
        self.slots[channel]
            .sync
            .overload_count
            .load(Ordering::Relaxed)
    }

    fn dispatch_command(&mut self, cmd: u32, frames: usize) -> Result<(), EngineError> {
        let next_gen = self.current_generation.wrapping_add(1);

        for slot in &self.slots {
            slot.sync.input_len.store(frames, Ordering::Relaxed);
            slot.sync.command.store(cmd, Ordering::Relaxed);
            slot.sync
                .target_generation
                .store(next_gen, Ordering::Release);
            futex_wake(&slot.sync.target_generation);
        }

        for (ch, slot) in self.slots.iter().enumerate() {
            let mut spins = 0;
            while slot.sync.completed_generation.load(Ordering::Acquire) != next_gen && spins < 2048
            {
                core::hint::spin_loop();
                spins += 1;
            }
            while slot.sync.completed_generation.load(Ordering::Acquire) != next_gen {
                if self.thread_handles[ch].is_finished() {
                    return Err(EngineError::WorkerThread {
                        channel: ch,
                        reason: format!("worker thread {ch} terminated unexpectedly"),
                    });
                }
                futex_wait(&slot.sync.completed_generation, self.current_generation);
            }
        }

        self.current_generation = next_gen;
        self.last_frames = frames;

        for slot in &self.slots {
            if slot.sync.has_error.load(Ordering::Acquire) {
                slot.sync.has_error.store(false, Ordering::Release);
                if let Some(err) = slot.error.lock().unwrap().take() {
                    return Err(err);
                }
            }
        }

        Ok(())
    }

    /// Processes one multi-channel tile through the half-band FIR cascade only (PCM output).
    pub fn process_tile_pcm(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        if inputs.len() != self.slots.len() {
            return Err(EngineError::Pipeline(format!(
                "ChannelWorkerPool channel count mismatch: expected {}, got {}",
                self.slots.len(),
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
                let buf = &mut *self.slots[i].buffers.in_buf.get();
                buf[..frames].copy_from_slice(in_ch);
            }
        }

        self.dispatch_command(CMD_PCM, frames)
    }

    /// Processes one multi-channel tile through the half-band cascade and CRFB modulator (DSD output).
    pub fn process_tile_dsd(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        if inputs.len() != self.slots.len() {
            return Err(EngineError::Pipeline(format!(
                "ChannelWorkerPool channel count mismatch: expected {}, got {}",
                self.slots.len(),
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
                let buf = &mut *self.slots[i].buffers.in_buf.get();
                buf[..frames].copy_from_slice(in_ch);
            }
        }

        self.dispatch_command(CMD_DSD, frames)
    }

    /// Returns the interpolated PCM output slice for a given channel from the last processed tile.
    #[inline]
    pub fn channel_pcm(&self, channel: usize) -> &[f64] {
        let out_len = self.last_frames * self.mult;
        unsafe {
            let buf = &*self.slots[channel].buffers.pcm_out_buf.get();
            &buf[..out_len]
        }
    }

    /// Returns the 1-bit packed DSD output byte slice for a given channel from the last processed tile.
    #[inline]
    pub fn channel_dsd(&self, channel: usize) -> &[u8] {
        let out_bytes = (self.last_frames * self.mult) / 8;
        unsafe {
            let buf = &*self.slots[channel].buffers.dsd_out_buf.get();
            &buf[..out_bytes]
        }
    }

    /// Resets all channel filter delay lines and modulator integrators to clean state.
    pub fn reset(&mut self) -> Result<(), EngineError> {
        self.dispatch_command(CMD_RESET, 0)
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
    use audigma_dsp::presets::PRESET_BALANCED;
    use std::f64::consts::PI;

    #[test]
    fn test_dsd_subtile_errors_preserve_whole_tile_state() {
        for stages in [0, 1, 2, 3, 9] {
            let mult = 1 << stages;
            let subtile_frames = 32;
            for order in [5, 7] {
                for policy in [OverloadPolicy::Error, OverloadPolicy::MuteRecover] {
                    let mut whole = HalfBandCascade::new(
                        FilterPreset::Balanced,
                        FilterPhase::Linear,
                        Apodization::Off,
                        stages,
                        97,
                    );
                    let mut tiled = HalfBandCascade::new(
                        FilterPreset::Balanced,
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
            ChannelWorkerPool::new(
                1,
                FilterPreset::Balanced,
                FilterPhase::Linear,
                Apodization::Off,
                9,
                0,
                None,
                None
            ),
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
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            Some(dsd_cfg.clone()),
            None,
        )
        .expect("worker pool creation failed");

        // Sequential reference
        let mut seq_cascade_l = HalfBandCascade::new(
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
        );
        let mut seq_cascade_r = HalfBandCascade::new(
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
        );
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
            seq_cascade_l.process_tile(&left, &mut seq_pcm_l).unwrap();
            seq_cascade_r.process_tile(&right, &mut seq_pcm_r).unwrap();
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
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
            Some(dsd_cfg.clone()),
            None,
        )
        .expect("worker pool creation failed");

        let mut seq_cascade_l = HalfBandCascade::new(
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
        );
        let mut seq_cascade_r = HalfBandCascade::new(
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            STAGES,
            FRAMES,
        );
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
            seq_cascade_l.process_tile(&left, &mut seq_pcm_l).unwrap();
            seq_cascade_r.process_tile(&right, &mut seq_pcm_r).unwrap();
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
            PRESET_BALANCED.preset,
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
            PRESET_BALANCED.preset,
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
        let res = ChannelWorkerPool::new(
            0,
            PRESET_BALANCED.preset,
            FilterPhase::Linear,
            Apodization::Off,
            2,
            128,
            None,
            None,
        );
        match res {
            Err(EngineError::InvalidTopology { message }) => {
                assert!(message.contains("requires at least 1 channel"));
            }
            _ => panic!("expected InvalidTopology"),
        }
    }
}
