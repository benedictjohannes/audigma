use audigma_log::{
    DecimationErrorRecorder, LogConfig, LogFormat, LogLevel, RtLogDrainer, RtLogEvent,
    rt_log_channel,
};
use std::sync::{Arc, Mutex};
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
fn test_log_config_precedence() {
    // 1. CLI flag takes top precedence
    let cfg = LogConfig::resolve(
        Some(LogFormat::Json),
        Some(LogLevel::Debug),
        Some("text"),
        Some("info"),
        LogLevel::Warn,
    );
    assert_eq!(cfg.format, LogFormat::Json);
    assert_eq!(cfg.filter, "debug");

    // 2. Settings take precedence over default when CLI is None
    let cfg = LogConfig::resolve(None, None, Some("json"), Some("error"), LogLevel::Info);
    assert_eq!(cfg.format, LogFormat::Json);
    assert_eq!(cfg.filter, "error");

    // 3. Command default takes precedence when both CLI and settings are None
    let cfg = LogConfig::resolve(None, None, None, None, LogLevel::Warn);
    assert_eq!(cfg.format, LogFormat::Text);
    assert_eq!(cfg.filter, "warn");
}

#[test]
fn test_rt_queue_overflow_and_drain() {
    let (mut producer, mut consumer, dropped) = rt_log_channel(2);

    let ev1 = RtLogEvent::VolumeEvent {
        volume_percent: Some(75.0),
        volume_db: Some(-6.0),
        muted: Some(false),
    };
    let ev2 = RtLogEvent::FormatNegotiated {
        is_dsd: false,
        rate_hz: 48000,
        bit_rate_hz: 48000,
        intermediate_rate_hz: 48000,
        channels: 2,
        interleave: 1,
    };
    let ev3 = RtLogEvent::DecimationError {
        channel: 0,
        code: -5,
        at_ns: 123456,
    };

    assert!(producer.try_push(ev1).is_ok());
    assert!(producer.try_push(ev2).is_ok());
    // Queue is now full (capacity 2), push must fail and increment dropped
    assert!(producer.try_push(ev3).is_err());
    assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(producer.dropped_count(), 1);

    // Consumer drains in FIFO order
    assert_eq!(consumer.pop().unwrap(), ev1);
    assert_eq!(consumer.pop().unwrap(), ev2);
    assert!(consumer.pop().is_err());

    // After pop, space is available again
    assert!(producer.try_push(ev3).is_ok());
    assert_eq!(consumer.pop().unwrap(), ev3);
}

#[test]
fn test_decimation_error_recorder() {
    let recorder = DecimationErrorRecorder::new();
    assert_eq!(recorder.snapshot_and_reset(), (0, 0, 0));

    recorder.record(1, -22);
    recorder.record(1, -23);
    recorder.record(0, -32);

    let (count, code, ch) = recorder.snapshot_and_reset();
    assert_eq!(count, 3);
    assert_eq!(code, -32);
    assert_eq!(ch, 0);

    // After reset, count is 0
    let (count, code, ch) = recorder.snapshot_and_reset();
    assert_eq!(count, 0);
    assert_eq!(code, -32);
    assert_eq!(ch, 0);
}

#[test]
fn test_json_renderer_format() {
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

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            target: "audigma::clock",
            event = "rate_switch",
            from_hz = 44100,
            to_hz = 176400,
            generation = 3,
            reopened_output = false,
            "dynamic sample rate switch detected"
        );
    });

    let raw = buf.0.lock().unwrap().clone();
    let output_str = String::from_utf8(raw).unwrap();
    eprintln!("JSON OUTPUT: {}", output_str);
    let parsed: serde_json::Value = serde_json::from_str(&output_str).unwrap();

    assert_eq!(parsed["level"], "INFO");
    assert_eq!(parsed["target"], "audigma::clock");
    assert_eq!(parsed["event"], "rate_switch");
    assert_eq!(parsed["from_hz"], 44100);
    assert_eq!(parsed["to_hz"], 176400);
    assert_eq!(parsed["generation"], 3);
    assert_eq!(parsed["reopened_output"], false);
    assert_eq!(parsed["message"], "dynamic sample rate switch detected");
    assert!(parsed.get("timestamp").is_some());
    assert!(parsed.get("threadName").is_some() || parsed.get("thread").is_some());
}

#[test]
fn test_text_renderer_format() {
    let buf = TestBuffer::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(buf.clone())
            .with_ansi(false)
            .with_thread_names(true)
            .with_thread_ids(false)
            .with_target(true),
    );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            target: "audigma::clock",
            event = "rate_switch",
            from_hz = 44100,
            to_hz = 176400,
            generation = 3,
            reopened_output = false,
            "dynamic sample rate switch detected"
        );
    });

    let raw = buf.0.lock().unwrap().clone();
    let output_str = String::from_utf8(raw).unwrap();

    assert!(output_str.contains("INFO"));
    assert!(output_str.contains("audigma::clock"));
    assert!(output_str.contains("dynamic sample rate switch detected"));
    assert!(output_str.contains("event=\"rate_switch\""));
    assert!(output_str.contains("from_hz=44100"));
    assert!(output_str.contains("to_hz=176400"));
    assert!(output_str.contains("generation=3"));
    assert!(output_str.contains("reopened_output=false"));
}

#[test]
fn test_rt_drainer_lifecycle() {
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

    let (mut producer, consumer, dropped) = rt_log_channel(16);
    let mut drainer = RtLogDrainer::new();
    drainer.add_channel(consumer, dropped);

    let recorder = Arc::new(DecimationErrorRecorder::new());
    drainer.add_decimation_recorder(Arc::clone(&recorder));

    producer
        .try_push(RtLogEvent::FormatNegotiated {
            is_dsd: false,
            rate_hz: 96000,
            bit_rate_hz: 96000,
            intermediate_rate_hz: 96000,
            channels: 2,
            interleave: 1,
        })
        .unwrap();

    recorder.record(0, -1);

    tracing::subscriber::with_default(subscriber, || {
        let mut handle = drainer.spawn();
        std::thread::sleep(std::time::Duration::from_millis(150));
        handle.stop();
    });

    let raw = buf.0.lock().unwrap().clone();
    let output_str = String::from_utf8(raw).unwrap();

    assert!(output_str.contains("pw_format_negotiated"));
    assert!(output_str.contains("decimation_error_summary"));
}
