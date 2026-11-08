"""Bitstream analysis helpers shared by the direct and cascade sections."""

import numpy as np

from dsm_analysis import (
    FLOOR_MARGIN_DB,
    analyze_record,
    harness_direct_sine,
    simulate_crfb,
)


def analyze_bitstream(bin_path, f_bit_hz, k_bin, n_samples):
    """Windowed coherent FFT analysis of a raw DSD bitstream (see dsm_analysis.analyze_record)."""
    bits = np.unpackbits(np.fromfile(bin_path, dtype=np.uint8))
    if len(bits) < n_samples:
        raise ValueError(f"Expected {n_samples} bits, got {len(bits)}")
    return analyze_record(np.where(bits[:n_samples] == 1, 1.0, -1.0), f_bit_hz, k_bin)


def bitexact_mismatches(bin_path, synth, n_samples, k_bin, amp_db, warmup=32768):
    """Number of bits where the Rust direct-sine record differs from the Python reference loop."""
    ref = simulate_crfb(synth, harness_direct_sine(n_samples, k_bin, amp_db, warmup))[warmup:]
    rust = np.unpackbits(np.fromfile(bin_path, dtype=np.uint8))[:n_samples]
    return int(np.count_nonzero(ref != rust))


def floor_limited(r):
    return r["snr_db"] > r["analysis_floor_db"] - FLOOR_MARGIN_DB


def fmt_snr(r):
    """SNR as measured, or as a lower bound when within FLOOR_MARGIN_DB of the analysis floor."""
    if floor_limited(r):
        return f">= {r['analysis_floor_db'] - FLOOR_MARGIN_DB:.0f} dB"
    return f"{r['snr_db']:.2f} dB"
