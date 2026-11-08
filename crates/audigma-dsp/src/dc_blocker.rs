//! First-order DC blocker on the PCM ahead of the interpolation cascade (DSD targets, `Design.md §4.1`).
//!
//! A DC offset `u` at the delta-sigma modulator input produces idle tones near `u * f_bit` that the
//! TPDF dither does not remove (DSP quality audit F9). The blocker removes source DC before the cascade:
//! `y[n] = g * (x[n] - x[n-1]) + R * y[n-1]`, `R = 1 - 2*pi*fc/fs`, `g = (1 + R) / 2`, in `f64` per channel.
//! `g` makes the gain exactly 1 at Nyquist and below 1 everywhere else (unnormalised, the filter would add
//! +0.0006 dB across the band at 44.1 kHz). At `fc = 1 Hz`: -0.011 dB and +2.9 degrees at 20 Hz, under
//! 1e-5 dB from 1 kHz up; time constant `1/(2*pi*fc)` = 159 ms.

use std::f64::consts::PI;

/// Corner frequency of the production DC blocker in Hz.
pub const DC_BLOCKER_CORNER_HZ: f64 = 1.0;

/// One channel of a first-order DC-blocking high-pass filter.
#[derive(Debug, Clone)]
pub struct DcBlocker {
    r: f64,
    g: f64,
    x1: f64,
    y1: f64,
}

impl DcBlocker {
    /// Creates a blocker with corner `corner_hz` at `sample_rate_hz` (`2*pi*corner_hz < sample_rate_hz`).
    pub fn new(corner_hz: f64, sample_rate_hz: f64) -> Self {
        assert!(
            corner_hz > 0.0 && sample_rate_hz > 0.0 && 2.0 * PI * corner_hz < sample_rate_hz,
            "DC blocker corner {corner_hz} Hz is invalid at {sample_rate_hz} Hz"
        );
        let r = 1.0 - 2.0 * PI * corner_hz / sample_rate_hz;
        Self {
            r,
            g: 0.5 * (1.0 + r),
            x1: 0.0,
            y1: 0.0,
        }
    }

    /// The production blocker ([`DC_BLOCKER_CORNER_HZ`]) at `sample_rate_hz`.
    pub fn production(sample_rate_hz: f64) -> Self {
        Self::new(DC_BLOCKER_CORNER_HZ, sample_rate_hz)
    }

    /// Clears the filter history (the next output starts from silence).
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }

    /// Filters `input` into `output` (equal lengths). Zero allocation.
    #[inline]
    pub fn process(&mut self, input: &[f64], output: &mut [f64]) {
        assert_eq!(input.len(), output.len(), "DC blocker length mismatch");
        let (r, g, mut x1, mut y1) = (self.r, self.g, self.x1, self.y1);
        for (&x, y) in input.iter().zip(output.iter_mut()) {
            y1 = g * (x - x1) + r * y1;
            x1 = x;
            *y = y1;
        }
        self.x1 = x1;
        self.y1 = y1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_db_and_deg(fc: f64, fs: f64, f: f64) -> (f64, f64) {
        // H(z) = g (1 - z^-1) / (1 - R z^-1)
        let r = 1.0 - 2.0 * PI * fc / fs;
        let g = 0.5 * (1.0 + r);
        let w = 2.0 * PI * f / fs;
        let (num_re, num_im) = (1.0 - w.cos(), w.sin());
        let (den_re, den_im) = (1.0 - r * w.cos(), r * w.sin());
        let mag = g * num_re.hypot(num_im) / den_re.hypot(den_im);
        let phase = num_im.atan2(num_re) - den_im.atan2(den_re);
        (20.0 * mag.log10(), phase.to_degrees())
    }

    #[test]
    fn test_dc_is_removed() {
        for fs in [44_100.0, 48_000.0, 176_400.0, 384_000.0] {
            let mut blocker = DcBlocker::production(fs);
            let tau = 1.0 / (2.0 * PI * DC_BLOCKER_CORNER_HZ);
            let n = (25.0 * tau * fs) as usize; // 25 time constants: e^-25 = -217 dB
            let input = vec![0.5; n];
            let mut output = vec![0.0; n];
            blocker.process(&input, &mut output);
            assert!(
                output[0] <= 0.5 && output[0] > 0.4999,
                "first sample passes the step"
            );
            let last = output[n - 1].abs();
            assert!(last < 0.5 * 1e-10, "fs {fs}: residual DC {last:e}");
        }
    }

    #[test]
    fn test_response_at_20_hz() {
        for fs in [44_100.0, 48_000.0, 96_000.0, 176_400.0] {
            let (db, deg) = response_db_and_deg(DC_BLOCKER_CORNER_HZ, fs, 20.0);
            assert!(
                (-0.0112..-0.0100).contains(&db),
                "fs {fs}: {db} dB at 20 Hz"
            );
            assert!((2.8..3.0).contains(&deg), "fs {fs}: {deg} deg at 20 Hz");
            let (db_1k, _) = response_db_and_deg(DC_BLOCKER_CORNER_HZ, fs, 1000.0);
            assert!(
                (-1e-5..=0.0).contains(&db_1k),
                "fs {fs}: {db_1k} dB at 1 kHz"
            );
            let (db_nyq, _) = response_db_and_deg(DC_BLOCKER_CORNER_HZ, fs, fs / 2.0);
            assert!(db_nyq.abs() < 1e-12, "fs {fs}: {db_nyq} dB at Nyquist");
        }
    }

    #[test]
    fn test_sine_at_20_hz_matches_analytic_response() {
        let fs = 44_100.0;
        let f = 20.0;
        let mut blocker = DcBlocker::production(fs);
        let n = fs as usize * 4; // settle for >20 time constants, then measure the last 2 s
        let input: Vec<f64> = (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / fs).sin())
            .collect();
        let mut output = vec![0.0; n];
        blocker.process(&input, &mut output);
        let tail = &output[n / 2..];
        let peak = tail.iter().fold(0.0f64, |a, &v| a.max(v.abs()));
        let (db, _) = response_db_and_deg(DC_BLOCKER_CORNER_HZ, fs, f);
        let measured_db = 20.0 * peak.log10();
        assert!((measured_db - db).abs() < 1e-3, "{measured_db} vs {db}");
    }

    #[test]
    fn test_chunked_matches_whole() {
        let fs = 44_100.0;
        let input: Vec<f64> = (0..1000)
            .map(|i| 0.1 + 0.5 * (2.0 * PI * 997.0 * i as f64 / fs).sin())
            .collect();
        let mut whole = DcBlocker::production(fs);
        let mut expected = vec![0.0; input.len()];
        whole.process(&input, &mut expected);

        let mut chunked = DcBlocker::production(fs);
        let mut actual = vec![0.0; input.len()];
        for (i, o) in input.chunks(37).zip(actual.chunks_mut(37)) {
            chunked.process(i, o);
        }
        assert_eq!(expected, actual);

        chunked.reset();
        chunked.process(&input, &mut actual);
        assert_eq!(expected, actual, "reset restores the initial state");
    }
}
