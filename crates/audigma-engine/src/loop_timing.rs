//! End-to-end feed-loop timing and device-headroom telemetry.
//!
//! [`crate::late`] only times the worker DSP dispatch. This module covers the rest of
//! the live loop (input read, control preparation, DSP dispatch, pack + device write)
//! and, where the output backend can report it, how much audio is queued in the device.
//!
//! The key quantity is *headroom*: after each emit, the device holds `buffered` audio.
//! If the next emit starts `gap` later, then `buffered - gap` is the remaining margin.
//! A negative headroom means the device ran dry between two writes, which is audible as
//! a missing-sample pop regardless of whether any single stage exceeded its budget or
//! the backend reported an xrun.
//!
//! The audio path only touches relaxed atomics. A non-real-time thread
//! (`audigma-health`) polls them and emits `audigma::late` records through `tracing`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::late::{LATE_POLL_INTERVAL, LateCounterSource, LateCounters};
use crate::timing::{MaxSlot, PoolTimings, TimingReport, TimingTracker};
use crate::traits::AudioOutput;

/// Minimum spacing between `loop_stall` records; anomalies in between are merged.
const STALL_MIN_SPACING: Duration = Duration::from_secs(1);

const NO_HEADROOM: i64 = i64::MAX;

/// Interval-scoped (or merged) view of the feed-loop timing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopSnapshot {
    /// Productive loop iterations (tiles emitted).
    pub tiles: u64,
    pub read_max_us: u64,
    /// Time from end of read to start of DSP dispatch (gains, drift, publish).
    pub prep_max_us: u64,
    /// Whole DSP dispatch as seen by the loop (includes the worker wait).
    pub dsp_max_us: u64,
    /// Pack + device write. May legitimately block when the device buffer is full.
    pub emit_max_us: u64,
    /// Longest span in which the loop was not writing: end of previous emit to start of next.
    pub feed_gap_max_us: u64,
    /// Smallest `buffered - gap`, in microseconds. `None` if the backend reports no delay.
    pub headroom_min_us: Option<i64>,
    /// Emits that began after the device buffer should have run dry (`headroom < 0`).
    pub starved: u64,
    /// Smallest device-buffered audio observed right after an emit.
    pub buffered_min_us: Option<u64>,
}

impl LoopSnapshot {
    fn merge(&mut self, o: &Self) {
        self.tiles = self.tiles.saturating_add(o.tiles);
        self.read_max_us = self.read_max_us.max(o.read_max_us);
        self.prep_max_us = self.prep_max_us.max(o.prep_max_us);
        self.dsp_max_us = self.dsp_max_us.max(o.dsp_max_us);
        self.emit_max_us = self.emit_max_us.max(o.emit_max_us);
        self.feed_gap_max_us = self.feed_gap_max_us.max(o.feed_gap_max_us);
        self.headroom_min_us = min_opt(self.headroom_min_us, o.headroom_min_us);
        self.starved = self.starved.saturating_add(o.starved);
        self.buffered_min_us = min_opt(self.buffered_min_us, o.buffered_min_us);
    }
}

fn min_opt<T: Ord>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Lock-free feed-loop telemetry shared by the live audio loop and the aggregator.
pub struct LoopTimingTelemetry {
    origin: Instant,
    tiles: AtomicU64,
    read_max_us: AtomicU64,
    prep_max_us: AtomicU64,
    dsp_max_us: AtomicU64,
    emit_max_us: AtomicU64,
    feed_gap_max_us: AtomicU64,
    headroom_min_us: AtomicI64,
    buffered_min_us: AtomicU64,
    starved: AtomicU64,
    total_starved: AtomicU64,
}

impl Default for LoopTimingTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl LoopTimingTelemetry {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            tiles: AtomicU64::new(0),
            read_max_us: AtomicU64::new(0),
            prep_max_us: AtomicU64::new(0),
            dsp_max_us: AtomicU64::new(0),
            emit_max_us: AtomicU64::new(0),
            feed_gap_max_us: AtomicU64::new(0),
            headroom_min_us: AtomicI64::new(NO_HEADROOM),
            buffered_min_us: AtomicU64::new(u64::MAX),
            starved: AtomicU64::new(0),
            total_starved: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn mono_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }

    #[inline]
    pub fn total_starved(&self) -> u64 {
        self.total_starved.load(Ordering::Relaxed)
    }

    /// Takes and resets the interval-scoped values.
    pub fn take_interval(&self) -> LoopSnapshot {
        let headroom = self.headroom_min_us.swap(NO_HEADROOM, Ordering::AcqRel);
        let buffered = self.buffered_min_us.swap(u64::MAX, Ordering::AcqRel);
        LoopSnapshot {
            tiles: self.tiles.swap(0, Ordering::AcqRel),
            read_max_us: self.read_max_us.swap(0, Ordering::AcqRel),
            prep_max_us: self.prep_max_us.swap(0, Ordering::AcqRel),
            dsp_max_us: self.dsp_max_us.swap(0, Ordering::AcqRel),
            emit_max_us: self.emit_max_us.swap(0, Ordering::AcqRel),
            feed_gap_max_us: self.feed_gap_max_us.swap(0, Ordering::AcqRel),
            headroom_min_us: (headroom != NO_HEADROOM).then_some(headroom),
            starved: self.starved.swap(0, Ordering::AcqRel),
            buffered_min_us: (buffered != u64::MAX).then_some(buffered),
        }
    }
}

#[inline]
fn us(d: Duration) -> u64 {
    d.as_micros() as u64
}

/// Audio-thread-local recorder. Owns the "previous emit" baseline.
pub struct LoopProbe {
    telemetry: Arc<LoopTimingTelemetry>,
    last_emit_end: Option<Instant>,
    last_buffered_us: Option<u64>,
}

impl LoopProbe {
    pub fn new(telemetry: Arc<LoopTimingTelemetry>) -> Self {
        Self {
            telemetry,
            last_emit_end: None,
            last_buffered_us: None,
        }
    }

    /// Records the non-emit stages of one productive iteration.
    #[inline]
    pub fn record_stages(&self, read: Duration, prep: Duration, dsp: Duration) {
        let t = &self.telemetry;
        t.read_max_us.fetch_max(us(read), Ordering::Relaxed);
        t.prep_max_us.fetch_max(us(prep), Ordering::Relaxed);
        t.dsp_max_us.fetch_max(us(dsp), Ordering::Relaxed);
    }

    /// Runs `f` (pack + device write) and records emit time, feed gap and headroom.
    #[inline]
    pub fn timed_emit<T>(
        &mut self,
        output: &mut dyn AudioOutput,
        f: impl FnOnce(&mut dyn AudioOutput) -> T,
    ) -> T {
        let start = Instant::now();
        let t = &self.telemetry;
        if let Some(prev_end) = self.last_emit_end {
            let gap = start.saturating_duration_since(prev_end);
            let gap_us = us(gap);
            t.feed_gap_max_us.fetch_max(gap_us, Ordering::Relaxed);
            if let Some(buffered) = self.last_buffered_us {
                let headroom = buffered as i64 - gap_us as i64;
                t.headroom_min_us.fetch_min(headroom, Ordering::Relaxed);
                if headroom < 0 {
                    t.starved.fetch_add(1, Ordering::Relaxed);
                    t.total_starved.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        let result = f(&mut *output);

        let end = Instant::now();
        t.emit_max_us
            .fetch_max(us(end.saturating_duration_since(start)), Ordering::Relaxed);
        t.tiles.fetch_add(1, Ordering::Relaxed);
        self.last_buffered_us = output.buffered_us();
        if let Some(b) = self.last_buffered_us {
            t.buffered_min_us.fetch_min(b, Ordering::Relaxed);
        }
        // Take the baseline after the delay query so the query itself is not counted as gap.
        self.last_emit_end = Some(Instant::now());
        result
    }

    /// Drops the previous-emit baseline (stream restart, device reopen).
    #[inline]
    pub fn reset_baseline(&mut self) {
        self.last_emit_end = None;
        self.last_buffered_us = None;
    }
}

/// Number of recent poll intervals whose empty-poll counts still explain a stall. A stall is
/// recorded when the loop finally emits after its gap, so the empty polls happened in the
/// (non-anomalous, already discarded) intervals just before.
const EMPTY_POLL_MEMORY: usize = 3;

/// Rolling record of the last few intervals' empty-poll deltas.
#[derive(Default)]
struct EmptyPollMemory([u64; EMPTY_POLL_MEMORY]);

impl EmptyPollMemory {
    fn push(&mut self, empty_polls: u64) {
        self.0.rotate_left(1);
        self.0[EMPTY_POLL_MEMORY - 1] = empty_polls;
    }

    fn recent(&self) -> u64 {
        self.0.iter().fold(0, |a, b| a.saturating_add(*b))
    }
}

/// Decides whether a merged window deserves a `loop_stall` record.
fn is_anomalous(s: &LoopSnapshot, d: &LateCounters) -> bool {
    s.starved > 0 || d.xruns > 0 || d.overruns > 0 || d.starvations > 0 || d.dropped_chunks > 0
}

/// Owns the `audigma-health` aggregator thread.
pub struct LoopTimingMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LoopTimingMonitor {
    /// `tile_budget_us` is the nominal audio duration of a full tile (for context only).
    pub fn spawn(
        telemetry: Arc<LoopTimingTelemetry>,
        counters: LateCounterSource,
        health_interval: Duration,
        tile_budget_us: u64,
        timings: Option<Arc<PoolTimings>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("audigma-health".to_string())
            .spawn(move || {
                let mut pending = LoopSnapshot::default();
                let mut health = LoopSnapshot::default();
                let mut last_counters = counters();
                let mut pending_delta = LateCounters::default();
                let mut empty_memory = EmptyPollMemory::default();
                let mut last_emit = Instant::now() - STALL_MIN_SPACING;
                let mut last_health = Instant::now();
                let mut tracker = timings.map(|t| TimingTracker::new(t, MaxSlot::Health));
                loop {
                    let stopping = stop_thread.load(Ordering::Acquire);
                    let snap = telemetry.take_interval();
                    let now_counters = counters();
                    let d = now_counters.delta(&last_counters);
                    last_counters = now_counters;
                    pending.merge(&snap);
                    health.merge(&snap);
                    pending_delta = add_counters(&pending_delta, &d);
                    empty_memory.push(d.empty_polls);

                    if is_anomalous(&pending, &pending_delta)
                        && (stopping || last_emit.elapsed() >= STALL_MIN_SPACING)
                    {
                        // Empty polls from just before this window still explain the stall.
                        pending_delta.empty_polls =
                            pending_delta.empty_polls.max(empty_memory.recent());
                        emit_stall(&pending, &pending_delta, tile_budget_us);
                        pending = LoopSnapshot::default();
                        pending_delta = LateCounters::default();
                        last_emit = Instant::now();
                    } else if !is_anomalous(&pending, &pending_delta) {
                        // Nothing worth reporting: keep the window from growing stale.
                        pending = LoopSnapshot::default();
                        pending_delta = LateCounters::default();
                    }

                    if last_health.elapsed() >= health_interval {
                        let report = tracker.as_mut().map(TimingTracker::window);
                        emit_health(&health, tile_budget_us, report.as_ref());
                        health = LoopSnapshot::default();
                        last_health = Instant::now();
                    }

                    if stopping {
                        break;
                    }
                    std::thread::sleep(LATE_POLL_INTERVAL);
                }
            })
            .ok();

        if handle.is_none() {
            tracing::warn!(
                target: audigma_log::LATE_TARGET,
                event = audigma_log::STREAMING_ERROR,
                error = "failed to spawn audigma-health thread",
                error_kind = "thread_spawn",
                "loop-timing aggregator unavailable"
            );
        }
        Self { stop, handle }
    }
}

impl Drop for LoopTimingMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn add_counters(a: &LateCounters, b: &LateCounters) -> LateCounters {
    LateCounters {
        xruns: a.xruns.saturating_add(b.xruns),
        overruns: a.overruns.saturating_add(b.overruns),
        starvations: a.starvations.saturating_add(b.starvations),
        empty_polls: a.empty_polls.saturating_add(b.empty_polls),
        dropped_chunks: a.dropped_chunks.saturating_add(b.dropped_chunks),
        processed_chunks: a.processed_chunks.saturating_add(b.processed_chunks),
    }
}

/// Best-effort explanation of a stall window, from stage maxima and backend counters.
///
/// - `input_empty`: the loop polled empty input (waiting out the grace window before idle fill)
///   while every stage it controls was fast, i.e. the loop was waiting, not slow. This is what
///   an input that ends or stops delivering looks like.
/// - `dsp_slow` / `read_slow` / `prep_slow`: the named stage alone exceeded the tile budget (the
///   slowest such stage wins). `emit_slow` additionally requires a starved emit, because the
///   emit stage blocks by design when the device buffer is full.
/// - `unknown`: nothing above explains it (for example a device xrun with fast stages).
pub fn classify_stall(s: &LoopSnapshot, d: &LateCounters, tile_budget_us: u64) -> &'static str {
    if tile_budget_us == 0 {
        return "unknown";
    }
    let stages = [
        ("dsp_slow", s.dsp_max_us),
        ("emit_slow", if s.starved > 0 { s.emit_max_us } else { 0 }),
        ("read_slow", s.read_max_us),
        ("prep_slow", s.prep_max_us),
    ];
    let slowest = stages
        .iter()
        .filter(|(_, us)| *us > tile_budget_us)
        .max_by_key(|(_, us)| *us);
    match slowest {
        Some((name, _)) => name,
        None if d.empty_polls > 0 => "input_empty",
        None => "unknown",
    }
}

fn emit_stall(s: &LoopSnapshot, d: &LateCounters, tile_budget_us: u64) {
    tracing::warn!(
        target: audigma_log::LATE_TARGET,
        event = audigma_log::LOOP_STALL,
        cause = classify_stall(s, d, tile_budget_us),
        tiles = s.tiles,
        starved_emits = s.starved,
        // `None` omits the field, so a genuine -1 stays distinguishable from "unavailable".
        headroom_min_us = s.headroom_min_us,
        buffered_min_us = s.buffered_min_us,
        feed_gap_max_us = s.feed_gap_max_us,
        read_max_us = s.read_max_us,
        prep_max_us = s.prep_max_us,
        dsp_max_us = s.dsp_max_us,
        emit_max_us = s.emit_max_us,
        tile_budget_us = tile_budget_us,
        xruns_delta = d.xruns,
        overruns_delta = d.overruns,
        starvations_delta = d.starvations,
        empty_polls_delta = d.empty_polls,
        dropped_chunks_delta = d.dropped_chunks,
        processed_chunks_delta = d.processed_chunks,
        "feed loop stall: device buffer may have run dry"
    );
}

fn emit_health(s: &LoopSnapshot, tile_budget_us: u64, timing: Option<&TimingReport>) {
    if s.tiles == 0 {
        return;
    }
    tracing::debug!(
        target: audigma_log::LATE_TARGET,
        event = audigma_log::LOOP_HEALTH,
        tiles = s.tiles,
        // `None` omits the field, so a genuine -1 stays distinguishable from "unavailable".
        headroom_min_us = s.headroom_min_us,
        buffered_min_us = s.buffered_min_us,
        feed_gap_max_us = s.feed_gap_max_us,
        read_max_us = s.read_max_us,
        prep_max_us = s.prep_max_us,
        dsp_max_us = s.dsp_max_us,
        emit_max_us = s.emit_max_us,
        tile_budget_us = tile_budget_us,
        // Worker execution (inside the worker) vs dispatch (wake-up + barrier, as seen by
        // the loop). Window percentiles have 25 us resolution; `*_cum_*` cover the stream.
        worker_exec_mean_us = timing.map(|t| t.worker.mean_us),
        worker_exec_p50_us = timing.map(|t| t.worker.p50_us),
        worker_exec_p99_us = timing.map(|t| t.worker.p99_us),
        worker_exec_max_us = timing.map(|t| t.worker.max_us),
        worker_exec_cum_p999_us = timing.map(|t| t.worker_cumulative.p999_us),
        worker_exec_cum_max_us = timing.map(|t| t.worker_cumulative.max_us),
        worker_p99_us = timing.map(|t| t.per_worker_list(|s| s.p99_us)),
        worker_max_us = timing.map(|t| t.per_worker_list(|s| s.max_us)),
        dispatch_mean_us = timing.map(|t| t.dispatch.mean_us),
        dispatch_p50_us = timing.map(|t| t.dispatch.p50_us),
        dispatch_p99_us = timing.map(|t| t.dispatch.p99_us),
        dispatch_cum_p999_us = timing.map(|t| t.dispatch_cumulative.p999_us),
        "feed loop health"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn empty_poll_memory_keeps_only_recent_intervals() {
        let mut m = EmptyPollMemory::default();
        assert_eq!(m.recent(), 0);
        m.push(100);
        m.push(0);
        m.push(0);
        assert_eq!(m.recent(), 100);
        m.push(0);
        assert_eq!(m.recent(), 0);
    }

    #[test]
    fn stall_cause_classification() {
        let budget = 1_451;
        let snap = |dsp, emit, read| LoopSnapshot {
            dsp_max_us: dsp,
            emit_max_us: emit,
            read_max_us: read,
            ..Default::default()
        };
        let polls = LateCounters {
            empty_polls: 226,
            ..Default::default()
        };
        let none = LateCounters::default();
        // The mpc-stop case: loop idle-waiting, every stage fast.
        assert_eq!(
            classify_stall(&snap(0, 73, 0), &polls, budget),
            "input_empty"
        );
        // A slow stage wins even if empty polls were seen.
        assert_eq!(
            classify_stall(&snap(3_000, 70, 0), &polls, budget),
            "dsp_slow"
        );
        let starved = |mut s: LoopSnapshot| {
            s.starved = 1;
            s
        };
        assert_eq!(
            classify_stall(&starved(snap(900, 5_000, 0)), &none, budget),
            "emit_slow"
        );
        assert_eq!(
            classify_stall(&starved(snap(2_000, 9_000, 0)), &none, budget),
            "emit_slow"
        );
        // A long blocking emit with no starvation is normal (full device buffer).
        assert_eq!(
            classify_stall(&snap(900, 5_000, 0), &none, budget),
            "unknown"
        );
        assert_eq!(
            classify_stall(&snap(900, 4_555, 0), &polls, budget),
            "input_empty"
        );
        assert_eq!(
            classify_stall(&snap(0, 0, 4_000), &none, budget),
            "read_slow"
        );
        // Fast stages and no empty input: xrun with no visible cause.
        assert_eq!(classify_stall(&snap(500, 70, 20), &none, budget), "unknown");
        // No budget information.
        assert_eq!(classify_stall(&snap(0, 0, 0), &polls, 0), "unknown");
    }

    use super::*;
    use std::io::Write;

    struct FakeOut(Option<u64>);
    impl Write for FakeOut {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl AudioOutput for FakeOut {
        fn buffered_us(&self) -> Option<u64> {
            self.0
        }
    }

    #[test]
    fn starved_when_gap_exceeds_buffered() {
        let t = Arc::new(LoopTimingTelemetry::new());
        let mut p = LoopProbe::new(Arc::clone(&t));
        let mut out = FakeOut(Some(1_000));
        p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        std::thread::sleep(Duration::from_millis(5));
        p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        let s = t.take_interval();
        assert_eq!(s.tiles, 2);
        assert_eq!(s.starved, 1);
        assert!(s.headroom_min_us.unwrap() < 0);
        assert_eq!(t.total_starved(), 1);
        assert_eq!(t.take_interval().headroom_min_us, None);
    }

    #[test]
    fn healthy_when_buffer_deep() {
        let t = Arc::new(LoopTimingTelemetry::new());
        let mut p = LoopProbe::new(Arc::clone(&t));
        let mut out = FakeOut(Some(500_000));
        for _ in 0..3 {
            p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        }
        let s = t.take_interval();
        assert_eq!(s.starved, 0);
        assert!(s.headroom_min_us.unwrap() > 0);
    }

    #[test]
    fn no_headroom_without_delay_and_reset_clears_baseline() {
        let t = Arc::new(LoopTimingTelemetry::new());
        let mut p = LoopProbe::new(Arc::clone(&t));
        let mut out = FakeOut(None);
        p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        assert_eq!(t.take_interval().headroom_min_us, None);
        p.reset_baseline();
        p.timed_emit(&mut out, |o| o.write_all(&[0]).unwrap());
        assert_eq!(t.take_interval().feed_gap_max_us, 0);
    }
}
