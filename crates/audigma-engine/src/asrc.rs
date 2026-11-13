//! ASRC clock drift estimator and fill trim (`Design.md §1.5`).
//!
//! Reconciles a free-running input clock with the DAC master clock by commanding a bounded
//! parts-per-million (PPM) correction for the low-rate baseband polyphase ASRC.
//!
//! The queue obeys `d(fill)/dt = D - c`, where `D` is the true clock offset and `c` the applied
//! correction (both relative, here in ppm). The controller therefore *measures* `D` feed-forward
//! as `slope(fill) + mean(c)` over a long window, independent of any setpoint, and only adds a
//! small proportional trim (no integrator) that returns the fill to the level latched at stream
//! start. There is no integrator state to wind up.
//!
//! The estimate is meaningful only for a free-running producer. With a demand-driven producer the
//! producer rate *is* the consumption rate, so applying the estimate would measure its own output;
//! [`DriftMode::Observe`] runs the estimator with 0 ppm applied instead.
//!
//! Invariants:
//! - The applied correction is bounded by `max_correction_ppm` and slew-limited.
//! - Operates purely at low intermediate PCM rates (never at MHz DSD rates).
//! - A fill past a safety rail is acted on only after `fault_debounce_secs`; a confirmed rail
//!   applies the full correction in the restoring direction and reports a fault.
//!
//! Sign convention: the resampler's ratio is `input / output`. A *positive* PPM consumes input
//! frames faster and drains the queue; a *negative* PPM consumes input more slowly.

use audigma_settings::DriftCorrection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::traits::InputClock;

/// Errors arising from clock drift tracking.
#[derive(Debug, Error, PartialEq)]
pub enum ClockDriftError {
    #[error(
        "Buffer underrun / starvation threshold reached (fill {fill_ms:.1} ms < min {min_ms:.1} ms)"
    )]
    Starvation { fill_ms: f64, min_ms: f64 },

    #[error("Buffer overrun threshold reached (fill {fill_ms:.1} ms > max {max_ms:.1} ms)")]
    Overrun { fill_ms: f64, max_ms: f64 },
}

/// Drift handling resolved for one stream (`Design.md §1.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriftMode {
    /// No ASRC and no estimator.
    #[default]
    Off,
    /// Estimator runs and is reported; 0 ppm is applied and the audio path is the normal one.
    Observe,
    /// The ASRC applies `estimate + trim`.
    Correct,
}

impl DriftMode {
    /// Resolves `[resampler] drift_correction` against the output backend and the input clock.
    pub fn resolve(setting: DriftCorrection, alsa_output: bool, clock: InputClock) -> Self {
        match (setting, clock) {
            (DriftCorrection::Off, _) => Self::Off,
            (DriftCorrection::On, InputClock::FreeRunning) => Self::Correct,
            (DriftCorrection::On, InputClock::DemandDriven) => Self::Observe,
            (DriftCorrection::Auto, InputClock::FreeRunning) if alsa_output => Self::Correct,
            (DriftCorrection::Auto, _) => Self::Off,
        }
    }

    /// Stable lowercase name for logs and status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::Correct => "correct",
        }
    }
}

/// Number of blocks in the estimator's regression window.
pub const DRIFT_WINDOW_BLOCKS: usize = 64;

/// Configuration of the drift estimator and trim.
#[derive(Debug, Clone)]
pub struct ClockDriftConfig {
    /// Maximum applied correction in ppm (`[resampler] max_clock_drift_ppm`).
    pub max_correction_ppm: f64,
    /// Time after a stream start or idle exit whose fill samples are ignored: the queue is still
    /// settling (cushion refill, decoder priming) and its ramp would read as drift.
    pub warmup_secs: f64,
    /// Averaging block length in seconds; removes the per-chunk fill sawtooth.
    pub block_secs: f64,
    /// Blocks needed (of [`DRIFT_WINDOW_BLOCKS`]) before the estimate is valid.
    pub min_blocks: usize,
    /// Fill error tolerated without any trim, in seconds.
    pub trim_dead_band_secs: f64,
    /// Trim gain in ppm per millisecond of fill error beyond the dead band.
    pub trim_ppm_per_ms: f64,
    /// Largest trim in ppm.
    pub max_trim_ppm: f64,
    /// Largest change of the applied correction, in ppm per second.
    pub slew_ppm_per_sec: f64,
    /// Starvation rail in seconds.
    pub min_fill_secs: f64,
    /// Lowest overrun rail in seconds; the effective rail is `max(this, 2 * setpoint)`.
    pub max_fill_secs: f64,
    /// Time the fill must stay past a rail before it is acted on.
    pub fault_debounce_secs: f64,
}

impl Default for ClockDriftConfig {
    fn default() -> Self {
        Self {
            max_correction_ppm: 500.0,
            warmup_secs: 2.0,
            block_secs: 0.5,
            min_blocks: 16,
            trim_dead_band_secs: 0.005,
            // About a 200 s time constant: inaudible, and far slower than the estimator window
            // so the two never interact.
            trim_ppm_per_ms: 5.0,
            max_trim_ppm: 100.0,
            slew_ppm_per_sec: 10.0,
            min_fill_secs: 0.010,
            max_fill_secs: 0.250,
            fault_debounce_secs: 0.050,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Block {
    /// Block centre on the controller's time axis, in seconds.
    t: f64,
    /// Mean fill in seconds.
    fill: f64,
    /// Mean applied correction in ppm.
    ppm: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rail {
    Starvation,
    Overrun,
}

/// Real-time safe drift estimator plus fill trim (no allocation after construction).
#[derive(Debug, Clone)]
pub struct ClockDriftController {
    config: ClockDriftConfig,
    apply: bool,
    warmup_left_secs: f64,
    time_secs: f64,
    acc_secs: f64,
    acc_fill: f64,
    acc_ppm: f64,
    blocks: [Block; DRIFT_WINDOW_BLOCKS],
    block_count: usize,
    block_head: usize,
    setpoint_secs: Option<f64>,
    estimate_ppm: Option<f64>,
    trim_ppm: f64,
    applied_ppm: f64,
    below_min_secs: f64,
    above_max_secs: f64,
    rail: Option<Rail>,
}

impl ClockDriftController {
    /// Creates a controller. `apply = false` is [`DriftMode::Observe`]: the estimator runs and
    /// the applied correction stays 0.
    pub fn new(config: ClockDriftConfig, apply: bool) -> Self {
        Self {
            warmup_left_secs: config.warmup_secs,
            config,
            apply,
            time_secs: 0.0,
            acc_secs: 0.0,
            acc_fill: 0.0,
            acc_ppm: 0.0,
            blocks: [Block::default(); DRIFT_WINDOW_BLOCKS],
            block_count: 0,
            block_head: 0,
            setpoint_secs: None,
            estimate_ppm: None,
            trim_ppm: 0.0,
            applied_ppm: 0.0,
            below_min_secs: 0.0,
            above_max_secs: 0.0,
            rail: None,
        }
    }

    /// Feeds one fill sample taken after `dt_secs` of consumption and returns the correction to
    /// apply to the next tile.
    ///
    /// During the warm-up the queue is still settling after a cushion refill or pipeline rebuild,
    /// so neither the estimator nor the rails see those samples. Afterwards a fault is returned
    /// only once the fill has stayed past a rail for `fault_debounce_secs`; until then the
    /// correction is held unchanged.
    pub fn update(&mut self, fill_secs: f64, dt_secs: f64) -> Result<f64, ClockDriftError> {
        if !fill_secs.is_finite() || !dt_secs.is_finite() || dt_secs <= 0.0 {
            return Ok(self.applied_ppm);
        }
        if self.warmup_left_secs > 0.0 {
            self.warmup_left_secs -= dt_secs;
            return Ok(self.applied_ppm);
        }
        let max = self.config.max_correction_ppm;
        let max_fill = self.overrun_rail_secs();
        if fill_secs < self.config.min_fill_secs {
            self.below_min_secs += dt_secs;
            self.above_max_secs = 0.0;
            if self.below_min_secs >= self.config.fault_debounce_secs - 1e-9 {
                self.enter_rail(Rail::Starvation, -max);
                return Err(ClockDriftError::Starvation {
                    fill_ms: fill_secs * 1000.0,
                    min_ms: self.config.min_fill_secs * 1000.0,
                });
            }
            return Ok(self.applied_ppm);
        }
        if fill_secs > max_fill {
            self.above_max_secs += dt_secs;
            self.below_min_secs = 0.0;
            if self.above_max_secs >= self.config.fault_debounce_secs - 1e-9 {
                self.enter_rail(Rail::Overrun, max);
                return Err(ClockDriftError::Overrun {
                    fill_ms: fill_secs * 1000.0,
                    max_ms: max_fill * 1000.0,
                });
            }
            return Ok(self.applied_ppm);
        }
        self.below_min_secs = 0.0;
        self.above_max_secs = 0.0;
        self.rail = None;

        self.time_secs += dt_secs;
        self.acc_secs += dt_secs;
        self.acc_fill += fill_secs * dt_secs;
        self.acc_ppm += self.applied_ppm * dt_secs;
        if self.acc_secs >= self.config.block_secs {
            self.close_block();
        }

        if self.apply {
            let target = (self.estimate_ppm.unwrap_or(0.0) + self.trim_ppm).clamp(-max, max);
            let step = self.config.slew_ppm_per_sec * dt_secs;
            self.applied_ppm += (target - self.applied_ppm).clamp(-step, step);
        }
        Ok(self.applied_ppm)
    }

    /// Restarts measurement after the input went idle. The estimate is kept (the clocks did not
    /// change); the warm-up restarts and the setpoint, window, trim and rails are cleared. Returns the new correction.
    pub fn on_idle(&mut self) -> f64 {
        self.clear_window();
        self.warmup_left_secs = self.config.warmup_secs;
        self.setpoint_secs = None;
        self.trim_ppm = 0.0;
        self.below_min_secs = 0.0;
        self.above_max_secs = 0.0;
        self.rail = None;
        if self.apply {
            let max = self.config.max_correction_ppm;
            self.applied_ppm = self.estimate_ppm.unwrap_or(0.0).clamp(-max, max);
        }
        self.applied_ppm
    }

    /// The correction currently applied, in ppm (always 0 when observing).
    #[inline]
    pub fn applied_ppm(&self) -> f64 {
        self.applied_ppm
    }

    /// The measured clock offset in ppm, once the window holds enough blocks.
    #[inline]
    pub fn estimate_ppm(&self) -> Option<f64> {
        self.estimate_ppm
    }

    /// The fill level the trim returns to, latched from the first block.
    #[inline]
    pub fn setpoint_secs(&self) -> Option<f64> {
        self.setpoint_secs
    }

    /// The current trim in ppm.
    #[inline]
    pub fn trim_ppm(&self) -> f64 {
        self.trim_ppm
    }

    fn overrun_rail_secs(&self) -> f64 {
        self.config
            .max_fill_secs
            .max(2.0 * self.setpoint_secs.unwrap_or(0.0))
    }

    fn enter_rail(&mut self, rail: Rail, ppm: f64) {
        if self.rail != Some(rail) {
            // Rail samples say nothing about the clock; restart the window, keep the estimate.
            self.clear_window();
            self.rail = Some(rail);
        }
        if self.apply {
            self.applied_ppm = ppm;
        }
    }

    fn clear_window(&mut self) {
        self.acc_secs = 0.0;
        self.acc_fill = 0.0;
        self.acc_ppm = 0.0;
        self.block_count = 0;
        self.block_head = 0;
    }

    fn close_block(&mut self) {
        let block = Block {
            t: self.time_secs - 0.5 * self.acc_secs,
            fill: self.acc_fill / self.acc_secs,
            ppm: self.acc_ppm / self.acc_secs,
        };
        self.acc_secs = 0.0;
        self.acc_fill = 0.0;
        self.acc_ppm = 0.0;
        self.blocks[self.block_head] = block;
        self.block_head = (self.block_head + 1) % DRIFT_WINDOW_BLOCKS;
        self.block_count = (self.block_count + 1).min(DRIFT_WINDOW_BLOCKS);

        let setpoint = *self.setpoint_secs.get_or_insert(block.fill);
        if self.block_count >= self.config.min_blocks.clamp(2, DRIFT_WINDOW_BLOCKS) {
            self.estimate_ppm = self.regress();
        }
        let error = block.fill - setpoint;
        let excess = (error.abs() - self.config.trim_dead_band_secs).max(0.0);
        self.trim_ppm = (error.signum() * excess * 1000.0 * self.config.trim_ppm_per_ms)
            .clamp(-self.config.max_trim_ppm, self.config.max_trim_ppm);
    }

    /// `slope(fill) + mean(applied)` over the window, in ppm.
    fn regress(&self) -> Option<f64> {
        let n = self.block_count as f64;
        let window = || {
            (0..self.block_count).map(|i| {
                self.blocks[(self.block_head + DRIFT_WINDOW_BLOCKS - 1 - i) % DRIFT_WINDOW_BLOCKS]
            })
        };
        let (mut st, mut sf, mut sp) = (0.0, 0.0, 0.0);
        for b in window() {
            st += b.t;
            sf += b.fill;
            sp += b.ppm;
        }
        let (mt, mf) = (st / n, sf / n);
        let (mut num, mut den) = (0.0, 0.0);
        for b in window() {
            num += (b.t - mt) * (b.fill - mf);
            den += (b.t - mt) * (b.t - mt);
        }
        (den > 0.0).then(|| num / den * 1e6 + sp / n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_resolution_follows_setting_backend_and_clock() {
        use DriftCorrection::*;
        use InputClock::*;
        assert_eq!(DriftMode::resolve(Off, true, FreeRunning), DriftMode::Off);
        assert_eq!(
            DriftMode::resolve(Auto, true, FreeRunning),
            DriftMode::Correct
        );
        assert_eq!(DriftMode::resolve(Auto, false, FreeRunning), DriftMode::Off);
        assert_eq!(DriftMode::resolve(Auto, true, DemandDriven), DriftMode::Off);
        assert_eq!(
            DriftMode::resolve(On, false, FreeRunning),
            DriftMode::Correct
        );
        assert_eq!(
            DriftMode::resolve(On, true, DemandDriven),
            DriftMode::Observe
        );
    }

    #[test]
    fn steady_fill_holds_zero() {
        let mut c = ClockDriftController::new(ClockDriftConfig::default(), true);
        for _ in 0..20_000 {
            assert!(c.update(0.090, 0.005).unwrap().abs() < 1e-9);
        }
        assert!(c.estimate_ppm().unwrap().abs() < 1e-6);
        assert!((c.setpoint_secs().unwrap() - 0.090).abs() < 1e-12);
    }

    #[test]
    fn rail_waits_for_debounce_then_applies_full_correction() {
        let mut c = ClockDriftController::new(ClockDriftConfig::default(), true);
        for _ in 0..1000 {
            c.update(0.090, 0.005).unwrap();
        }
        // A drain shorter than the debounce (end of stream) leaves the correction untouched.
        for _ in 0..9 {
            assert_eq!(c.update(0.002, 0.005).unwrap(), 0.0);
        }
        let err = c.update(0.002, 0.005).unwrap_err();
        assert_eq!(
            err,
            ClockDriftError::Starvation {
                fill_ms: 2.0,
                min_ms: 10.0
            }
        );
        assert_eq!(c.applied_ppm(), -500.0);
        // Recovering slews back instead of jumping.
        let ppm = c.update(0.090, 0.005).unwrap();
        assert!((ppm + 500.0 - 0.05).abs() < 1e-9, "{ppm}");
    }

    #[test]
    fn rails_are_ignored_during_warmup() {
        let cfg = ClockDriftConfig {
            fault_debounce_secs: 0.0,
            ..Default::default()
        };
        // The queue dips right after a stream start while the pipeline primes.
        let mut c = ClockDriftController::new(cfg, true);
        for _ in 0..399 {
            assert_eq!(c.update(0.002, 0.005).unwrap(), 0.0);
        }
        for _ in 0..10 {
            c.update(0.020, 0.005).unwrap();
        }
        // Past the 2 s warm-up the same low fill is a fault again.
        assert!(matches!(
            c.update(0.002, 0.005),
            Err(ClockDriftError::Starvation { .. })
        ));
    }

    #[test]
    fn overrun_rail_scales_with_setpoint() {
        let cfg = ClockDriftConfig {
            fault_debounce_secs: 0.0,
            ..Default::default()
        };
        let mut c = ClockDriftController::new(cfg, true);
        // Past the warm-up and the first block, so the setpoint is latched.
        for _ in 0..600 {
            c.update(0.200, 0.005).unwrap();
        }
        // Setpoint 200 ms: the rail is at 400 ms, not 250 ms.
        assert!(c.update(0.300, 0.005).is_ok());
        assert!(matches!(
            c.update(0.401, 0.005),
            Err(ClockDriftError::Overrun { .. })
        ));
        assert_eq!(c.applied_ppm(), 500.0);
    }

    #[test]
    fn warmup_ramp_is_ignored() {
        // The queue climbs 15 ms during the first 1.5 s (refill after a rate switch), then holds.
        for warmup in [0.0, 2.0] {
            let cfg = ClockDriftConfig {
                warmup_secs: warmup,
                ..Default::default()
            };
            let mut c = ClockDriftController::new(cfg, false);
            for i in 0..2400 {
                let t = i as f64 * 0.005;
                c.update(0.075 + 0.015 * (t / 1.5).min(1.0), 0.005).unwrap();
            }
            // 12 s in: the first estimate is out.
            let est = c.estimate_ppm().unwrap();
            if warmup > 0.0 {
                assert!(est.abs() < 1e-6, "{est}");
                assert!((c.setpoint_secs().unwrap() - 0.090).abs() < 1e-9);
            } else {
                assert!(
                    est > 100.0,
                    "ramp should bias the estimate without warm-up: {est}"
                );
            }
        }
    }

    #[test]
    fn observe_never_applies() {
        let mut c = ClockDriftController::new(
            ClockDriftConfig {
                fault_debounce_secs: 0.0,
                ..Default::default()
            },
            false,
        );
        // A rising fill: estimate positive, nothing applied, rails do not act either.
        for i in 0..4000 {
            let fill = 0.090 + i as f64 * 0.005 * 100e-6;
            assert_eq!(c.update(fill, 0.005).unwrap(), 0.0);
        }
        assert!((c.estimate_ppm().unwrap() - 100.0).abs() < 1.0);
        assert!(c.update(0.001, 0.005).is_err());
        assert_eq!(c.applied_ppm(), 0.0);
        assert_eq!(c.on_idle(), 0.0);
    }

    #[test]
    fn idle_keeps_estimate_and_clears_the_rest() {
        let mut c = ClockDriftController::new(ClockDriftConfig::default(), true);
        for i in 0..4000 {
            c.update(0.090 + i as f64 * 0.005 * 50e-6, 0.005).unwrap();
        }
        let est = c.estimate_ppm().unwrap();
        assert_eq!(c.on_idle(), est);
        assert_eq!(c.setpoint_secs(), None);
        assert_eq!(c.trim_ppm(), 0.0);
        assert_eq!(c.estimate_ppm(), Some(est));
    }

    #[test]
    fn applied_correction_is_slew_limited_and_clamped() {
        let cfg = ClockDriftConfig {
            max_correction_ppm: 200.0,
            ..Default::default()
        };
        let mut c = ClockDriftController::new(cfg, true);
        let mut last = 0.0;
        // Fill rising at 1000 ppm: the estimate exceeds the limit.
        for i in 0..20_000 {
            let ppm = c.update(0.090 + i as f64 * 0.005 * 1000e-6, 0.005).unwrap();
            assert!((ppm - last).abs() <= 10.0 * 0.005 + 1e-9);
            assert!(ppm.abs() <= 200.0);
            last = ppm;
        }
        assert_eq!(last, 200.0);
    }
}
