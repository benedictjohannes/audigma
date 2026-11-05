//! Fixed-bucket timing histograms for the channel worker pool.
//!
//! The audio path (workers and the dispatching loop thread) only performs relaxed atomic
//! increments into pre-allocated, cache-line-aligned histograms. Readers (`audigma-health`,
//! `audigma-late`) never reset the counts: each keeps its own previous snapshot and works on
//! deltas, so a windowed view and a cumulative view are both available without contention.
//!
//! Two metrics are kept apart on purpose:
//! - **worker execution**: time inside the worker, from the end of its wake-up to the end of
//!   its command (the Design §8 metric);
//! - **dispatch**: the whole `dispatch_command` as seen by the loop (wake-up + barrier).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Linear bucket width in microseconds.
pub const BUCKET_US: u64 = 25;
/// Linear buckets cover `0..LINEAR_BUCKETS * BUCKET_US` (6.4 ms, above the 5.8 ms tile budget at
/// 44.1 kHz); one more bucket collects overflow.
pub const LINEAR_BUCKETS: usize = 256;
pub const BUCKETS: usize = LINEAR_BUCKETS + 1;

/// Independent readers of the interval maximum. Each swaps only its own slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxSlot {
    Health = 0,
    Late = 1,
}

/// One histogram; own cache lines so workers never share a line with each other.
#[repr(align(64))]
pub struct TimingHistogram {
    counts: [AtomicU64; BUCKETS],
    sum_ns: AtomicU64,
    interval_max_ns: [AtomicU64; 2],
}

impl Default for TimingHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl TimingHistogram {
    pub fn new() -> Self {
        Self {
            counts: std::array::from_fn(|_| AtomicU64::new(0)),
            sum_ns: AtomicU64::new(0),
            interval_max_ns: [AtomicU64::new(0), AtomicU64::new(0)],
        }
    }

    /// Records one sample. No allocation, no locks.
    #[inline]
    pub fn record_ns(&self, ns: u64) {
        let bucket = ((ns / 1_000 / BUCKET_US) as usize).min(LINEAR_BUCKETS);
        self.counts[bucket].fetch_add(1, Ordering::Relaxed);
        self.sum_ns.fetch_add(ns, Ordering::Relaxed);
        self.interval_max_ns[0].fetch_max(ns, Ordering::Relaxed);
        self.interval_max_ns[1].fetch_max(ns, Ordering::Relaxed);
    }

    /// Reads the cumulative counts; takes and resets this reader's interval maximum.
    pub fn read(&self, slot: MaxSlot) -> HistSnapshot {
        let mut counts = [0u64; BUCKETS];
        for (dst, src) in counts.iter_mut().zip(&self.counts) {
            *dst = src.load(Ordering::Relaxed);
        }
        HistSnapshot {
            counts,
            sum_ns: self.sum_ns.load(Ordering::Relaxed),
            max_ns: self.interval_max_ns[slot as usize].swap(0, Ordering::Relaxed),
        }
    }
}

/// Point-in-time copy of a histogram (cumulative counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistSnapshot {
    counts: [u64; BUCKETS],
    sum_ns: u64,
    /// Maximum since this reader's previous read (not part of cumulative arithmetic).
    max_ns: u64,
}

impl Default for HistSnapshot {
    fn default() -> Self {
        Self {
            counts: [0; BUCKETS],
            sum_ns: 0,
            max_ns: 0,
        }
    }
}

impl HistSnapshot {
    /// `self - earlier`, keeping `self.max_ns` as the window maximum.
    fn delta(&self, earlier: &Self) -> Self {
        let mut counts = [0u64; BUCKETS];
        for ((d, a), b) in counts.iter_mut().zip(&self.counts).zip(&earlier.counts) {
            *d = a.saturating_sub(*b);
        }
        Self {
            counts,
            sum_ns: self.sum_ns.saturating_sub(earlier.sum_ns),
            max_ns: self.max_ns,
        }
    }

    fn add(&mut self, other: &Self) {
        for (a, b) in self.counts.iter_mut().zip(&other.counts) {
            *a += *b;
        }
        self.sum_ns += other.sum_ns;
        self.max_ns = self.max_ns.max(other.max_ns);
    }

    pub fn count(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// Smallest bucket upper edge covering fraction `q` of the samples, in microseconds.
    /// Capped by `max_us` (the exact maximum), which also resolves the overflow bucket.
    /// `None` when empty.
    fn quantile_us(&self, q: f64, max_us: u64) -> Option<u64> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let target = ((total as f64) * q).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (i, c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= target {
                if i >= LINEAR_BUCKETS {
                    return Some(max_us);
                }
                return Some(((i as u64 + 1) * BUCKET_US).min(max_us.max(1)));
            }
        }
        Some(max_us)
    }

    fn stats(&self, max_us: u64) -> StageStats {
        let count = self.count();
        StageStats {
            count,
            mean_us: if count == 0 {
                0
            } else {
                self.sum_ns / count / 1_000
            },
            p50_us: self.quantile_us(0.50, max_us).unwrap_or(0),
            p99_us: self.quantile_us(0.99, max_us).unwrap_or(0),
            p999_us: self.quantile_us(0.999, max_us).unwrap_or(0),
            max_us,
        }
    }
}

/// Summary of one stage over a window (or cumulatively). Percentiles are bucket upper
/// edges (25 us resolution) capped by the exact maximum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageStats {
    pub count: u64,
    pub mean_us: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub max_us: u64,
}

/// Histograms owned by a [`ChannelWorkerPool`](crate::worker::ChannelWorkerPool).
pub struct PoolTimings {
    workers: Vec<TimingHistogram>,
    dispatch: TimingHistogram,
}

impl PoolTimings {
    pub fn new(workers: usize) -> Arc<Self> {
        Arc::new(Self {
            workers: (0..workers).map(|_| TimingHistogram::new()).collect(),
            dispatch: TimingHistogram::new(),
        })
    }

    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    #[inline]
    pub fn record_worker(&self, worker: usize, ns: u64) {
        self.workers[worker].record_ns(ns);
    }

    #[inline]
    pub fn record_dispatch(&self, ns: u64) {
        self.dispatch.record_ns(ns);
    }

    fn read(&self, slot: MaxSlot) -> PoolSnapshot {
        PoolSnapshot {
            workers: self.workers.iter().map(|h| h.read(slot)).collect(),
            dispatch: self.dispatch.read(slot),
        }
    }
}

#[derive(Clone)]
struct PoolSnapshot {
    workers: Vec<HistSnapshot>,
    dispatch: HistSnapshot,
}

/// Window + cumulative view for one report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimingReport {
    /// All workers merged, window.
    pub worker: StageStats,
    /// Per worker, window.
    pub per_worker: Vec<StageStats>,
    pub dispatch: StageStats,
    /// All workers merged, since the tracker was created (p99.9 needs more than a window).
    pub worker_cumulative: StageStats,
    pub dispatch_cumulative: StageStats,
}

impl TimingReport {
    /// `"a,b,..."` of a per-worker field, for structured logs with a static field set.
    pub fn per_worker_list(&self, f: impl Fn(&StageStats) -> u64) -> String {
        self.per_worker
            .iter()
            .map(|s| f(s).to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Per-reader state: advances windows and keeps the baseline for cumulative figures.
pub struct TimingTracker {
    timings: Arc<PoolTimings>,
    slot: MaxSlot,
    baseline: PoolSnapshot,
    last: PoolSnapshot,
    cum_worker_max_ns: u64,
    cum_dispatch_max_ns: u64,
}

impl TimingTracker {
    pub fn new(timings: Arc<PoolTimings>, slot: MaxSlot) -> Self {
        let baseline = timings.read(slot);
        let last = baseline.clone();
        Self {
            timings,
            slot,
            baseline,
            last,
            cum_worker_max_ns: 0,
            cum_dispatch_max_ns: 0,
        }
    }

    /// Starts a new window now, discarding what happened since the previous one.
    pub fn mark(&mut self) {
        self.last = self.timings.read(self.slot);
    }

    /// Closes the current window and starts the next one.
    pub fn window(&mut self) -> TimingReport {
        let cur = self.timings.read(self.slot);

        let mut worker_win = HistSnapshot::default();
        let mut worker_cum = HistSnapshot::default();
        let mut per_worker = Vec::with_capacity(cur.workers.len());
        for ((c, l), b) in cur
            .workers
            .iter()
            .zip(&self.last.workers)
            .zip(&self.baseline.workers)
        {
            let win = c.delta(l);
            per_worker.push(win.stats(us_ceil(win.max_ns)));
            worker_win.add(&win);
            worker_cum.add(&c.delta(b));
        }
        let dispatch_win = cur.dispatch.delta(&self.last.dispatch);
        let dispatch_cum = cur.dispatch.delta(&self.baseline.dispatch);

        self.cum_worker_max_ns = self.cum_worker_max_ns.max(worker_win.max_ns);
        self.cum_dispatch_max_ns = self.cum_dispatch_max_ns.max(dispatch_win.max_ns);

        let report = TimingReport {
            worker: worker_win.stats(us_ceil(worker_win.max_ns)),
            per_worker,
            dispatch: dispatch_win.stats(us_ceil(dispatch_win.max_ns)),
            worker_cumulative: worker_cum.stats(us_ceil(self.cum_worker_max_ns)),
            dispatch_cumulative: dispatch_cum.stats(us_ceil(self.cum_dispatch_max_ns)),
        };
        self.last = cur;
        report
    }
}

#[inline]
fn us_ceil(ns: u64) -> u64 {
    ns.div_ceil(1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_use_bucket_edges_capped_by_max() {
        let h = TimingHistogram::new();
        for _ in 0..990 {
            h.record_ns(100_000); // 100 us -> bucket 4, edge 125
        }
        for _ in 0..10 {
            h.record_ns(1_000_000); // 1000 us -> bucket 40, edge 1025, capped to max 1000
        }
        let s = h.read(MaxSlot::Health);
        let st = s.stats(us_ceil(s.max_ns));
        assert_eq!(st.count, 1000);
        assert_eq!(st.p50_us, 125);
        assert_eq!(st.p99_us, 125);
        assert_eq!(st.p999_us, 1000);
        assert_eq!(st.max_us, 1000);
        assert_eq!(st.mean_us, (990 * 100 + 10 * 1000) / 1000);
    }

    #[test]
    fn overflow_bucket_reports_exact_max() {
        let h = TimingHistogram::new();
        h.record_ns(10_000_000);
        let s = h.read(MaxSlot::Late);
        let st = s.stats(us_ceil(s.max_ns));
        assert_eq!(st.p99_us, 10_000);
    }

    #[test]
    fn readers_have_independent_interval_max() {
        let h = TimingHistogram::new();
        h.record_ns(500_000);
        assert_eq!(h.read(MaxSlot::Health).max_ns, 500_000);
        assert_eq!(h.read(MaxSlot::Health).max_ns, 0);
        assert_eq!(h.read(MaxSlot::Late).max_ns, 500_000);
    }

    #[test]
    fn tracker_windows_and_cumulative() {
        let t = PoolTimings::new(2);
        t.record_worker(0, 50_000);
        t.record_dispatch(80_000);
        let mut tr = TimingTracker::new(Arc::clone(&t), MaxSlot::Health);
        for _ in 0..4 {
            t.record_worker(0, 100_000);
            t.record_worker(1, 200_000);
            t.record_dispatch(250_000);
        }
        let r = tr.window();
        assert_eq!(r.worker.count, 8);
        assert_eq!(r.per_worker[0].count, 4);
        assert_eq!(r.per_worker[1].max_us, 200);
        assert_eq!(r.dispatch.count, 4);
        assert_eq!(r.worker_cumulative.count, 8);

        t.record_worker(0, 100_000);
        t.record_worker(1, 100_000);
        t.record_dispatch(100_000);
        let r = tr.window();
        assert_eq!(r.worker.count, 2);
        assert_eq!(r.worker.max_us, 100);
        assert_eq!(r.worker_cumulative.count, 10);
        assert_eq!(r.worker_cumulative.max_us, 200);
        assert_eq!(r.per_worker_list(|s| s.count), "1,1");
    }

    #[test]
    fn empty_window_is_zero() {
        let t = PoolTimings::new(1);
        let mut tr = TimingTracker::new(t, MaxSlot::Late);
        let r = tr.window();
        assert_eq!(r.worker, StageStats::default());
        assert_eq!(r.dispatch.p99_us, 0);
    }
}
