//! DSP deadline-miss telemetry and episode aggregation.
//!
//! The audio path only touches atomics through [`LateTileTelemetry`]. A dedicated
//! non-real-time thread (`audigma-late`) polls that telemetry, maintains the episode
//! state machine, and emits structured records through `tracing`.

use crate::timing::{MaxSlot, PoolTimings, TimingReport, TimingTracker};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

/// How often the `audigma-late` aggregator polls the atomics.
pub const LATE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Cumulative counter snapshot used to derive per-interval deltas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LateCounters {
    pub xruns: u64,
    pub overruns: u64,
    pub starvations: u64,
    pub empty_polls: u64,
    pub dropped_chunks: u64,
    pub processed_chunks: u64,
}

impl LateCounters {
    /// Returns `self - previous`, saturating at zero if a backend counter wrapped or reset.
    #[inline]
    pub fn delta(&self, previous: &Self) -> Self {
        Self {
            xruns: self.xruns.saturating_sub(previous.xruns),
            overruns: self.overruns.saturating_sub(previous.overruns),
            starvations: self.starvations.saturating_sub(previous.starvations),
            empty_polls: self.empty_polls.saturating_sub(previous.empty_polls),
            dropped_chunks: self.dropped_chunks.saturating_sub(previous.dropped_chunks),
            processed_chunks: self
                .processed_chunks
                .saturating_sub(previous.processed_chunks),
        }
    }
}

/// Details of the first late tile observed in an aggregation interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstLate {
    /// Monotonic timestamp since the telemetry origin, in nanoseconds.
    pub mono_ns: u64,
    /// Wall-clock duration of that tile in microseconds.
    pub elapsed_us: u64,
    /// Input buffer fill at that moment, in milliseconds.
    pub fill_ms: f64,
}

/// One interval-scoped telemetry snapshot.
#[derive(Debug, Clone, Copy, Default)]
pub struct IntervalSnapshot {
    pub late: u64,
    pub full: u64,
    pub partial: u64,
    pub max_us: u64,
    pub first_late: Option<FirstLate>,
    pub last_late_mono_ns: u64,
}

impl IntervalSnapshot {
    fn merge(&mut self, other: &Self) {
        self.late = self.late.saturating_add(other.late);
        self.full = self.full.saturating_add(other.full);
        self.partial = self.partial.saturating_add(other.partial);
        self.max_us = self.max_us.max(other.max_us);
        if self.first_late.is_none() {
            self.first_late = other.first_late;
        }
        self.last_late_mono_ns = self.last_late_mono_ns.max(other.last_late_mono_ns);
    }
}

/// Lock-free telemetry shared by the live audio loop and the `audigma-late` aggregator.
pub struct LateTileTelemetry {
    origin: Instant,
    tile_baseband_frames: usize,
    budget_us: u64,
    input_rate_hz: u32,
    output_rate_hz: u32,

    // Cumulative counters (lifetime of this pipeline instance).
    total_full_tiles: AtomicU64,
    total_late_tiles: AtomicU64,
    total_partial_tiles: AtomicU64,
    max_tile_us: AtomicU64,

    // Interval-scoped counters, reset by the aggregator at each boundary.
    interval_late_tiles: AtomicU64,
    interval_full_tiles: AtomicU64,
    interval_partial_tiles: AtomicU64,
    interval_max_us: AtomicU64,
    last_late_mono_ns: AtomicU64,

    // First-late capture uses a monotonically increasing seqlock so the aggregator
    // never observes a partially written tuple.
    first_late_seq: AtomicU64,
    first_late_mono_ns: AtomicU64,
    first_late_elapsed_us: AtomicU64,
    first_late_fill_ms_bits: AtomicU64,

    // Latest sampled input buffer fill, stored as f64 bits (NaN when unavailable).
    current_fill_ms_bits: AtomicU64,
}

impl LateTileTelemetry {
    /// Creates telemetry for a real-time live tile processor.
    pub fn new(
        tile_baseband_frames: usize,
        budget: Duration,
        input_rate_hz: u32,
        output_rate_hz: u32,
    ) -> Self {
        Self {
            origin: Instant::now(),
            tile_baseband_frames,
            budget_us: budget.as_micros() as u64,
            input_rate_hz,
            output_rate_hz,
            total_full_tiles: AtomicU64::new(0),
            total_late_tiles: AtomicU64::new(0),
            total_partial_tiles: AtomicU64::new(0),
            max_tile_us: AtomicU64::new(0),
            interval_late_tiles: AtomicU64::new(0),
            interval_full_tiles: AtomicU64::new(0),
            interval_partial_tiles: AtomicU64::new(0),
            interval_max_us: AtomicU64::new(0),
            last_late_mono_ns: AtomicU64::new(0),
            first_late_seq: AtomicU64::new(0),
            first_late_mono_ns: AtomicU64::new(0),
            first_late_elapsed_us: AtomicU64::new(0),
            first_late_fill_ms_bits: AtomicU64::new(f64::NAN.to_bits()),
            current_fill_ms_bits: AtomicU64::new(f64::NAN.to_bits()),
        }
    }

    /// Returns monotonic nanoseconds since this telemetry instance was created.
    ///
    /// This is the same clock already used for tile timing, so the audio path adds no
    /// new clock source or syscall family.
    #[inline]
    pub fn mono_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }

    #[inline]
    pub fn budget_us(&self) -> u64 {
        self.budget_us
    }

    #[inline]
    pub fn input_rate_hz(&self) -> u32 {
        self.input_rate_hz
    }

    #[inline]
    pub fn output_rate_hz(&self) -> u32 {
        self.output_rate_hz
    }

    #[inline]
    pub fn tile_baseband_frames(&self) -> usize {
        self.tile_baseband_frames
    }

    /// Records one processed tile. Called only from the live audio loop.
    ///
    /// `mono_ns` is read by the caller only for late tiles, keeping the common path to
    /// a few relaxed atomic operations.
    #[inline]
    pub fn record_tile(&self, is_full: bool, elapsed_us: u64, is_late: bool, mono_ns: Option<u64>) {
        self.max_tile_us.fetch_max(elapsed_us, Ordering::Relaxed);
        if is_full {
            self.total_full_tiles.fetch_add(1, Ordering::Relaxed);
            self.interval_full_tiles.fetch_add(1, Ordering::Relaxed);
            self.interval_max_us
                .fetch_max(elapsed_us, Ordering::Relaxed);

            if is_late {
                let previous = self.interval_late_tiles.fetch_add(1, Ordering::Relaxed);
                self.total_late_tiles.fetch_add(1, Ordering::Relaxed);
                let mono = mono_ns.unwrap_or_else(|| self.mono_ns());
                self.last_late_mono_ns.store(mono, Ordering::Relaxed);
                if previous == 0 {
                    self.write_first_late(mono, elapsed_us);
                }
            }
        } else {
            self.total_partial_tiles.fetch_add(1, Ordering::Relaxed);
            self.interval_partial_tiles.fetch_add(1, Ordering::Relaxed);
            self.interval_max_us
                .fetch_max(elapsed_us, Ordering::Relaxed);
        }
    }

    /// Updates the most recently sampled input buffer fill for late-tile context.
    #[inline]
    pub fn set_current_fill_ms(&self, fill_ms: Option<f64>) {
        let bits = fill_ms
            .filter(|v| v.is_finite())
            .map(f64::to_bits)
            .unwrap_or(f64::NAN.to_bits());
        self.current_fill_ms_bits.store(bits, Ordering::Relaxed);
    }

    #[inline]
    fn current_fill_ms(&self) -> Option<f64> {
        let value = f64::from_bits(self.current_fill_ms_bits.load(Ordering::Relaxed));
        value.is_finite().then_some(value)
    }

    #[inline]
    fn write_first_late(&self, mono_ns: u64, elapsed_us: u64) {
        let fill_ms = self.current_fill_ms().unwrap_or(0.0);
        // Seqlock: odd while the tuple is being written, even when complete.
        self.first_late_seq.fetch_add(1, Ordering::AcqRel);
        self.first_late_mono_ns.store(mono_ns, Ordering::Relaxed);
        self.first_late_elapsed_us
            .store(elapsed_us, Ordering::Relaxed);
        self.first_late_fill_ms_bits
            .store(fill_ms.to_bits(), Ordering::Relaxed);
        self.first_late_seq.fetch_add(1, Ordering::Release);
    }

    fn read_first_late(&self) -> Option<FirstLate> {
        // Bound the retry so a preempted audio thread cannot stall the aggregator.
        for _ in 0..1_000 {
            let start = self.first_late_seq.load(Ordering::Acquire);
            if start & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let mono_ns = self.first_late_mono_ns.load(Ordering::Relaxed);
            let elapsed_us = self.first_late_elapsed_us.load(Ordering::Relaxed);
            let fill_ms = f64::from_bits(self.first_late_fill_ms_bits.load(Ordering::Relaxed));
            let end = self.first_late_seq.load(Ordering::Acquire);
            if start == end {
                return Some(FirstLate {
                    mono_ns,
                    elapsed_us,
                    fill_ms,
                });
            }
            std::hint::spin_loop();
        }
        None
    }

    /// Takes and resets all interval-scoped counters.
    ///
    /// The cumulative counters are intentionally untouched so episode totals and
    /// status telemetry remain valid across boundaries.
    pub fn take_interval(&self) -> IntervalSnapshot {
        let first_before = self.read_first_late();

        let late = self.interval_late_tiles.swap(0, Ordering::AcqRel);
        let full = self.interval_full_tiles.swap(0, Ordering::AcqRel);
        let partial = self.interval_partial_tiles.swap(0, Ordering::AcqRel);
        let max_us = self.interval_max_us.swap(0, Ordering::AcqRel);
        let last_late_mono_ns = self.last_late_mono_ns.swap(0, Ordering::AcqRel);

        let first_late = if late > 0 {
            first_before.or_else(|| self.read_first_late())
        } else {
            None
        };

        IntervalSnapshot {
            late,
            full,
            partial,
            max_us,
            first_late,
            last_late_mono_ns,
        }
    }

    #[inline]
    pub fn total_late_tiles(&self) -> u64 {
        self.total_late_tiles.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn max_tile_us(&self) -> u64 {
        self.max_tile_us.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn total_tiles_processed(&self) -> u64 {
        self.total_full_tiles
            .load(Ordering::Relaxed)
            .saturating_add(self.total_partial_tiles.load(Ordering::Relaxed))
    }

    #[inline]
    pub fn total_full_tiles(&self) -> u64 {
        self.total_full_tiles.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn total_partial_tiles(&self) -> u64 {
        self.total_partial_tiles.load(Ordering::Relaxed)
    }
}

/// A structured deadline-miss record, rendered to `audigma::late` through `tracing`.
#[derive(Debug, Clone, PartialEq)]
pub enum LateRecord {
    Start {
        episode_id: u64,
        tile_us: u64,
        budget_us: u64,
        fill_ms: f64,
        input_rate_hz: u32,
        output_rate_hz: u32,
    },
    Slice {
        episode_id: u64,
        start: String,
        end: String,
        late: u64,
        total: u64,
        max_us: u64,
        budget_us: u64,
        fill_min_ms: f64,
        fill_max_ms: f64,
        partial_tiles: u64,
        counters_delta: LateCounters,
        /// Worker/dispatch timing over this slice, when the pool exposes histograms.
        /// Boxed to keep `LateRecord` small; records are built off the real-time path.
        timing: Option<Box<TimingReport>>,
    },
    End {
        episode_id: u64,
        start: String,
        end: String,
        late: u64,
        total: u64,
        max_us: u64,
        budget_us: u64,
        fill_min_ms: f64,
        fill_max_ms: f64,
        input_rate_hz: u32,
        output_rate_hz: u32,
    },
}

impl LateRecord {
    /// Emits this record through the process-wide structured logging pipeline.
    pub fn emit(&self) {
        match self {
            Self::Start {
                episode_id,
                tile_us,
                budget_us,
                fill_ms,
                input_rate_hz,
                output_rate_hz,
            } => {
                tracing::warn!(
                    target: audigma_log::LATE_TARGET,
                    event = audigma_log::LATE_START,
                    episode_id = *episode_id,
                    tile_us = *tile_us,
                    budget_us = *budget_us,
                    fill_ms = *fill_ms,
                    input_rate_hz = *input_rate_hz,
                    output_rate_hz = *output_rate_hz,
                    "dsp tile deadline exceeded"
                );
            }
            Self::Slice {
                episode_id,
                start,
                end,
                late,
                total,
                max_us,
                budget_us,
                fill_min_ms,
                fill_max_ms,
                partial_tiles,
                counters_delta,
                timing,
            } => {
                let timing = timing.as_ref();
                tracing::warn!(
                    target: audigma_log::LATE_TARGET,
                    event = audigma_log::LATE_SLICE,
                    episode_id = *episode_id,
                    start = %start,
                    end = %end,
                    late = *late,
                    total = *total,
                    max_us = *max_us,
                    budget_us = *budget_us,
                    fill_min_ms = *fill_min_ms,
                    fill_max_ms = *fill_max_ms,
                    partial_tiles = *partial_tiles,
                    xruns_delta = counters_delta.xruns,
                    overruns_delta = counters_delta.overruns,
                    starvations_delta = counters_delta.starvations,
                    empty_polls_delta = counters_delta.empty_polls,
                    dropped_chunks_delta = counters_delta.dropped_chunks,
                    processed_chunks_delta = counters_delta.processed_chunks,
                    worker_exec_mean_us = timing.map(|t| t.worker.mean_us),
                    worker_exec_p50_us = timing.map(|t| t.worker.p50_us),
                    worker_exec_p99_us = timing.map(|t| t.worker.p99_us),
                    worker_exec_max_us = timing.map(|t| t.worker.max_us),
                    worker_p99_us = timing.map(|t| t.per_worker_list(|s| s.p99_us)),
                    worker_max_us = timing.map(|t| t.per_worker_list(|s| s.max_us)),
                    dispatch_p50_us = timing.map(|t| t.dispatch.p50_us),
                    dispatch_p99_us = timing.map(|t| t.dispatch.p99_us),
                    dispatch_max_us = timing.map(|t| t.dispatch.max_us),
                    "dsp deadline miss interval slice"
                );
            }
            Self::End {
                episode_id,
                start,
                end,
                late,
                total,
                max_us,
                budget_us,
                fill_min_ms,
                fill_max_ms,
                input_rate_hz,
                output_rate_hz,
            } => {
                tracing::info!(
                    target: audigma_log::LATE_TARGET,
                    event = audigma_log::LATE_END,
                    episode_id = *episode_id,
                    start = %start,
                    end = %end,
                    late = *late,
                    total = *total,
                    max_us = *max_us,
                    budget_us = *budget_us,
                    fill_min_ms = *fill_min_ms,
                    fill_max_ms = *fill_max_ms,
                    input_rate_hz = *input_rate_hz,
                    output_rate_hz = *output_rate_hz,
                    "dsp deadline miss episode recovered"
                );
            }
        }
    }
}

/// Thread-safe provider for the latest cumulative backend counter snapshot.
pub type LateCounterSource = Arc<dyn Fn() -> LateCounters + Send + Sync>;

#[derive(Debug, Default)]
struct EpisodeTotals {
    late: u64,
    total: u64,
    max_us: u64,
    fill_min: Option<f64>,
    fill_max: Option<f64>,
    last_late_mono_ns: u64,
}

impl EpisodeTotals {
    fn add_interval(&mut self, snapshot: &IntervalSnapshot) {
        self.late = self.late.saturating_add(snapshot.late);
        self.total = self.total.saturating_add(snapshot.full);
        self.max_us = self.max_us.max(snapshot.max_us);
        self.last_late_mono_ns = self.last_late_mono_ns.max(snapshot.last_late_mono_ns);
    }

    fn sample_fill(&mut self, fill: f64) {
        self.fill_min = Some(self.fill_min.map_or(fill, |v| v.min(fill)));
        self.fill_max = Some(self.fill_max.map_or(fill, |v| v.max(fill)));
    }
}

#[derive(Debug)]
struct Episode {
    id: u64,
    start_mono_ns: u64,
    start_wall: SystemTime,
    totals: EpisodeTotals,
    pending: IntervalSnapshot,
    interval_fill_min: Option<f64>,
    interval_fill_max: Option<f64>,
}

impl Episode {
    fn sample_interval_fill(&mut self, fill: f64) {
        self.interval_fill_min = Some(self.interval_fill_min.map_or(fill, |v| v.min(fill)));
        self.interval_fill_max = Some(self.interval_fill_max.map_or(fill, |v| v.max(fill)));
    }

    fn reset_interval_fill(&mut self) {
        self.interval_fill_min = None;
        self.interval_fill_max = None;
    }
}

static NEXT_EPISODE_ID: AtomicU64 = AtomicU64::new(1);

#[inline]
fn allocate_episode_id() -> u64 {
    NEXT_EPISODE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Episode state machine driven by interval snapshots.
pub struct LateEpisodeAggregator {
    telemetry: Arc<LateTileTelemetry>,
    counters: LateCounterSource,
    interval_ns: u64,
    last_seen_late_total: u64,
    last_counters: LateCounters,
    next_boundary_ns: u64,
    episode: Option<Episode>,
    timing: Option<TimingTracker>,
}

impl LateEpisodeAggregator {
    pub fn new(
        telemetry: Arc<LateTileTelemetry>,
        counters: LateCounterSource,
        interval: Duration,
    ) -> Self {
        let interval_ns = interval.as_nanos().min(u64::MAX as u128) as u64;
        let last_counters = counters();
        Self {
            telemetry,
            counters,
            interval_ns: interval_ns.max(1),
            last_seen_late_total: 0,
            last_counters,
            next_boundary_ns: 0,
            episode: None,
            timing: None,
        }
    }

    /// Adds worker/dispatch timing to `late_slice` records.
    pub fn with_timings(mut self, timings: Arc<PoolTimings>) -> Self {
        self.timing = Some(TimingTracker::new(timings, MaxSlot::Late));
        self
    }

    /// Advances the state machine to `mono_now_ns` and returns records to emit.
    pub fn tick(&mut self, mono_now_ns: u64, wall_now: SystemTime) -> Vec<LateRecord> {
        let mut records = Vec::new();
        let total_late = self.telemetry.total_late_tiles();

        if self.episode.is_none() {
            if total_late > self.last_seen_late_total {
                self.open_episode(total_late, mono_now_ns, wall_now, &mut records);
            } else {
                // Stay ready for a new episode without letting interval counters grow
                // stale while the stream is meeting all deadlines.
                let _ = self.telemetry.take_interval();
                self.last_seen_late_total = total_late;
                return records;
            }
        }

        if let Some(fill) = self.telemetry.current_fill_ms() {
            if let Some(episode) = self.episode.as_mut() {
                episode.sample_interval_fill(fill);
                episode.totals.sample_fill(fill);
            }
        }

        if self.episode.is_some() && mono_now_ns >= self.next_boundary_ns {
            // Read the cumulative count before taking the interval snapshot. Any late
            // tile after the snapshot belongs to the next interval and must be able to
            // open a fresh episode.
            let late_before_snapshot = self.telemetry.total_late_tiles();
            let snapshot = self.telemetry.take_interval();
            let episode = self.episode.as_mut().expect("episode checked above");
            episode.pending.merge(&snapshot);

            if episode.pending.late == 0 {
                let episode = self.episode.take().expect("episode checked above");
                records.push(self.make_end(episode, mono_now_ns, wall_now));
                self.last_seen_late_total = late_before_snapshot;
                return records;
            }

            let counters = (self.counters)();
            let delta = counters.delta(&self.last_counters);
            self.last_counters = counters;

            episode.totals.add_interval(&episode.pending);
            let timing = self
                .timing
                .as_mut()
                .map(|tracker| Box::new(tracker.window()));
            records.push(Self::make_slice(
                self.telemetry.as_ref(),
                episode,
                mono_now_ns,
                wall_now,
                delta,
                timing,
            ));

            episode.pending = IntervalSnapshot::default();
            episode.reset_interval_fill();

            // If the aggregator was delayed past several boundaries, summarize the
            // accumulated activity in this slice and re-anchor to the next future
            // boundary. This keeps the heartbeat aligned without emitting a burst of
            // empty slices that would falsely close the episode.
            let skipped = (mono_now_ns - self.next_boundary_ns) / self.interval_ns;
            self.next_boundary_ns = self
                .next_boundary_ns
                .saturating_add(skipped.saturating_add(1).saturating_mul(self.interval_ns));
        }

        records
    }

    /// Flushes an open episode at shutdown. Emits one final cumulative `late_end`.
    pub fn flush(&mut self, mono_now_ns: u64, wall_now: SystemTime) -> Vec<LateRecord> {
        let Some(mut episode) = self.episode.take() else {
            return Vec::new();
        };

        let snapshot = self.telemetry.take_interval();
        episode.pending.merge(&snapshot);
        if episode.pending.late > 0 {
            episode.totals.add_interval(&episode.pending);
        }

        let record = self.make_end(episode, mono_now_ns, wall_now);
        self.last_seen_late_total = self.telemetry.total_late_tiles();
        vec![record]
    }

    fn open_episode(
        &mut self,
        total_late: u64,
        mono_now_ns: u64,
        wall_now: SystemTime,
        records: &mut Vec<LateRecord>,
    ) {
        let mut snapshot = self.telemetry.take_interval();
        let new_late = total_late.saturating_sub(self.last_seen_late_total);

        // The idle path resets interval counters every poll. If a late tile landed in
        // the reset window, recover its count from the cumulative counter.
        snapshot.late = snapshot.late.max(new_late).max(1);
        if snapshot.first_late.is_none() && new_late > 0 {
            snapshot.first_late = Some(FirstLate {
                mono_ns: mono_now_ns,
                elapsed_us: snapshot.max_us,
                fill_ms: self.telemetry.current_fill_ms().unwrap_or(0.0),
            });
        }
        snapshot.full = snapshot.full.max(snapshot.late);
        if let Some(first) = snapshot.first_late {
            snapshot.max_us = snapshot.max_us.max(first.elapsed_us);
            snapshot.last_late_mono_ns = snapshot.last_late_mono_ns.max(first.mono_ns);
        }

        let (start_mono_ns, tile_us, fill_ms) = match snapshot.first_late {
            Some(first) if first.mono_ns > 0 => (first.mono_ns, first.elapsed_us, first.fill_ms),
            Some(first) => (mono_now_ns, first.elapsed_us, first.fill_ms),
            None => (
                mono_now_ns,
                snapshot.max_us,
                self.telemetry.current_fill_ms().unwrap_or(0.0),
            ),
        };

        let start_wall = self.mono_to_wall(start_mono_ns, mono_now_ns, wall_now);
        let episode_id = allocate_episode_id();

        let start = LateRecord::Start {
            episode_id,
            tile_us,
            budget_us: self.telemetry.budget_us(),
            fill_ms,
            input_rate_hz: self.telemetry.input_rate_hz(),
            output_rate_hz: self.telemetry.output_rate_hz(),
        };
        records.push(start);

        let mut episode = Episode {
            id: episode_id,
            start_mono_ns,
            start_wall,
            totals: EpisodeTotals::default(),
            pending: snapshot,
            interval_fill_min: Some(fill_ms),
            interval_fill_max: Some(fill_ms),
        };
        episode.totals.sample_fill(fill_ms);
        episode.totals.last_late_mono_ns = snapshot.last_late_mono_ns;

        self.next_boundary_ns = start_mono_ns.saturating_add(self.interval_ns);
        self.last_counters = (self.counters)();
        self.last_seen_late_total = total_late;
        self.episode = Some(episode);
        if let Some(tracker) = self.timing.as_mut() {
            tracker.mark();
        }
    }

    fn make_slice(
        telemetry: &LateTileTelemetry,
        episode: &Episode,
        mono_now_ns: u64,
        wall_now: SystemTime,
        delta: LateCounters,
        timing: Option<Box<TimingReport>>,
    ) -> LateRecord {
        let end_mono = episode.pending.last_late_mono_ns.max(episode.start_mono_ns);
        LateRecord::Slice {
            episode_id: episode.id,
            start: format_rfc3339(episode.start_wall),
            end: format_rfc3339(mono_to_wall(end_mono, mono_now_ns, wall_now)),
            late: episode.pending.late,
            total: episode.pending.full,
            max_us: episode.pending.max_us,
            budget_us: telemetry.budget_us(),
            fill_min_ms: episode.interval_fill_min.unwrap_or(0.0),
            fill_max_ms: episode.interval_fill_max.unwrap_or(0.0),
            partial_tiles: episode.pending.partial,
            counters_delta: delta,
            timing,
        }
    }

    fn make_end(&self, episode: Episode, mono_now_ns: u64, wall_now: SystemTime) -> LateRecord {
        let end_mono = episode.totals.last_late_mono_ns.max(episode.start_mono_ns);
        LateRecord::End {
            episode_id: episode.id,
            start: format_rfc3339(episode.start_wall),
            end: format_rfc3339(mono_to_wall(end_mono, mono_now_ns, wall_now)),
            late: episode.totals.late,
            total: episode.totals.total,
            max_us: episode.totals.max_us,
            budget_us: self.telemetry.budget_us(),
            fill_min_ms: episode.totals.fill_min.unwrap_or(0.0),
            fill_max_ms: episode.totals.fill_max.unwrap_or(0.0),
            input_rate_hz: self.telemetry.input_rate_hz(),
            output_rate_hz: self.telemetry.output_rate_hz(),
        }
    }

    #[inline]
    fn mono_to_wall(
        &self,
        event_mono_ns: u64,
        mono_now_ns: u64,
        wall_now: SystemTime,
    ) -> SystemTime {
        mono_to_wall(event_mono_ns, mono_now_ns, wall_now)
    }
}

#[inline]
fn mono_to_wall(event_mono_ns: u64, mono_now_ns: u64, wall_now: SystemTime) -> SystemTime {
    if event_mono_ns <= mono_now_ns {
        wall_now - Duration::from_nanos(mono_now_ns - event_mono_ns)
    } else {
        wall_now + Duration::from_nanos(event_mono_ns - mono_now_ns)
    }
}

fn format_rfc3339(time: SystemTime) -> String {
    let dt = time::OffsetDateTime::from(time);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

/// Owns the background `audigma-late` aggregator thread.
pub struct LateTileMonitor {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LateTileMonitor {
    /// Spawns the non-real-time aggregator for one live pipeline instance.
    pub fn spawn(
        telemetry: Arc<LateTileTelemetry>,
        counters: LateCounterSource,
        interval: Duration,
        timings: Option<Arc<PoolTimings>>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("audigma-late".to_string())
            .spawn(move || {
                let mut aggregator =
                    LateEpisodeAggregator::new(Arc::clone(&telemetry), counters, interval);
                if let Some(timings) = timings {
                    aggregator = aggregator.with_timings(timings);
                }
                loop {
                    let mono_now = telemetry.mono_ns();
                    let wall_now = SystemTime::now();
                    for record in aggregator.tick(mono_now, wall_now) {
                        record.emit();
                    }

                    if stop_thread.load(Ordering::Acquire) {
                        let mono_now = telemetry.mono_ns();
                        let wall_now = SystemTime::now();
                        for record in aggregator.flush(mono_now, wall_now) {
                            record.emit();
                        }
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
                error = "failed to spawn audigma-late thread",
                error_kind = "thread_spawn",
                "late-tile aggregator unavailable"
            );
        }

        Self { stop, handle }
    }
}

impl Drop for LateTileMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::UNIX_EPOCH;

    const INTERVAL_MS: u64 = 10_000;
    const INTERVAL_NS: u64 = INTERVAL_MS * 1_000_000;
    const BUDGET_US: u64 = 1_451;

    fn test_telemetry() -> Arc<LateTileTelemetry> {
        Arc::new(LateTileTelemetry::new(
            256,
            Duration::from_micros(BUDGET_US),
            176_400,
            11_289_600,
        ))
    }

    fn test_counters() -> (LateCounterSource, Arc<Mutex<LateCounters>>) {
        let state = Arc::new(Mutex::new(LateCounters::default()));
        let source: LateCounterSource = {
            let state = Arc::clone(&state);
            Arc::new(move || *state.lock().unwrap())
        };
        (source, state)
    }

    fn aggregator(
        telemetry: Arc<LateTileTelemetry>,
    ) -> (LateEpisodeAggregator, Arc<Mutex<LateCounters>>) {
        let (source, state) = test_counters();
        (
            LateEpisodeAggregator::new(telemetry, source, Duration::from_millis(INTERVAL_MS)),
            state,
        )
    }

    fn wall_at(mono_ns: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_nanos(mono_ns)
    }

    fn record_full(telemetry: &LateTileTelemetry, mono_ns: u64, us: u64, late: bool) {
        telemetry.record_tile(true, us, late, Some(mono_ns));
    }

    fn record_partial(telemetry: &LateTileTelemetry, us: u64) {
        telemetry.record_tile(false, us, false, None);
    }

    #[test]
    fn no_late_tiles_emit_no_records() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        for i in 0..1_000u64 {
            record_full(&telemetry, i * 1_000_000, 500, false);
        }

        let mut all = Vec::new();
        for step in 1..=30u64 {
            all.extend(agg.tick(step * INTERVAL_NS / 10, wall_at(step * INTERVAL_NS / 10)));
        }
        assert!(all.is_empty());
        assert_eq!(telemetry.total_late_tiles(), 0);
    }

    #[test]
    fn isolated_late_tile_starts_slice_then_ends_after_quiet_interval() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 1_000_000u64;
        telemetry.set_current_fill_ms(Some(52.0));
        record_full(&telemetry, first_ns, 1_567, true);

        let start = agg.tick(first_ns + 50_000_000, wall_at(first_ns + 50_000_000));
        assert_eq!(start.len(), 1);
        assert!(matches!(
            &start[0],
            LateRecord::Start { tile_us, budget_us, fill_ms, .. }
                if *tile_us == 1_567 && *budget_us == BUDGET_US && *fill_ms == 52.0
        ));

        // The first interval contains the late tile, so it is reported as a slice.
        let at_first = first_ns + INTERVAL_NS;
        let records = agg.tick(at_first, wall_at(at_first));
        assert_eq!(records.len(), 1);
        match &records[0] {
            LateRecord::Slice {
                late,
                total,
                max_us,
                ..
            } => {
                assert_eq!(*late, 1);
                assert_eq!(*total, 1);
                assert_eq!(*max_us, 1_567);
            }
            other => panic!("expected slice, got {other:?}"),
        }

        // The following interval is quiet, closing the episode.
        let at_second = first_ns + 2 * INTERVAL_NS;
        let records = agg.tick(at_second, wall_at(at_second));
        assert_eq!(records.len(), 1);
        match &records[0] {
            LateRecord::End {
                late,
                total,
                max_us,
                ..
            } => {
                assert_eq!(*late, 1);
                assert_eq!(*total, 1);
                assert_eq!(*max_us, 1_567);
            }
            other => panic!("expected end, got {other:?}"),
        }
    }

    #[test]
    fn continuous_late_tiles_emit_one_slice_per_interval() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 5_000_000u64;
        let mut mono = first_ns;
        for interval in 0..5u64 {
            telemetry.set_current_fill_ms(Some(50.0 + interval as f64));
            record_full(&telemetry, mono, 1_500 + interval, true);
            record_full(&telemetry, mono + 1_000_000, 600, false);
            if interval == 0 {
                let records = agg.tick(mono + 10_000_000, wall_at(mono + 10_000_000));
                assert!(matches!(records[0], LateRecord::Start { .. }));
            }
            mono = first_ns + (interval + 1) * INTERVAL_NS;
            let records = agg.tick(mono, wall_at(mono));
            if interval < 4 {
                assert_eq!(records.len(), 1, "interval {interval}");
                assert!(matches!(records[0], LateRecord::Slice { .. }));
                // Seed the next interval with another late tile. The final interval
                // deliberately stays quiet so the episode closes at the next boundary.
                record_full(&telemetry, mono + 10_000, 1_600, true);
            }
        }

        // Next quiet interval closes the episode.
        mono += INTERVAL_NS;
        let records = agg.tick(mono, wall_at(mono));
        assert_eq!(records.len(), 1);
        assert!(matches!(records[0], LateRecord::End { .. }));
    }

    #[test]
    fn clean_interval_closes_episode_and_later_late_opens_new_id() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 2_000_000u64;
        record_full(&telemetry, first_ns, 1_600, true);
        let start = agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));
        let first_id = match &start[0] {
            LateRecord::Start { episode_id, .. } => *episode_id,
            other => panic!("expected start, got {other:?}"),
        };

        let at_first = first_ns + INTERVAL_NS;
        assert!(matches!(
            agg.tick(at_first, wall_at(at_first))[0],
            LateRecord::Slice { .. }
        ));
        let at_second = first_ns + 2 * INTERVAL_NS;
        let closed = agg.tick(at_second, wall_at(at_second));
        match &closed[0] {
            LateRecord::End { episode_id, .. } => assert_eq!(*episode_id, first_id),
            other => panic!("expected end, got {other:?}"),
        }

        let second_start = at_second + 1_000_000;
        record_full(&telemetry, second_start, 1_700, true);
        let records = agg.tick(second_start + 1_000_000, wall_at(second_start + 1_000_000));
        match &records[0] {
            LateRecord::Start { episode_id, .. } => {
                assert!(*episode_id > first_id, "episode ids must be monotonic");
            }
            other => panic!("expected start, got {other:?}"),
        }
    }

    #[test]
    fn slice_end_reflects_last_late_tile_not_interval_boundary() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 3_000_000u64;
        let last_ns = first_ns + 2_000_000_000;
        record_full(&telemetry, first_ns, 1_500, true);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));

        record_full(&telemetry, last_ns, 1_600, true);
        let boundary = first_ns + INTERVAL_NS;
        let records = agg.tick(boundary, wall_at(boundary));
        match &records[0] {
            LateRecord::Slice { start, end, .. } => {
                let expected_start = format_rfc3339(wall_at(first_ns));
                let expected_end = format_rfc3339(wall_at(last_ns));
                assert_eq!(start, &expected_start);
                assert_eq!(end, &expected_end);
            }
            other => panic!("expected slice, got {other:?}"),
        }
    }

    #[test]
    fn interval_counters_reset_and_cumulative_totals_are_correct() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 4_000_000u64;
        telemetry.set_current_fill_ms(Some(50.0));
        record_full(&telemetry, first_ns, 1_500, true);
        for i in 0..9u64 {
            record_full(&telemetry, first_ns + (i + 1) * 1_000, 500, false);
        }
        record_partial(&telemetry, 100);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));

        let at_first = first_ns + INTERVAL_NS;
        let slice = agg.tick(at_first, wall_at(at_first));
        match &slice[0] {
            LateRecord::Slice {
                late,
                total,
                partial_tiles,
                ..
            } => {
                assert_eq!(*late, 1);
                assert_eq!(*total, 10);
                assert_eq!(*partial_tiles, 1);
            }
            other => panic!("expected slice, got {other:?}"),
        }

        telemetry.set_current_fill_ms(Some(51.0));
        for _ in 0..5u64 {
            record_full(&telemetry, at_first + 1_000, 500, false);
        }
        record_full(&telemetry, at_first + 2_000, 1_600, true);

        let at_second = first_ns + 2 * INTERVAL_NS;
        let slice = agg.tick(at_second, wall_at(at_second));
        match &slice[0] {
            LateRecord::Slice {
                late,
                total,
                partial_tiles,
                ..
            } => {
                assert_eq!(*late, 1);
                assert_eq!(*total, 6);
                assert_eq!(*partial_tiles, 0);
            }
            other => panic!("expected slice, got {other:?}"),
        }

        let at_third = first_ns + 3 * INTERVAL_NS;
        let end = agg.tick(at_third, wall_at(at_third));
        match &end[0] {
            LateRecord::End {
                late,
                total,
                max_us,
                ..
            } => {
                assert_eq!(*late, 2);
                assert_eq!(*total, 16);
                assert_eq!(*max_us, 1_600);
            }
            other => panic!("expected end, got {other:?}"),
        }
    }

    #[test]
    fn first_late_capture_is_not_overwritten() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 6_000_000u64;
        telemetry.set_current_fill_ms(Some(48.0));
        record_full(&telemetry, first_ns, 1_500, true);
        telemetry.set_current_fill_ms(Some(60.0));
        record_full(&telemetry, first_ns + 1_000, 2_000, true);

        let start = agg.tick(first_ns + 2_000, wall_at(first_ns + 2_000));
        match &start[0] {
            LateRecord::Start {
                tile_us, fill_ms, ..
            } => {
                assert_eq!(*tile_us, 1_500);
                assert_eq!(*fill_ms, 48.0);
            }
            other => panic!("expected start, got {other:?}"),
        }
    }

    #[test]
    fn long_episode_produces_periodic_slices_indefinitely() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 7_000_000u64;
        record_full(&telemetry, first_ns, 1_500, true);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));

        let mut slices = 0;
        for interval in 1..=100u64 {
            record_full(&telemetry, first_ns + interval * 1_000_000, 1_600, true);
            let boundary = first_ns + interval * INTERVAL_NS;
            let records = agg.tick(boundary, wall_at(boundary));
            if matches!(records[0], LateRecord::Slice { .. }) {
                slices += 1;
            }
        }
        assert_eq!(slices, 100);
        assert!(agg.episode.is_some(), "episode must remain open");
    }

    #[test]
    fn delayed_poll_summarizes_elapsed_intervals_without_closing_episode() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 10_000_000u64;
        record_full(&telemetry, first_ns, 1_500, true);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));

        // A late tile in the middle of the stalled span.
        record_full(&telemetry, first_ns + 1_500_000_000, 1_600, true);

        // The poller wakes two and a half intervals later. All accumulated activity is
        // summarized in one slice instead of emitting empty follow-up slices that would
        // falsely close the episode.
        let delayed = first_ns + 2 * INTERVAL_NS + INTERVAL_NS / 2;
        let records = agg.tick(delayed, wall_at(delayed));
        assert_eq!(records.len(), 1);
        match &records[0] {
            LateRecord::Slice { late, max_us, .. } => {
                assert_eq!(*late, 2);
                assert_eq!(*max_us, 1_600);
            }
            other => panic!("expected slice, got {other:?}"),
        }
        assert!(agg.episode.is_some());

        // The next quiet boundary closes the episode normally.
        let at_third = first_ns + 3 * INTERVAL_NS;
        let records = agg.tick(at_third, wall_at(at_third));
        assert!(matches!(records[0], LateRecord::End { .. }));
    }

    #[test]
    fn shutdown_flush_emits_open_episode_summary() {
        let telemetry = test_telemetry();
        let (mut agg, _) = aggregator(Arc::clone(&telemetry));

        let first_ns = 8_000_000u64;
        record_full(&telemetry, first_ns, 1_500, true);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));
        record_full(&telemetry, first_ns + 2_000, 1_700, true);

        let records = agg.flush(first_ns + 3_000, wall_at(first_ns + 3_000));
        assert_eq!(records.len(), 1);
        match &records[0] {
            LateRecord::End {
                late,
                total,
                max_us,
                ..
            } => {
                assert_eq!(*late, 2);
                assert_eq!(*total, 2);
                assert_eq!(*max_us, 1_700);
            }
            other => panic!("expected end, got {other:?}"),
        }
    }

    #[test]
    fn counter_deltas_are_interval_scoped() {
        let telemetry = test_telemetry();
        let (source, state) = test_counters();
        let mut agg = LateEpisodeAggregator::new(
            Arc::clone(&telemetry),
            source,
            Duration::from_millis(INTERVAL_MS),
        );

        *state.lock().unwrap() = LateCounters {
            xruns: 1,
            overruns: 2,
            starvations: 3,
            empty_polls: 4,
            dropped_chunks: 5,
            processed_chunks: 6,
        };

        let first_ns = 9_000_000u64;
        record_full(&telemetry, first_ns, 1_500, true);
        agg.tick(first_ns + 1_000, wall_at(first_ns + 1_000));

        *state.lock().unwrap() = LateCounters {
            xruns: 2,
            overruns: 2,
            starvations: 5,
            empty_polls: 9,
            dropped_chunks: 5,
            processed_chunks: 20,
        };

        let boundary = first_ns + INTERVAL_NS;
        let records = agg.tick(boundary, wall_at(boundary));
        match &records[0] {
            LateRecord::Slice { counters_delta, .. } => {
                assert_eq!(
                    *counters_delta,
                    LateCounters {
                        xruns: 1,
                        overruns: 0,
                        starvations: 2,
                        empty_polls: 5,
                        dropped_chunks: 0,
                        processed_chunks: 14,
                    }
                );
            }
            other => panic!("expected slice, got {other:?}"),
        }
    }

    #[test]
    fn rfc3339_format_has_millisecond_precision() {
        let wall = UNIX_EPOCH + Duration::from_millis(1_772_022_896_250);
        assert_eq!(format_rfc3339(wall), "2026-02-25T12:34:56.250Z");
    }
}
