//! Centralized DSP tile execution, multi-channel cascade dispatch, and deadline tracking (`Design.md §4.2`, `LateTileLogging.md`).

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::{Duration, Instant};

use audigma_dsp::config::{Apodization, FilterPhase, FilterPreset};

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
    total_tiles_processed: u64,
    total_late_tiles: u64,
    max_dsp_tile_us: u64,
}

impl TileProcessor {
    /// Creates a new `TileProcessor` configured for the specified channel count, filter parameters, and topology.
    ///
    /// If `is_realtime` is `true`, execution time is measured against `tile_baseband_frames / intermediate_rate`
    /// and late tile deadline misses are tracked (`LateTileLogging.md`). If `false` (offline mode),
    /// timing overhead is bypassed completely.
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

        Ok(Self {
            worker_pool,
            topology,
            tile_baseband_frames,
            tile_budget,
            total_tiles_processed: 0,
            total_late_tiles: 0,
            max_dsp_tile_us: 0,
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
    fn record_tile_metrics(&mut self, frames: usize, elapsed: Duration, budget: Duration) {
        let us = elapsed.as_micros() as u64;
        if us > self.max_dsp_tile_us {
            self.max_dsp_tile_us = us;
        }
        self.total_tiles_processed = self.total_tiles_processed.saturating_add(1);

        // Only full-size tiles count as deadline misses (`LateTileLogging.md §2`). Partial tiles
        // are dominated by fixed dispatch/futex overhead that does not scale with tile length.
        if frames == self.tile_baseband_frames && elapsed > budget {
            self.total_late_tiles = self.total_late_tiles.saturating_add(1);
        }
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
        self.max_dsp_tile_us
    }

    /// Returns the total number of full tiles whose execution exceeded `tile_budget`.
    #[inline]
    pub fn total_late_tiles(&self) -> u64 {
        self.total_late_tiles
    }

    /// Returns the total number of tiles processed since initialization.
    #[inline]
    pub fn total_tiles_processed(&self) -> u64 {
        self.total_tiles_processed
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
