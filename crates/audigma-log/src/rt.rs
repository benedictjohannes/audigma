//! Real-time safe logging bridge and event definitions (`StructuredLogging.md §8`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Default capacity for low-frequency RT control event ring buffers.
pub const DEFAULT_RT_QUEUE_CAPACITY: usize = 256;

/// Fixed-size, allocation-free event queued by RT paths (§8.2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RtLogEvent {
    FormatNegotiated {
        is_dsd: bool,
        rate_hz: u32,
        bit_rate_hz: u32,
        intermediate_rate_hz: u32,
        channels: u8,
        interleave: i32,
    },
    VolumeEvent {
        volume_percent: Option<f32>,
        volume_db: Option<f32>,
        muted: Option<bool>,
    },
    DecimationError {
        channel: u8,
        code: i32,
        at_ns: u64,
    },
}

/// Error returned when pushing an event to a full RT log queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("RT log queue is full")]
pub struct RtQueueFullError;

/// RT-safe producer handle (§8.3).
pub struct RtLogProducer {
    producer: rtrb::Producer<RtLogEvent>,
    dropped_events: Arc<AtomicU64>,
}

impl RtLogProducer {
    /// Attempts to push an event into the ring buffer without blocking or allocating.
    /// Increments `dropped_events` on buffer full.
    #[inline]
    pub fn try_push(&mut self, event: RtLogEvent) -> Result<(), RtQueueFullError> {
        if self.producer.push(event).is_err() {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            Err(RtQueueFullError)
        } else {
            Ok(())
        }
    }

    /// Number of events dropped due to buffer overflow.
    #[inline]
    pub fn dropped_count(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

/// Consumer handle used by the drainer thread.
pub struct RtLogConsumer {
    consumer: rtrb::Consumer<RtLogEvent>,
}

impl RtLogConsumer {
    #[inline]
    pub fn pop(&mut self) -> Result<RtLogEvent, rtrb::PopError> {
        self.consumer.pop()
    }
}

/// Creates a new RT log channel pair with the given bounded capacity.
pub fn rt_log_channel(capacity: usize) -> (RtLogProducer, RtLogConsumer, Arc<AtomicU64>) {
    let (p, c) = rtrb::RingBuffer::new(capacity);
    let dropped = Arc::new(AtomicU64::new(0));
    let producer = RtLogProducer {
        producer: p,
        dropped_events: Arc::clone(&dropped),
    };
    let consumer = RtLogConsumer { consumer: c };
    (producer, consumer, dropped)
}

/// Lock-free error accumulator for high-rate per-buffer audio loop errors (§8.3).
#[derive(Debug, Default)]
pub struct DecimationErrorRecorder {
    count: AtomicU64,
    last_code: AtomicI32,
    last_channel: AtomicU8,
}

impl DecimationErrorRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// RT-safe record method.
    #[inline]
    pub fn record(&self, channel: u8, code: i32) {
        self.last_channel.store(channel, Ordering::Relaxed);
        self.last_code.store(code, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Non-RT snapshot and reset.
    pub fn snapshot_and_reset(&self) -> (u64, i32, u8) {
        let count = self.count.swap(0, Ordering::Relaxed);
        let code = self.last_code.load(Ordering::Relaxed);
        let channel = self.last_channel.load(Ordering::Relaxed);
        (count, code, channel)
    }
}

/// Drains RT queues and emits `tracing` records from a non-RT thread (`StructuredLogging.md §8.4`).
pub struct RtLogDrainer {
    consumers: Vec<RtLogConsumer>,
    dropped_counters: Vec<Arc<AtomicU64>>,
    decimation_recorders: Vec<Arc<DecimationErrorRecorder>>,
}

impl RtLogDrainer {
    pub fn new() -> Self {
        Self {
            consumers: Vec::new(),
            dropped_counters: Vec::new(),
            decimation_recorders: Vec::new(),
        }
    }

    pub fn add_channel(
        &mut self,
        consumer: RtLogConsumer,
        dropped_counter: Arc<AtomicU64>,
    ) {
        self.consumers.push(consumer);
        self.dropped_counters.push(dropped_counter);
    }

    pub fn add_decimation_recorder(&mut self, recorder: Arc<DecimationErrorRecorder>) {
        self.decimation_recorders.push(recorder);
    }

    /// Spawns the drainer on a normal-priority background thread named `audigma-rt-log`.
    pub fn spawn(self) -> RtLogDrainerHandle {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        let dispatch = tracing::dispatcher::get_default(|d| d.clone());

        let thread_handle = std::thread::Builder::new()
            .name("audigma-rt-log".to_string())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    let mut drainer = self;
                    let mut last_dropped_total = 0u64;
                    let mut last_summary_time = Instant::now();

                    while running_clone.load(Ordering::Relaxed) {
                        drainer.drain_all();

                        // Check dropped RT events
                        let total_dropped: u64 = drainer
                            .dropped_counters
                            .iter()
                            .map(|c| c.load(Ordering::Relaxed))
                            .sum();
                        if total_dropped > last_dropped_total {
                            let delta = total_dropped - last_dropped_total;
                            tracing::warn!(
                                target: "audigma::rt",
                                event = crate::events::RT_EVENTS_DROPPED,
                                dropped_count = delta,
                                total_dropped,
                                "RT logging queue overflow; events dropped"
                            );
                            last_dropped_total = total_dropped;
                        }

                        // Check decimation error recorders (every 5 seconds)
                        let now = Instant::now();
                        if now.duration_since(last_summary_time) >= Duration::from_secs(5) {
                            let window_s = now.duration_since(last_summary_time).as_secs();
                            drainer.emit_decimation_summaries(window_s);
                            last_summary_time = now;
                        }

                        std::thread::sleep(Duration::from_millis(50));
                    }

                    // Final drain before exiting thread
                    drainer.drain_all();
                    let window_s = Instant::now().duration_since(last_summary_time).as_secs();
                    drainer.emit_decimation_summaries(window_s.max(1));
                });
            })
            .expect("failed to spawn audigma-rt-log thread");

        RtLogDrainerHandle {
            running,
            handle: Some(thread_handle),
        }
    }

    fn drain_all(&mut self) {
        for c in &mut self.consumers {
            while let Ok(event) = c.pop() {
                emit_rt_event(event);
            }
        }
    }

    fn emit_decimation_summaries(&self, window_s: u64) {
        for r in &self.decimation_recorders {
            let (count, code, channel) = r.snapshot_and_reset();
            if count > 0 {
                tracing::warn!(
                    target: "audigma::pw::input",
                    event = crate::events::DECIMATION_ERROR_SUMMARY,
                    count,
                    last_code = code,
                    channel,
                    window_s,
                    "PipeWire decimation error summary"
                );
            }
        }
    }
}

impl Default for RtLogDrainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts an `RtLogEvent` to a `tracing` structured event.
pub fn emit_rt_event(event: RtLogEvent) {
    match event {
        RtLogEvent::FormatNegotiated {
            is_dsd,
            rate_hz,
            bit_rate_hz,
            intermediate_rate_hz,
            channels,
            interleave,
        } => {
            tracing::info!(
                target: "audigma::pw::input",
                event = crate::events::PW_FORMAT_NEGOTIATED,
                is_dsd,
                rate_hz,
                bit_rate_hz,
                intermediate_rate_hz,
                channels,
                interleave,
                "PipeWire format negotiated"
            );
        }
        RtLogEvent::VolumeEvent {
            volume_percent,
            volume_db,
            muted,
        } => {
            tracing::debug!(
                target: "audigma::pw::input",
                event = crate::events::PW_VOLUME_EVENT,
                volume_percent,
                volume_db,
                muted,
                "PipeWire volume change event"
            );
        }
        RtLogEvent::DecimationError {
            channel,
            code,
            at_ns,
        } => {
            tracing::warn!(
                target: "audigma::pw::input",
                event = "decimation_error",
                channel,
                code,
                at_ns,
                "PipeWire decimation error"
            );
        }
    }
}

/// Handle managing the lifetime of the `audigma-rt-log` drainer thread.
pub struct RtLogDrainerHandle {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl RtLogDrainerHandle {
    /// Signals the drainer to stop, performs final drain, and joins the thread.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RtLogDrainerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
