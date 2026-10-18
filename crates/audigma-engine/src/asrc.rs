//! Proportional-Integral (PI) ASRC Clock Drift Controller (`Design.md §1`, `§3.4`).
//!
//! Reconciles independent hardware clock domains (e.g. PipeWire input clock vs. ALSA DAC master clock)
//! by tracking SPSC ring buffer fill levels and computing bounded parts-per-million (PPM) adjustments
//! for the low-rate baseband polyphase ASRC resampler.
//!
//! Invariants:
//! - Drift correction is bounded by `max_correction_ppm` (default 500 ppm).
//! - Operates purely at low intermediate PCM rates (never at MHz DSD rates).
//! - When fill level crosses extreme safety thresholds (starvation or overflow), triggers a controlled
//!   fault/recovery event rather than letting the queue corrupt or silently drop/repeat DSD bits.

use thiserror::Error;

/// Errors arising from adaptive clock drift tracking.
#[derive(Debug, Error, PartialEq)]
pub enum ClockDriftError {
    #[error("Buffer underrun / starvation threshold reached (fill {fill:.1}% < min {min:.1}%)")]
    Starvation { fill: f64, min: f64 },

    #[error("Buffer overrun threshold reached (fill {fill:.1}% > max {max:.1}%)")]
    Overrun { fill: f64, max: f64 },
}

/// Configuration parameters for the PI clock drift controller.
#[derive(Debug, Clone)]
pub struct ClockDriftConfig {
    /// Desired target queue fill fraction (e.g. 0.5 for 50% midpoint).
    pub target_fill: f64,
    /// Maximum allowed clock correction in parts-per-million (PPM).
    pub max_correction_ppm: f64,
    /// Proportional gain coefficient ($K_p$).
    pub kp: f64,
    /// Integral gain coefficient ($K_i$).
    pub ki: f64,
    /// Low starvation safety threshold fraction (e.g. 0.05 for 5%).
    pub min_fill_threshold: f64,
    /// High overrun safety threshold fraction (e.g. 0.95 for 95%).
    pub max_fill_threshold: f64,
    /// Integral anti-windup clamp in PPM.
    pub integral_clamp_ppm: f64,
}

impl Default for ClockDriftConfig {
    fn default() -> Self {
        Self {
            target_fill: 0.5,
            max_correction_ppm: 500.0,
            kp: 250.0, // e.g. 10% fill deviation produces 25 ppm proportional response
            ki: 5.0,   // slowly zeroes out steady-state frequency offset
            min_fill_threshold: 0.05,
            max_fill_threshold: 0.95,
            integral_clamp_ppm: 300.0,
        }
    }
}

/// Real-time safe PI clock drift controller for low-rate ASRC.
#[derive(Debug, Clone)]
pub struct AdaptiveClockDriftController {
    config: ClockDriftConfig,
    integral_err: f64,
    last_correction_ppm: f64,
    smoothed_fill: f64,
    consecutive_safety_faults: usize,
}

impl AdaptiveClockDriftController {
    /// Creates a new controller with given configuration.
    pub fn new(config: ClockDriftConfig) -> Self {
        let initial_fill = config.target_fill;
        Self {
            config,
            integral_err: 0.0,
            last_correction_ppm: 0.0,
            smoothed_fill: initial_fill,
            consecutive_safety_faults: 0,
        }
    }

    /// Resets the integral state and historical filters.
    pub fn reset(&mut self) {
        self.integral_err = 0.0;
        self.last_correction_ppm = 0.0;
        self.smoothed_fill = self.config.target_fill;
        self.consecutive_safety_faults = 0;
    }

    /// Updates the drift controller with current queue occupancy.
    ///
    /// - `occupied_slots`: Current number of occupied chunks/frames in queue.
    /// - `total_capacity`: Maximum capacity of the queue.
    /// - `dt_seconds`: Elapsed time since last update in seconds (typically chunk duration, e.g. 0.01s).
    ///
    /// Returns the computed clock correction in parts-per-million (PPM) to pass to `PolyphaseResampler::set_drift_ppm`.
    /// Positive PPM means output is running faster than input (needs input upsampled slightly faster).
    pub fn update(
        &mut self,
        occupied_slots: usize,
        total_capacity: usize,
        dt_seconds: f64,
    ) -> Result<f64, ClockDriftError> {
        if total_capacity == 0 {
            return Ok(0.0);
        }

        let raw_fill = occupied_slots as f64 / total_capacity as f64;

        // Safety threshold checks
        if raw_fill < self.config.min_fill_threshold {
            self.consecutive_safety_faults += 1;
            return Err(ClockDriftError::Starvation {
                fill: raw_fill * 100.0,
                min: self.config.min_fill_threshold * 100.0,
            });
        } else if raw_fill > self.config.max_fill_threshold {
            self.consecutive_safety_faults += 1;
            return Err(ClockDriftError::Overrun {
                fill: raw_fill * 100.0,
                max: self.config.max_fill_threshold * 100.0,
            });
        }
        self.consecutive_safety_faults = 0;

        // Exponential smoothing on raw fill to filter short-term scheduler jitter
        let alpha = 0.1;
        self.smoothed_fill = (1.0 - alpha) * self.smoothed_fill + alpha * raw_fill;

        // Error = (target - actual):
        // If queue is below target (fill < target), input is arriving too slowly compared to consumer,
        // so we need positive ppm to slow down consumption / speed up generation.
        let error = self.config.target_fill - self.smoothed_fill;

        // Integrate error with anti-windup clamp
        self.integral_err += error * dt_seconds;
        let clamped_integral = (self.integral_err * self.config.ki).clamp(
            -self.config.integral_clamp_ppm,
            self.config.integral_clamp_ppm,
        );

        // Compute PI correction
        let p_term = error * self.config.kp;
        let total_ppm = (p_term + clamped_integral).clamp(
            -self.config.max_correction_ppm,
            self.config.max_correction_ppm,
        );

        self.last_correction_ppm = total_ppm;
        Ok(total_ppm)
    }

    /// Returns the most recent drift correction in PPM.
    #[inline]
    pub fn last_drift_ppm(&self) -> f64 {
        self.last_correction_ppm
    }

    /// Returns the current smoothed queue fill fraction (0.0 to 1.0).
    #[inline]
    pub fn smoothed_fill(&self) -> f64 {
        self.smoothed_fill
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drift_controller_midpoint_stable() {
        let config = ClockDriftConfig::default();
        let mut controller = AdaptiveClockDriftController::new(config);

        // Feed exact 50% capacity
        for _ in 0..100 {
            let ppm = controller.update(50, 100, 0.01).unwrap();
            assert!(ppm.abs() < 1e-4);
        }
        assert_eq!(controller.last_drift_ppm(), 0.0);
    }

    #[test]
    fn test_drift_controller_bounded_ppm() {
        let config = ClockDriftConfig {
            max_correction_ppm: 500.0,
            ..Default::default()
        };
        let mut controller = AdaptiveClockDriftController::new(config);

        // Heavy starvation just above threshold
        for _ in 0..50 {
            let ppm = controller.update(10, 100, 0.01).unwrap();
            assert!(ppm <= 500.0);
            assert!(ppm > 0.0);
        }
    }

    #[test]
    fn test_drift_controller_starvation_fault() {
        let config = ClockDriftConfig {
            min_fill_threshold: 0.10,
            ..Default::default()
        };
        let mut controller = AdaptiveClockDriftController::new(config);

        // Below minimum threshold
        let err = controller.update(5, 100, 0.01).unwrap_err();
        match err {
            ClockDriftError::Starvation { fill, min } => {
                assert_eq!(fill, 5.0);
                assert_eq!(min, 10.0);
            }
            _ => panic!("Expected starvation error"),
        }
    }
}
