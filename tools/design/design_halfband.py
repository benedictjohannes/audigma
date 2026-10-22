#!/usr/bin/env python3
"""Half-band FIR filter designer for audigma-dsp (Design.md §4.2).

Designs the coefficient families used by the multi-stage 2x interpolation cascade:

* Stage 0 (1x -> 2x): steep anti-imaging transition from the audio passband to
  22.05 kHz (the source Nyquist), at an 88.2 kHz output rate.
* Stage 1+ (2x -> 4x and above): relaxed transition from 20 kHz to 68.2 kHz at
  176.4 kHz output rate.

For every preset the following phase/apodization permutations are synthesized:

* ``(Linear, Off)``: the original symmetric half-band prototype.  Even taps away
  from the centre are zero, the centre tap is 0.5, and odd taps are symmetric.
* ``(Linear, Gentle)``: a Kaiser-windowed linear-phase low-pass with an earlier,
  smoother roll-off starting in the 17.5-18 kHz region.  This is exported as a
  general (non-half-band) polyphase filter.
* ``(Minimum, Off)``: the homomorphic minimum-phase equivalent of the
  ``(Linear, Off)`` prototype (same magnitude response, causal energy).
* ``(Minimum, Gentle)``: the homomorphic minimum-phase equivalent of the
  gentle-apodized prototype.

Guarantees for the symmetric half-band prototypes:
- Center tap = 0.5
- Even taps (at non-zero offsets from center) = 0.0
- Odd taps are symmetric: h_odd[k] == h_odd[-k]

All full-length filters are renormalized to exact unity DC gain (sum(h) == 1).
"""

import numpy as np
import scipy.signal as signal

# FFT size used by the homomorphic cepstral minimum-phase transform.  The plan
# requires >= 65536 so that numerical precision survives down to a 140 dB
# stopband floor.
MIN_PHASE_FFT_SIZE = 65536

# Gentle apodization: move the roll-off knee down into the 17.5-18 kHz region
# and reach the full stopband at the source Nyquist (22.05 kHz).  The resulting
# transition is wider than the brick-wall (20 -> 24.1 kHz) prototype, which
# suppresses the long impulse-response tail while retaining the required
# anti-image rejection.
GENTLE_PASSBAND_HZ = 17_500.0
GENTLE_STOPBAND_HZ = 22_050.0

# Requested Kaiser attenuation per preset.  A few dB of margin is added above
# the contractual 100/120/140 dB goals to absorb window/Remez measurement
# differences and floating-point round-off.
GENTLE_ATTENUATION_DB = {
    "low_latency": 104.0,
    "balanced": 124.0,
    "reference": 144.0,
}

# Audio passband used for spectral measurements (Design.md §4.1).
MEASURE_PASSBAND_HZ = 20_000.0


def _measure_response(h, fs_hz, passband_hz, stopband_hz):
    """Returns (max passband deviation dB, stopband rejection dB)."""
    w, response = signal.freqz(h, worN=1 << 17, fs=fs_hz)
    response_db = 20.0 * np.log10(np.abs(response) + 1e-30)
    passband = response_db[w <= passband_hz]
    stopband = response_db[w >= stopband_hz]
    return float(np.max(np.abs(passband))), float(-np.max(stopband))


def design_halfband(numtaps, passband_hz, stopband_hz, fs_hz):
    """Designs an exact half-band filter using Remez with even-tap nulling."""
    assert numtaps % 2 == 1, "Filter length must be odd"
    h = signal.remez(
        numtaps,
        [0, passband_hz, stopband_hz, fs_hz / 2.0],
        [1, 0],
        weight=[1, 1],
        fs=fs_hz,
    )
    center = (numtaps - 1) // 2
    h_hb = np.copy(h)
    # Zero out all even taps away from center
    for k in range(1, (numtaps - center) // 2 + 1):
        if center + 2 * k < numtaps:
            h_hb[center + 2 * k] = 0.0
            h_hb[center - 2 * k] = 0.0
    # Set center tap to 0.5
    h_hb[center] = 0.5
    # Normalize so DC gain is exactly 1.0 (sum of all taps == 1.0)
    # Since center is 0.5, sum of odd taps must be 0.5
    odd_sum = np.sum(h_hb) - 0.5
    if odd_sum != 0:
        odd_indices = [
            i for i in range(numtaps) if (i - center) % 2 != 0
        ]
        h_hb[odd_indices] *= 0.5 / odd_sum

    # Measure performance
    pb_ripple_db, sb_rejection_db = _measure_response(
        h_hb, fs_hz, passband_hz, stopband_hz
    )

    # Extract unique positive-offset odd taps: h_odd[1], h_odd[3], ...
    # from center + 1 up to center + (2*M - 1)
    unique_odd_taps = []
    k = 1
    while center + k < numtaps:
        unique_odd_taps.append(float(h_hb[center + k]))
        k += 2

    return {
        "numtaps": numtaps,
        "h_full": h_hb.tolist(),
        "full_taps": h_hb.tolist(),
        "unique_odd_taps": unique_odd_taps,
        "passband_hz": passband_hz,
        "stopband_hz": stopband_hz,
        "pb_ripple_db": pb_ripple_db,
        "sb_rejection_db": sb_rejection_db,
    }


def minimum_phase_filter(h, n_fft=MIN_PHASE_FFT_SIZE):
    """Returns the causal minimum-phase equivalent of a linear-phase FIR.

    ``half=False`` keeps the original magnitude response (including its
    stopband rejection); the default ``half=True`` would return a square-root
    magnitude response and halve the achievable attenuation in dB.
    """
    h = np.asarray(h, dtype=np.float64)
    h_min = signal.minimum_phase(
        h, method="homomorphic", n_fft=n_fft, half=False
    )
    h_min = np.asarray(h_min, dtype=np.float64)
    dc_gain = float(np.sum(h_min))
    if abs(dc_gain) < 1e-15:
        raise ValueError("minimum-phase transform produced zero DC gain")
    return h_min / dc_gain


def design_gentle(preset_name, fs_hz=88_200.0):
    """Designs a gentle-apodized linear-phase Stage 0 low-pass.

    A Kaiser window provides a smooth, monotonically decaying time-domain
    envelope (low far-out Gibbs ripple) while the requested attenuation margin
    keeps the stopband rejection at or above the preset contract.
    """
    attenuation_db = GENTLE_ATTENUATION_DB[preset_name]
    # scipy's kaiserord expects the transition width normalized to Nyquist.
    width = (GENTLE_STOPBAND_HZ - GENTLE_PASSBAND_HZ) / (fs_hz / 2.0)
    numtaps, beta = signal.kaiserord(attenuation_db, width)
    if numtaps % 2 == 0:
        numtaps += 1
    cutoff_hz = 0.5 * (GENTLE_PASSBAND_HZ + GENTLE_STOPBAND_HZ)
    h = signal.firwin(
        numtaps, cutoff_hz, window=("kaiser", beta), fs=fs_hz
    )
    h = np.asarray(h, dtype=np.float64)
    h /= np.sum(h)

    pb_ripple_db, sb_rejection_db = _measure_response(
        h, fs_hz, GENTLE_PASSBAND_HZ, GENTLE_STOPBAND_HZ
    )

    return {
        "numtaps": int(numtaps),
        "full_taps": h.tolist(),
        "passband_hz": GENTLE_PASSBAND_HZ,
        "stopband_hz": GENTLE_STOPBAND_HZ,
        "pb_ripple_db": pb_ripple_db,
        "sb_rejection_db": sb_rejection_db,
    }


def _minimum_phase_result(source, fs_hz, n_fft=MIN_PHASE_FFT_SIZE):
    """Builds a table record for the minimum-phase version of ``source``."""
    h_min = minimum_phase_filter(source["full_taps"], n_fft=n_fft)
    pb_ripple_db, sb_rejection_db = _measure_response(
        h_min,
        fs_hz,
        source["passband_hz"],
        source["stopband_hz"],
    )
    return {
        "numtaps": len(h_min),
        "full_taps": h_min.tolist(),
        "passband_hz": source["passband_hz"],
        "stopband_hz": source["stopband_hz"],
        "pb_ripple_db": pb_ripple_db,
        "sb_rejection_db": sb_rejection_db,
    }


def _verify_mode(label, result, ripple_limit_db, rejection_limit_db):
    if result["pb_ripple_db"] > ripple_limit_db:
        raise RuntimeError(
            f"{label}: passband deviation {result['pb_ripple_db']:.6f} dB "
            f"exceeds {ripple_limit_db} dB"
        )
    if result["sb_rejection_db"] < rejection_limit_db:
        raise RuntimeError(
            f"{label}: stopband rejection {result['sb_rejection_db']:.2f} dB "
            f"is below {rejection_limit_db} dB"
        )


def design_all_presets():
    # Stage 0: 44.1k -> 88.2k (Passband 20kHz, Stopband 24.1kHz, Fs=88.2k)
    stage0_linear_off = {
        "balanced": design_halfband(175, 20000.0, 24100.0, 88200.0),
        "reference": design_halfband(199, 20000.0, 24100.0, 88200.0),
        "low_latency": design_halfband(139, 20000.0, 24100.0, 88200.0),
    }

    # Stage 1+: 88.2k -> 176.4k and higher (Passband 20kHz, Stopband 68.2kHz, Fs=176.4k)
    stage1_linear_off = {
        "balanced": design_halfband(27, 20000.0, 68200.0, 176400.0),
        "reference": design_halfband(31, 20000.0, 68200.0, 176400.0),
        "low_latency": design_halfband(23, 20000.0, 68200.0, 176400.0),
    }

    ripple_limits = {
        "balanced": 0.01,
        "reference": 0.001,
        "low_latency": 0.02,
    }
    rejection_limits = {
        "balanced": 120.0,
        "reference": 140.0,
        "low_latency": 100.0,
    }

    stage0 = {}
    for name, linear_off in stage0_linear_off.items():
        gentle_linear = design_gentle(name)
        gentle_minimum = _minimum_phase_result(gentle_linear, 88200.0)
        minimum_off = _minimum_phase_result(linear_off, 88200.0)

        _verify_mode(
            f"stage0/{name}/linear_off",
            linear_off,
            ripple_limits[name],
            rejection_limits[name],
        )
        _verify_mode(
            f"stage0/{name}/linear_gentle",
            gentle_linear,
            ripple_limits[name],
            rejection_limits[name],
        )
        _verify_mode(
            f"stage0/{name}/minimum_off",
            minimum_off,
            ripple_limits[name],
            rejection_limits[name],
        )
        _verify_mode(
            f"stage0/{name}/minimum_gentle",
            gentle_minimum,
            ripple_limits[name],
            rejection_limits[name],
        )

        stage0[name] = {
            "linear_off": linear_off,
            "linear_gentle": gentle_linear,
            "minimum_off": minimum_off,
            "minimum_gentle": gentle_minimum,
        }

    stage1 = {}
    for name, linear_off in stage1_linear_off.items():
        minimum_off = _minimum_phase_result(linear_off, 176400.0)
        _verify_mode(
            f"stage1/{name}/linear_off",
            linear_off,
            ripple_limits[name],
            rejection_limits[name],
        )
        _verify_mode(
            f"stage1/{name}/minimum_off",
            minimum_off,
            ripple_limits[name],
            rejection_limits[name],
        )
        stage1[name] = {
            "linear_off": linear_off,
            "minimum_off": minimum_off,
        }

    print("=== Stage 0 Results ===")
    for name, modes in stage0.items():
        for mode, res in modes.items():
            print(
                f"{name:12s} {mode:15s}: taps={res['numtaps']:3d}, "
                f"pb_ripple={res['pb_ripple_db']:.7f} dB, "
                f"sb_rejection={res['sb_rejection_db']:.2f} dB"
            )

    print("\n=== Stage 1+ Results ===")
    for name, modes in stage1.items():
        for mode, res in modes.items():
            print(
                f"{name:12s} {mode:15s}: taps={res['numtaps']:3d}, "
                f"pb_ripple={res['pb_ripple_db']:.7f} dB, "
                f"sb_rejection={res['sb_rejection_db']:.2f} dB"
            )

    return {"stage0": stage0, "stage1": stage1}


if __name__ == "__main__":
    design_all_presets()
