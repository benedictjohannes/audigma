#!/usr/bin/env python3
"""Polyphase rational SRC and fractional ASRC table generator for audigma-dsp (Design.md §4.2).

Generates polyphase windowed-sinc filter tables:
- Rational cross-family conversion (44.1 kHz <-> 48 kHz: ratios 160/147 and 147/160).
- Fractional ASRC drift tracking at the 1x baseband rate.
"""

import numpy as np
import scipy.signal as signal


def design_polyphase_sinc(num_phases=256, taps_per_phase=32, cutoff_ratio=0.4535, beta=12.5):
    """
    Designs a polyphase windowed-sinc filter bank using a Kaiser window.

    - num_phases: Number of fractional phase positions (e.g., 256 for sub-sample interpolation).
    - taps_per_phase: Number of filter taps evaluated per output sample.
    - cutoff_ratio: Normalized cutoff relative to Nyquist (20kHz / 44.1kHz = 0.4535).
    - beta: Kaiser window parameter for stopband attenuation (beta=12.5 gives > 120 dB rejection).
    """
    total_taps = num_phases * taps_per_phase
    # Total filter spans [-taps_per_phase/2, taps_per_phase/2] input sample periods
    t = (np.arange(total_taps) - (total_taps - 1) / 2.0) / num_phases
    # Ideal sinc
    sinc = np.sinc(2.0 * cutoff_ratio * t) * (2.0 * cutoff_ratio)
    # Kaiser window
    window = np.kaiser(total_taps, beta)
    h_proto = sinc * window

    # Normalize so each phase branch has sum == 1.0 (unity DC gain across phases)
    table = np.zeros((num_phases, taps_per_phase))
    for p in range(num_phases):
        branch = h_proto[p::num_phases]
        branch_sum = np.sum(branch)
        if branch_sum != 0:
            branch = branch / branch_sum
        table[p, :] = branch

    # Analyze prototype frequency response relative to baseband (fs=1.0 on normalized input grid)
    # The filter operates at internal rate = num_phases * fs_in.
    # Relative to fs_in (1.0), Nyquist is 0.5.
    w, H = signal.freqz(h_proto, worN=16384, fs=num_phases)
    H_db = 20 * np.log10(np.abs(H) / num_phases + 1e-18)
    # Stopband: images appear above 1.0 - cutoff_ratio
    sb_idx = np.where(w >= (1.0 - cutoff_ratio))[0]
    sb_rejection_db = float(-np.max(H_db[sb_idx])) if len(sb_idx) > 0 else 120.0

    return {
        "num_phases": num_phases,
        "taps_per_phase": taps_per_phase,
        "table": table.tolist(),
        "sb_rejection_db": sb_rejection_db,
    }


def main():
    res = design_polyphase_sinc(num_phases=256, taps_per_phase=32, cutoff_ratio=0.4535, beta=12.5)
    print(
        f"Polyphase Sinc Bank: phases={res['num_phases']}, taps/phase={res['taps_per_phase']}, "
        f"rejection={res['sb_rejection_db']:.1f} dB"
    )


if __name__ == "__main__":
    main()
