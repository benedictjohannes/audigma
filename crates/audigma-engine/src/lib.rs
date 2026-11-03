//! `audigma-engine`: Pure-Rust audio pipeline planner, offline converter, and abstract I/O traits
//! (`Design.md §2`, `§3.4`).
//!
//! Provides:
//! - Abstract, zero-FFI `AudioInput` and `AudioOutput` stream traits.
//! - Offline pipeline runner (`OfflinePipeline`) chaining channel topology conditioning,
//!   pre-modulator gain staging, two-pass true-peak analysis (`peak_analysis = "two-pass"`),
//!   polyphase rational SRC, 2^K half-band FIR cascades, 5th-order CRFB delta-sigma modulators,
//!   and container egress.
//! - Deterministic seeding, gapless `trim-to-duration` phase-alignment, and `preserve-tail` flushing.

pub mod asrc;
pub mod decimator;
pub mod late;
pub mod lifecycle;
pub mod live_pipeline;
pub mod offline_pipeline;
pub mod peak;
pub mod processor;
pub mod queue;
pub mod tail;
pub mod topology;
pub mod traits;
pub mod transport;
pub mod worker;

pub use asrc::{AdaptiveClockDriftController, ClockDriftConfig, ClockDriftError};
pub use decimator::ChannelDecimatorPool;
pub use late::{
    LateCounterSource, LateCounters, LateEpisodeAggregator, LateRecord, LateTileMonitor,
    LateTileTelemetry,
};
pub use lifecycle::{
    BlockAction, FrameGain, IdleCarrierFormat, LifecycleController, LifecycleState,
};
pub use live_pipeline::{
    DaemonControlHandle, DaemonOutputFormat, DaemonState, DaemonTelemetry, DaemonVolumeMode,
    LivePipeline, StreamExitReason,
};
pub use offline_pipeline::{OfflinePipeline, OfflineReport};
pub use peak::{Pass1PeakResult, measure_true_peak_pass1};
pub use processor::TileProcessor;
pub use queue::{AudioSpscQueue, DEFAULT_CHUNK_SAMPLES, TaggedChunk};
pub use tail::TailManager;
pub use topology::PipelineTopology;
pub use traits::{
    AudioInput, AudioOutput, DecoderInput, DsdDecodeConfig, EngineError, HardwareVolumeBridge,
    MockAudioInput, SeekableSink, StreamSink, VolumeUpdate,
};
pub use transport::{
    is_dsd_passthrough_eligible, pack_and_write_stereo_dsd, process_live_dsd_passthrough_tile,
    transfer_offline_dsd_passthrough, write_output_all,
};
pub use worker::{
    ChannelWorkerPool, InputDspConfig, InputResampling, InputTileControls, TargetDsdConfig,
};
