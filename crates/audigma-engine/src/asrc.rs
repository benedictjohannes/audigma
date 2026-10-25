//! Proportional-Integral (PI) ASRC Clock Drift Controller (`Design.md §1`, `§3.4`).
//!
//! Reconciles independent hardware clock domains (e.g. PipeWire input clock vs. ALSA DAC master clock)
//! by tracking the buffered input duration and computing bounded parts-per-million (PPM) adjustments
//! for the low-rate baseband polyphase ASRC resampler.
//!
//! Invariants:
//! - Drift correction is bounded by `max_correction_ppm` (default 500 ppm).
//! - Operates purely at low intermediate PCM rates (never at MHz DSD rates).
//! - When the buffer stays past an extreme safety threshold (starvation or overrun) for
//!   `fault_debounce_secs`, the controller applies the maximum correction in the direction that
//!   restores the buffer and reports a fault event. Brief chunk-level dips do not trip a fault.
//!
//! Sign convention: the resampler's ratio is `input / output`. A *positive* PPM therefore consumes
//! input frames faster and drains the queue; a *negative* PPM consumes input more slowly and fills
//! the queue. The controller's error is `(fill - target) / target`, so a low buffer produces a
//! negative correction.

use thiserror::Error;

/// Errors arising from adaptive clock drift tracking.
#[derive(Debug, Error, PartialEq)]
pub enum ClockDriftError {
    #[error(
        "Buffer underrun / starvation threshold reached (fill {fill_ms:.1} ms < min {min_ms:.1} ms)"
    )]
    Starvation { fill_ms: f64, min_ms: f64 },

    #[error("Buffer overrun threshold reached (fill {fill_ms:.1} ms > max {max_ms:.1} ms)")]
    Overrun { fill_ms: f64, max_ms: f64 },
}

/// Configuration parameters for the PI clock drift controller.
#[derive(Debug, Clone)]
pub struct ClockDriftConfig {
    /// Desired buffered input duration in seconds (e.g. 0.060 for 60 ms).
    pub target_fill_secs: f64,
    /// Maximum allowed clock correction in parts-per-million (PPM).
    pub max_correction_ppm: f64,
    /// Proportional gain in PPM per unit of *relative* fill error.
    pub kp: f64,
    /// Integral gain coefficient ($K_i$).
    pub ki: f64,
    /// Low starvation safety threshold in seconds.
    pub min_fill_secs: f64,
    /// High overrun safety threshold in seconds.
    pub max_fill_secs: f64,
    /// Integral anti-windup clamp in PPM.
    pub integral_clamp_ppm: f64,
    /// Time constant of the fill low-pass filter, in seconds.
    ///
    /// Must be several times longer than the PipeWire quantum so the per-chunk
    /// sawtooth is averaged out instead of driving the proportional term.
    pub smoothing_tau_secs: f64,
    /// Minimum time the buffer must remain past a safety rail before a fault is
    /// reported. Chunk-level dips shorter than this are treated as normal PI input.
    pub fault_debounce_secs: f64,
}

impl Default for ClockDriftConfig {
    fn default() -> Self {
        Self {
            target_fill_secs: 0.060,
            max_correction_ppm: 500.0,
            // A 10% relative fill error produces 35 ppm. Deliberately gentle:
            // the fill signal is quantized by the PipeWire quantum (~23 ms),
            // so a high gain turns each chunk step into audible pitch jitter.
            kp: 350.0,
            // Slowly removes persistent oscillator offset. Combined with the
            // wide integral clamp below, this holds a few-hundred-ppm real
            // offset at the target without a large steady-state error.
            ki: 8.0,
            min_fill_secs: 0.010,
            max_fill_secs: 0.250,
            // Must exceed the expected real clock offset (~250 ppm measured on
            // this setup); otherwise the P term has to make up the difference
            // and the buffer settles tens of milliseconds off target.
            integral_clamp_ppm: 400.0,
            smoothing_tau_secs: 0.060,
            fault_debounce_secs: 0.050,
        }
    }
}

/// Real-time safe PI clock drift controller for low-rate ASRC.
#[derive(Debug, Clone)]
pub struct AdaptiveClockDriftController {
    config: ClockDriftConfig,
    integral_err: f64,
    last_correction_ppm: f64,
    smoothed_fill_secs: f64,
    consecutive_safety_faults: usize,
    below_min_secs: f64,
    above_max_secs: f64,
}

impl AdaptiveClockDriftController {
    /// Creates a new controller with given configuration.
    pub fn new(config: ClockDriftConfig) -> Self {
        let initial_fill = config.target_fill_secs;
        Self {
            config,
            integral_err: 0.0,
            last_correction_ppm: 0.0,
            smoothed_fill_secs: initial_fill,
            consecutive_safety_faults: 0,
            below_min_secs: 0.0,
            above_max_secs: 0.0,
        }
    }

    /// Resets the integral state and historical filters.
    pub fn reset(&mut self) {
        self.integral_err = 0.0;
        self.last_correction_ppm = 0.0;
        self.smoothed_fill_secs = self.config.target_fill_secs;
        self.consecutive_safety_faults = 0;
        self.below_min_secs = 0.0;
        self.above_max_secs = 0.0;
    }

    /// Updates the drift controller with the current buffered input duration.
    ///
    /// - `fill_secs`: Current buffered input duration in seconds.
    /// - `dt_seconds`: Elapsed time since the last update in seconds.
    ///
    /// Returns the computed clock correction in parts-per-million (PPM) to pass to
    /// `PolyphaseResampler::set_drift_ppm`.
    ///
    /// Positive PPM consumes input faster (drains the queue); negative PPM consumes
    /// input more slowly (fills the queue).
    ///
    /// A safety fault is only reported once the buffer has stayed past the rail for
    /// `fault_debounce_secs`; before that the rail correction is still returned as a
    /// normal `Ok` value so the buffer is actively restored.
    pub fn update(&mut self, fill_secs: f64, dt_seconds: f64) -> Result<f64, ClockDriftError> {
        if !fill_secs.is_finite() {
            return Ok(self.last_correction_ppm);
        }

        let max = self.config.max_correction_ppm;

        if fill_secs < self.config.min_fill_secs {
            self.below_min_secs += dt_seconds;
            self.above_max_secs = 0.0;
            // Respond immediately and avoid integral windup while past the rail.
            self.smoothed_fill_secs = fill_secs;
            self.integral_err = 0.0;
            self.last_correction_ppm = -max;
            if self.below_min_secs >= self.config.fault_debounce_secs {
                self.consecutive_safety_faults += 1;
                return Err(ClockDriftError::Starvation {
                    fill_ms: fill_secs * 1000.0,
                    min_ms: self.config.min_fill_secs * 1000.0,
                });
            }
            return Ok(-max);
        } else if fill_secs > self.config.max_fill_secs {
            self.above_max_secs += dt_seconds;
            self.below_min_secs = 0.0;
            self.smoothed_fill_secs = fill_secs;
            self.integral_err = 0.0;
            self.last_correction_ppm = max;
            if self.above_max_secs >= self.config.fault_debounce_secs {
                self.consecutive_safety_faults += 1;
                return Err(ClockDriftError::Overrun {
                    fill_ms: fill_secs * 1000.0,
                    max_ms: self.config.max_fill_secs * 1000.0,
                });
            }
            return Ok(max);
        }
        self.below_min_secs = 0.0;
        self.above_max_secs = 0.0;
        self.consecutive_safety_faults = 0;

        // Exponential smoothing with a time constant independent of the update
        // rate (the controller is driven per tile, whose duration varies with
        // the input rate).
        let alpha = if self.config.smoothing_tau_secs > 1e-9 {
            (1.0 - (-dt_seconds / self.config.smoothing_tau_secs).exp()).clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.smoothed_fill_secs = (1.0 - alpha) * self.smoothed_fill_secs + alpha * fill_secs;

        // Relative error: a low buffer yields a negative correction (consume
        // input more slowly), a high buffer yields a positive correction.
        let target = self.config.target_fill_secs.max(1e-6);
        let error = (self.smoothed_fill_secs - target) / target;

        // Integrate error with anti-windup clamp.
        self.integral_err += error * dt_seconds;
        let clamped_integral = (self.integral_err * self.config.ki).clamp(
            -self.config.integral_clamp_ppm,
            self.config.integral_clamp_ppm,
        );

        let p_term = error * self.config.kp;
        let total_ppm = (p_term + clamped_integral).clamp(-max, max);

        self.last_correction_ppm = total_ppm;
        Ok(total_ppm)
    }

    /// Returns the most recent drift correction in PPM.
    #[inline]
    pub fn last_drift_ppm(&self) -> f64 {
        self.last_correction_ppm
    }

    /// Returns the current smoothed buffered input duration in seconds.
    #[inline]
    pub fn smoothed_fill_secs(&self) -> f64 {
        self.smoothed_fill_secs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ClockDriftConfig {
        ClockDriftConfig {
            target_fill_secs: 0.080,
            kp: 1000.0,
            max_correction_ppm: 500.0,
            // Exercise the rail immediately in unit tests.
            fault_debounce_secs: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn test_drift_controller_target_stable() {
        let mut controller = AdaptiveClockDriftController::new(test_config());

        for _ in 0..100 {
            let ppm = controller.update(0.080, 0.005).unwrap();
            assert!(ppm.abs() < 1e-4);
        }
        assert!(controller.last_drift_ppm().abs() < 1e-9);
    }

    #[test]
    fn test_drift_controller_sign() {
        // Below target but above the starvation rail: consume input more slowly.
        let mut low = AdaptiveClockDriftController::new(test_config());
        let ppm = low.update(0.060, 0.005).unwrap();
        assert!(
            ppm < 0.0,
            "low buffer should request negative ppm, got {ppm}"
        );

        // Above target but below the overrun rail: consume input faster.
        let mut high = AdaptiveClockDriftController::new(test_config());
        let ppm = high.update(0.100, 0.005).unwrap();
        assert!(
            ppm > 0.0,
            "high buffer should request positive ppm, got {ppm}"
        );
    }

    #[test]
    fn test_drift_controller_bounded_ppm() {
        let mut controller = AdaptiveClockDriftController::new(test_config());

        for _ in 0..200 {
            let ppm = controller.update(0.030, 0.005).unwrap();
            assert!(ppm <= 500.0);
            assert!(ppm >= -500.0);
            assert!(ppm < 0.0);
        }
        assert_eq!(controller.last_drift_ppm(), -500.0);
    }

    #[test]
    fn test_drift_controller_starvation_fault_rail() {
        let mut controller = AdaptiveClockDriftController::new(test_config());

        let err = controller.update(0.002, 0.005).unwrap_err();
        match err {
            ClockDriftError::Starvation { fill_ms, min_ms } => {
                assert!((fill_ms - 2.0).abs() < 1e-9);
                assert!((min_ms - 10.0).abs() < 1e-9);
            }
            _ => panic!("Expected starvation error"),
        }
        assert_eq!(controller.last_drift_ppm(), -500.0);
    }

    #[test]
    fn test_drift_controller_overrun_fault_rail() {
        let mut controller = AdaptiveClockDriftController::new(test_config());

        let err = controller.update(0.300, 0.005).unwrap_err();
        match err {
            ClockDriftError::Overrun { fill_ms, max_ms } => {
                assert!((fill_ms - 300.0).abs() < 1e-9);
                assert!((max_ms - 250.0).abs() < 1e-9);
            }
            _ => panic!("Expected overrun error"),
        }
        assert_eq!(controller.last_drift_ppm(), 500.0);
    }

    #[test]
    fn test_drift_controller_fault_debounce() {
        // Default min rail is 10 ms and debounce is 50 ms. A sustained dip must
        // apply the rail correction immediately but only report a fault after
        // the debounce window.
        let mut controller = AdaptiveClockDriftController::new(ClockDriftConfig::default());
        let dt = 0.005;
        let mut fault_at = None;
        for i in 0..40 {
            match controller.update(0.002, dt) {
                Ok(ppm) => assert_eq!(ppm, -500.0, "update {i} should apply the rail"),
                Err(ClockDriftError::Starvation { .. }) => {
                    fault_at = Some(i);
                    break;
                }
                Err(other) => panic!("unexpected fault: {other:?}"),
            }
        }
        let fault_at = fault_at.expect("sustained starvation must eventually fault");
        assert!(
            (8..=12).contains(&fault_at),
            "fault should be debounced to ~10 updates, got {fault_at}"
        );

        // A brief dip that recovers must reset the debounce timer.
        let mut controller = AdaptiveClockDriftController::new(ClockDriftConfig::default());
        let _ = controller.update(0.002, dt).unwrap();
        let _ = controller.update(0.060, dt).unwrap();
        let _ = controller.update(0.002, dt).unwrap();
        assert!(controller.update(0.002, dt).unwrap().is_finite());
    }
}
