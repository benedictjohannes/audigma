//! Shutdown-flush coverage for the `audigma-late` background thread.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use audigma_engine::late::{LateCounters, LateTileMonitor, LateTileTelemetry};
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
fn monitor_flushes_open_episode_on_shutdown() {
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
    tracing::subscriber::set_global_default(subscriber)
        .expect("global tracing subscriber must be settable in this test binary");

    let telemetry = Arc::new(LateTileTelemetry::new(
        256,
        Duration::from_micros(1_451),
        176_400,
        11_289_600,
    ));
    telemetry.record_tile(true, 1_567, true, Some(1_000_000));

    let counters: Arc<dyn Fn() -> LateCounters + Send + Sync> = Arc::new(LateCounters::default);
    let monitor = LateTileMonitor::spawn(
        Arc::clone(&telemetry),
        counters,
        Duration::from_secs(10),
        None,
        None,
    );

    // Allow the 100 ms poll loop to observe the late tile and emit late_start.
    std::thread::sleep(Duration::from_millis(250));
    drop(monitor);

    let raw = buf.0.lock().unwrap().clone();
    let output = String::from_utf8(raw).unwrap();
    let events: Vec<serde_json::Value> = output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("valid JSON log line"))
        .collect();

    assert!(
        events
            .iter()
            .any(|e| e["event"] == "late_start" && e["level"] == "DEBUG"),
        "expected DEBUG late_start, got: {output}"
    );
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "late_end" && e["level"] == "INFO"),
        "expected shutdown INFO late_end, got: {output}"
    );
    let end = events
        .iter()
        .find(|e| e["event"] == "late_end")
        .expect("late_end present");
    assert_eq!(end["target"], "audigma::late");
    assert_eq!(end["late"], 1);
    assert_eq!(end["budget_us"], 1_451);
}
