//! Centralized DSP tile execution, multi-channel cascade dispatch, and deadline tracking (`Design.md §4.2`).

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::{Duration, Instant};

use audigma_dsp::config::{Apodization, FilterPhase, FilterPreset, TargetRateSpec};

use crate::late::LateTileTelemetry;
use crate::topology::PipelineTopology;
use crate::traits::EngineError;
use crate::worker::{ChannelWorkerPool, TargetDsdConfig};

/// Executes individual audio tiles through the half-band filter cascade and delta-sigma modulators,
/// measuring wall-clock execution against real-time deadline budgets when real-time tracking is enabled.
pub struct TileProcessor {
    worker_pool: ChannelWorkerPool,
    topology: PipelineTopology,
    tile_baseband_frames: usize,
    tile_budget: Option<Duration>,
    late_telemetry: Option<Arc<LateTileTelemetry>>,
}

impl TileProcessor {
    /// Creates a new `TileProcessor` configured for the specified channel count, filter parameters, and topology.
    ///
    /// If `is_realtime` is `true`, execution time is measured against `tile_baseband_frames / intermediate_rate`
    /// and late tile deadline misses are tracked. If `false` (offline mode), timing overhead is bypassed completely.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: usize,
        preset: FilterPreset,
        phase: FilterPhase,
        apodization: Apodization,
        topology: PipelineTopology,
        tile_baseband_frames: usize,
        target_dsd: Option<TargetDsdConfig>,
        is_realtime: bool,
        thread_init_hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    ) -> Result<Self, EngineError> {
        let worker_pool = ChannelWorkerPool::new(
            channels,
            preset,
            phase,
            apodization,
            topology.cascade_stages,
            tile_baseband_frames,
            target_dsd,
            thread_init_hook,
        )?;

        let tile_budget = if is_realtime {
            Some(Duration::from_secs_f64(
                tile_baseband_frames as f64 / topology.intermediate_rate.get() as f64,
            ))
        } else {
            None
        };

        let late_telemetry = tile_budget.map(|budget| {
            let output_rate_hz = match topology.target_spec {
                TargetRateSpec::Dsd { bit_rate } => bit_rate.get(),
                TargetRateSpec::Pcm { rate, .. } => rate.get(),
            };
            Arc::new(LateTileTelemetry::new(
                tile_baseband_frames,
                budget,
                topology.input_rate.get(),
                output_rate_hz,
            ))
        });

        Ok(Self {
            worker_pool,
            topology,
            tile_baseband_frames,
            tile_budget,
            late_telemetry,
        })
    }

    /// Processes one multi-channel tile through the half-band FIR cascade (PCM output).
    pub fn process_tile_pcm(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        if let Some(budget) = self.tile_budget {
            let frames = inputs.first().map(|s| s.len()).unwrap_or(0);
            let start = Instant::now();
            self.worker_pool.process_tile_pcm(inputs)?;
            self.record_tile_metrics(frames, start.elapsed(), budget);
        } else {
            self.worker_pool.process_tile_pcm(inputs)?;
        }
        Ok(())
    }

    /// Processes one multi-channel tile through the half-band cascade and delta-sigma modulators (DSD output).
    pub fn process_tile_dsd(&mut self, inputs: &[&[f64]]) -> Result<(), EngineError> {
        if let Some(budget) = self.tile_budget {
            let frames = inputs.first().map(|s| s.len()).unwrap_or(0);
            let start = Instant::now();
            self.worker_pool.process_tile_dsd(inputs)?;
            self.record_tile_metrics(frames, start.elapsed(), budget);
        } else {
            self.worker_pool.process_tile_dsd(inputs)?;
        }
        Ok(())
    }

    #[inline]
    fn record_tile_metrics(&self, frames: usize, elapsed: Duration, budget: Duration) {
        let Some(telemetry) = self.late_telemetry.as_ref() else {
            return;
        };

        let us = elapsed.as_micros() as u64;
        let is_full = frames == self.tile_baseband_frames;

        // Only full-size tiles count as deadline misses. Partial tiles are dominated by
        // fixed dispatch/futex overhead that does not scale with tile length.
        let is_late = is_full && elapsed > budget;

        // Reading the monotonic clock is deferred to the rare late path only.
        let mono_ns = is_late.then(|| telemetry.mono_ns());
        telemetry.record_tile(is_full, us, is_late, mono_ns);
    }

    /// Returns the pipeline topology for this processor.
    #[inline]
    pub fn topology(&self) -> &PipelineTopology {
        &self.topology
    }

    /// Returns the baseband tile frame capacity.
    #[inline]
    pub fn tile_baseband_frames(&self) -> usize {
        self.tile_baseband_frames
    }

    /// Returns the real-time deadline budget for a full baseband tile, or `None` in offline mode.
    #[inline]
    pub fn tile_budget(&self) -> Option<Duration> {
        self.tile_budget
    }

    /// Returns the maximum observed single-tile DSP execution time in microseconds.
    #[inline]
    pub fn max_dsp_tile_us(&self) -> u64 {
        self.late_telemetry
            .as_ref()
            .map(|t| t.max_tile_us())
            .unwrap_or(0)
    }

    /// Returns the total number of full tiles whose execution exceeded `tile_budget`.
    #[inline]
    pub fn total_late_tiles(&self) -> u64 {
        self.late_telemetry
            .as_ref()
            .map(|t| t.total_late_tiles())
            .unwrap_or(0)
    }

    /// Returns the total number of tiles processed since initialization.
    #[inline]
    pub fn total_tiles_processed(&self) -> u64 {
        self.late_telemetry
            .as_ref()
            .map(|t| t.total_tiles_processed())
            .unwrap_or(0)
    }

    /// Returns the shared deadline-miss telemetry, if this processor is real-time.
    #[inline]
    pub fn late_telemetry(&self) -> Option<Arc<LateTileTelemetry>> {
        self.late_telemetry.clone()
    }

    /// Updates the buffer-fill context sampled for the next late tile.
    #[inline]
    pub fn set_buffer_fill_ms(&self, fill_ms: Option<f64>) {
        if let Some(telemetry) = self.late_telemetry.as_ref() {
            telemetry.set_current_fill_ms(fill_ms);
        }
    }
}

impl Deref for TileProcessor {
    type Target = ChannelWorkerPool;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.worker_pool
    }
}

impl DerefMut for TileProcessor {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.worker_pool
    }
}
