#!/usr/bin/env python3
"""Half-band FIR filter designer for audigma-dsp (Design.md §4.2).

Designs linear-phase half-band FIR filters for multi-stage 2x interpolation cascades:
- Stage 0 (1x -> 2x): steep transition band (20 kHz to 24.1 kHz at 88.2 kHz Fs_out).
- Stage 1+ (2x -> 4x and above): relaxed transition band (20 kHz to 68.2 kHz at 176.4 kHz Fs_out).

Guarantees half-band structural symmetry:
- Center tap = 0.5
- Even taps (at non-zero offsets from center) = 0.0
- Odd taps are symmetric: h_odd[k] == h_odd[-k]
"""

import numpy as np
import scipy.signal as signal


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
    w, H = signal.freqz(h_hb, worN=16384, fs=fs_hz)
    H_db = 20 * np.log10(np.abs(H) + 1e-18)
    pb_idx = np.where(w <= passband_hz)[0]
    sb_idx = np.where(w >= stopband_hz)[0]
    pb_ripple_db = float(np.max(np.abs(H_db[pb_idx])))
    sb_rejection_db = float(-np.max(H_db[sb_idx]))

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
        "unique_odd_taps": unique_odd_taps,
        "pb_ripple_db": pb_ripple_db,
        "sb_rejection_db": sb_rejection_db,
    }


def design_all_presets():
    # Stage 0: 44.1k -> 88.2k (Passband 20kHz, Stopband 24.1kHz, Fs=88.2k)
    stage0 = {
        "balanced": design_halfband(175, 20000.0, 24100.0, 88200.0),
        "reference": design_halfband(199, 20000.0, 24100.0, 88200.0),
        "low_latency": design_halfband(139, 20000.0, 24100.0, 88200.0),
    }

    # Stage 1+: 88.2k -> 176.4k and higher (Passband 20kHz, Stopband 68.2kHz, Fs=176.4k)
    stage1 = {
        "balanced": design_halfband(27, 20000.0, 68200.0, 176400.0),
        "reference": design_halfband(31, 20000.0, 68200.0, 176400.0),
        "low_latency": design_halfband(23, 20000.0, 68200.0, 176400.0),
    }

    print("=== Stage 0 Results ===")
    for name, res in stage0.items():
        print(
            f"{name:12s}: taps={res['numtaps']:3d}, unique_odd={len(res['unique_odd_taps']):2d}, "
            f"pb_ripple={res['pb_ripple_db']:.6f} dB, sb_rejection={res['sb_rejection_db']:.2f} dB"
        )

    print("\n=== Stage 1+ Results ===")
    for name, res in stage1.items():
        print(
            f"{name:12s}: taps={res['numtaps']:3d}, unique_odd={len(res['unique_odd_taps']):2d}, "
            f"pb_ripple={res['pb_ripple_db']:.6f} dB, sb_rejection={res['sb_rejection_db']:.2f} dB"
        )

    return {"stage0": stage0, "stage1": stage1}


if __name__ == "__main__":
    design_all_presets()
