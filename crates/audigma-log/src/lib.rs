//! `audigma-log`: Structured logging and real-time safe telemetry foundation (`StructuredLogging.md`).

pub mod config;
pub mod events;
pub mod rt;

use std::io::IsTerminal;
use tracing_appender::non_blocking::{NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub use config::{LogConfig, LogConfigError, LogFormat, LogLevel};
pub use events::*;
pub use rt::{
    DEFAULT_RT_QUEUE_CAPACITY, DecimationErrorRecorder, RtLogConsumer, RtLogDrainer,
    RtLogDrainerHandle, RtLogEvent, RtLogProducer, RtQueueFullError, emit_rt_event, rt_log_channel,
};

/// Default capacity for the non-blocking writer buffer (records).
pub const NON_BLOCKING_BUFFER_LIMIT: usize = 10_000;

/// Initializes the global tracing subscriber according to `LogConfig` writing to stderr.
///
/// Configures a single non-blocking writer with `lossy: true` and 10,000 capacity
/// to eliminate stderr lock contention from application threads (`StructuredLogging.md §4`, `§9`).
pub fn init(config: &LogConfig) -> Result<WorkerGuard, LogConfigError> {
    let (non_blocking, guard) = NonBlockingBuilder::default()
        .lossy(true)
        .buffered_lines_limit(NON_BLOCKING_BUFFER_LIMIT)
        .finish(std::io::stderr());

    let is_terminal = std::io::stderr().is_terminal();
    init_internal(config, non_blocking, is_terminal)?;
    Ok(guard)
}

/// Internal subscriber registration with custom non-blocking writer.
fn init_internal<W>(
    config: &LogConfig,
    writer: W,
    is_terminal: bool,
) -> Result<(), LogConfigError>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Send + Sync + 'static,
{
    let env_filter = EnvFilter::try_new(&config.filter)
        .map_err(|e| LogConfigError::FilterParse(e.to_string()))?;

    let effective_format = config.format.resolve_effective();

    match effective_format {
        LogFormat::Text => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(is_terminal)
                .with_thread_names(true)
                .with_thread_ids(false)
                .with_target(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init()
                .map_err(|e| LogConfigError::SetGlobalDefault(e.to_string()))?;
        }
        LogFormat::Json => {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .with_span_list(false)
                .with_writer(writer)
                .with_ansi(false)
                .with_thread_names(true)
                .with_thread_ids(false)
                .with_target(true);

            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt_layer)
                .try_init()
                .map_err(|e| LogConfigError::SetGlobalDefault(e.to_string()))?;
        }
        LogFormat::Auto => unreachable!("resolve_effective resolves Auto to Text or Json"),
    }

    Ok(())
}
