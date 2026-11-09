//! Integration coverage for `audigma::late` structured JSON output.

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use audigma_engine::late::{
    HeadroomSource, HeadroomWindow, LateCounters, LateEpisodeAggregator, LateRecord,
    LateTileTelemetry,
};
use tracing_subscriber::layer::SubscriberExt;

#[derive(Clone, Default)]
struct TestBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for TestBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestBuffer {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn synthetic_telemetry_emits_valid_json_records() {
    let buf = TestBuffer::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_span_list(false)
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_thread_names(true)
            .with_thread_ids(false)
            .with_target(true),
    );

    let telemetry = Arc::new(LateTileTelemetry::new(
        256,
        Duration::from_micros(1_451),
        176_400,
        11_289_600,
    ));
    let counters: Arc<dyn Fn() -> LateCounters + Send + Sync> = Arc::new(LateCounters::default);
    let interval = Duration::from_millis(10_000);
    let mut aggregator = LateEpisodeAggregator::new(Arc::clone(&telemetry), counters, interval);

    let first_ns = 1_000_000u64;
    telemetry.set_current_fill_ms(Some(52.0));
    telemetry.record_tile(true, 1_567, true, Some(first_ns));

    let mut records = aggregator.tick(
        first_ns + 1_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 1_000),
    );
    records.extend(aggregator.tick(
        first_ns + 10_000_000_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 10_000_000_000),
    ));
    records.extend(aggregator.tick(
        first_ns + 20_000_000_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 20_000_000_000),
    ));

    tracing::subscriber::with_default(subscriber, || {
        for record in &records {
            record.emit();
        }
    });

    let raw = buf.0.lock().unwrap().clone();
    let output = String::from_utf8(raw).unwrap();
    let lines: Vec<&str> = output.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines.len(),
        3,
        "expected start, slice and end records: {output}"
    );

    let start: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(start["target"], "audigma::late");
    assert_eq!(start["level"], "DEBUG");
    assert_eq!(start["event"], "late_start");
    assert_eq!(start["tile_us"], 1_567);
    assert_eq!(start["budget_us"], 1_451);
    assert_eq!(start["fill_ms"], 52.0);
    assert_eq!(start["input_rate_hz"], 176_400);
    assert_eq!(start["output_rate_hz"], 11_289_600);
    assert!(start["episode_id"].is_u64());

    let slice: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(slice["target"], "audigma::late");
    assert_eq!(slice["level"], "WARN");
    assert_eq!(slice["event"], "late_slice");
    assert_eq!(slice["late"], 1);
    assert_eq!(slice["total"], 1);
    assert_eq!(slice["max_us"], 1_567);
    assert!(slice["start"].as_str().unwrap().ends_with('Z'));
    assert!(slice["end"].as_str().unwrap().ends_with('Z'));
    assert_eq!(slice["partial_tiles"], 0);
    assert_eq!(slice["xruns_delta"], 0);
    assert_eq!(slice["processed_chunks_delta"], 0);

    let end: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(end["target"], "audigma::late");
    assert_eq!(end["level"], "INFO");
    assert_eq!(end["event"], "late_end");
    assert_eq!(end["late"], 1);
    assert_eq!(end["total"], 1);
    assert_eq!(end["max_us"], 1_567);
    assert!(end["timestamp"].is_string());
}

#[test]
fn records_group_under_one_episode_id() {
    let telemetry = Arc::new(LateTileTelemetry::new(
        256,
        Duration::from_micros(1_451),
        176_400,
        11_289_600,
    ));
    let counters: Arc<dyn Fn() -> LateCounters + Send + Sync> = Arc::new(LateCounters::default);
    let mut aggregator = LateEpisodeAggregator::new(
        Arc::clone(&telemetry),
        counters,
        Duration::from_millis(10_000),
    );

    let first_ns = 1_000_000u64;
    telemetry.record_tile(true, 1_567, true, Some(first_ns));
    let start = aggregator.tick(
        first_ns + 1_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 1_000),
    );
    let slice = aggregator.tick(
        first_ns + 10_000_000_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 10_000_000_000),
    );
    let end = aggregator.tick(
        first_ns + 20_000_000_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 20_000_000_000),
    );

    let episode_id = |records: &[LateRecord]| match &records[0] {
        LateRecord::Start { episode_id, .. }
        | LateRecord::Slice { episode_id, .. }
        | LateRecord::End { episode_id, .. } => *episode_id,
    };

    let id = episode_id(&start);
    assert_eq!(episode_id(&slice), id);
    assert_eq!(episode_id(&end), id);
}

/// Runs one late episode with the given headroom and returns the slice's JSON record.
fn slice_with_headroom(window: HeadroomWindow) -> serde_json::Value {
    let buf = TestBuffer::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_writer(buf.clone())
            .with_ansi(false),
    );
    let telemetry = Arc::new(LateTileTelemetry::new(
        256,
        Duration::from_micros(1_451),
        176_400,
        11_289_600,
    ));
    let counters: Arc<dyn Fn() -> LateCounters + Send + Sync> = Arc::new(LateCounters::default);
    let headroom: HeadroomSource = Arc::new(move || window);
    let mut aggregator = LateEpisodeAggregator::new(
        Arc::clone(&telemetry),
        counters,
        Duration::from_millis(10_000),
    )
    .with_headroom(headroom);

    let first_ns = 1_000_000u64;
    telemetry.record_tile(true, 1_567, true, Some(first_ns));
    let mut records = aggregator.tick(
        first_ns + 1_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns),
    );
    records.extend(aggregator.tick(
        first_ns + 10_000_000_000,
        UNIX_EPOCH + Duration::from_nanos(first_ns + 10_000_000_000),
    ));
    tracing::subscriber::with_default(subscriber, || {
        for record in &records {
            record.emit();
        }
    });

    let output = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    output
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|v| v["event"] == "late_slice")
        .expect("late_slice record")
}

#[test]
fn slice_is_info_when_device_headroom_is_healthy() {
    let slice = slice_with_headroom(HeadroomWindow {
        min_us: Some(40_000),
        buffered_peak_us: Some(45_000),
    });
    assert_eq!(slice["level"], "INFO");
    assert_eq!(slice["headroom_min_us"], 40_000);
    assert_eq!(slice["buffer_peak_us"], 45_000);
}

#[test]
fn slice_is_warn_when_half_the_device_buffer_is_consumed() {
    let slice = slice_with_headroom(HeadroomWindow {
        min_us: Some(20_000),
        buffered_peak_us: Some(45_000),
    });
    assert_eq!(slice["level"], "WARN");
}

#[test]
fn headroom_threshold_is_exactly_half_the_buffer() {
    let at_half = HeadroomWindow {
        min_us: Some(22_500),
        buffered_peak_us: Some(45_000),
    };
    assert!(!at_half.is_low());
    let just_below = HeadroomWindow {
        min_us: Some(22_499),
        buffered_peak_us: Some(45_000),
    };
    assert!(just_below.is_low());
    let starved = HeadroomWindow {
        min_us: Some(-1),
        buffered_peak_us: Some(45_000),
    };
    assert!(starved.is_low());
}

#[test]
fn unknown_headroom_stays_warn() {
    assert!(HeadroomWindow::default().is_low());
    assert_eq!(
        slice_with_headroom(HeadroomWindow::default())["level"],
        "WARN"
    );
}
