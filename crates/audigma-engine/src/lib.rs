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

pub mod peak;
pub mod pipeline;
pub mod tail;
pub mod traits;

pub use peak::{Pass1PeakResult, measure_true_peak_pass1};
pub use pipeline::{OfflinePipeline, OfflineReport};
pub use tail::TailManager;
pub use traits::{
    AudioInput, AudioOutput, DecoderInput, EngineError, MockAudioInput, SeekableSink, StreamSink,
};
