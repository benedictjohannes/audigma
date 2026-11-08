#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "numpy",
#     "scipy",
# ]
# ///
"""Polyphase sample-rate converter design for audigma-dsp (Design.md §4.2).

One closed-form prototype serves every ratio (rational 44.1k <-> 48k at any tier, and the
adaptive ASRC around any nominal ratio). Rust evaluates the same formula when a resampler is
constructed (`audigma_dsp::fir::src_prototype`); this script is the source of the design
parameters (exported to `tables.rs` by `export_tables.py`) and the reference the measurement
tool compares the compiled banks against.

Prototype, in units of input samples t, for a converter with band-limit scale
s = min(1, out_rate / in_rate):

    h(t) = 2 fc sinc(2 fc t) * kaiser(t),   fc = SRC_CUTOFF * s,
    kaiser(t) = I0(beta * sqrt(1 - (2 t / T)^2)) / I0(beta)  for |t| <= T/2, else 0,
    T = taps per output sample = 2 * ceil(SRC_TAPS_UNITY / (2 s)).

The band edges are relative to the *lower* of the two rates (so a downsampler is also an
anti-alias filter): passband to 20 kHz and stopband from 24.1 kHz at 44.1 kHz, i.e. 0.4535 and
0.5465 of the lower rate; the cutoff is half-way (0.5). What lands in 20-22.05 kHz from the
transition band is outside the audio band, the same convention as the half-band cascade.

Bank row for fractional output position phi in [0, 1) (phi = fraction of an input sample past
the newest-but-T/2 sample, see `PolyphaseResampler::process`):

    row(phi)[i] = h(i - T/2 + phi) / sum_j h(j - T/2 + phi),   i = 0..T-1

Each row is normalised to unity DC gain.
"""

import math

import numpy as np

# 20 kHz passband edge and its mirror image (24.1 kHz) at 44.1 kHz, relative to the lower rate.
SRC_PASS_EDGE = 20000.0 / 44100.0
SRC_STOP_EDGE = 1.0 - SRC_PASS_EDGE
SRC_CUTOFF = 0.5
# Stopband attenuation target (dB) of the prototype: equal to the `reference` cascade goal.
SRC_ATTEN_DB = 140.0
# Phases of the ASRC bank; the position between two phases is cubic (4-point Lagrange) interpolated.
SRC_ASRC_PHASES = 256


def kaiser_beta(atten_db):
    """Kaiser's empirical beta for a given stopband attenuation (> 50 dB)."""
    return 0.1102 * (atten_db - 8.7)


SRC_KAISER_BETA = kaiser_beta(SRC_ATTEN_DB)


def bessel_i0(x):
    """Modified Bessel function I0 by its power series (same summation as the Rust implementation)."""
    half = 0.5 * x
    term = 1.0
    total = 1.0
    k = 1
    while True:
        term *= (half / k) ** 2
        total += term
        if term < total * 1e-17:
            return total
        k += 1


def taps_for_scale(scale, taps_unity):
    """Taps per output sample for band-limit scale `scale` (even, so the window is symmetric about T/2)."""
    return 2 * math.ceil(taps_unity / (2.0 * scale) - 1e-9)


def prototype(t, scale, taps, beta=None):
    """Closed-form prototype h(t) (t in input samples, any array shape)."""
    beta = SRC_KAISER_BETA if beta is None else beta
    t = np.asarray(t, dtype=float)
    fc = SRC_CUTOFF * scale
    half = taps / 2.0
    r = 1.0 - (t / half) ** 2
    i0b = bessel_i0(beta)
    win = np.array([bessel_i0(beta * math.sqrt(v)) / i0b if v >= 0.0 else 0.0 for v in r.ravel()]).reshape(t.shape)
    return 2.0 * fc * np.sinc(2.0 * fc * t) * win


def bank_row(phi, scale, taps, beta=None):
    """One normalised bank row for fractional position `phi` (see module docstring)."""
    h = prototype(np.arange(taps) - taps / 2.0 + phi, scale, taps, beta)
    return h / np.sum(h)


def prototype_response(scale, taps, beta=None, oversample=64, n_fft=1 << 20):
    """Frequency response (freqs in units of the input rate, magnitude) of the continuous prototype."""
    t = np.arange(-taps * oversample // 2, taps * oversample // 2 + 1) / oversample
    h = prototype(t, scale, taps, beta) / oversample
    hf = np.abs(np.fft.rfft(h, n_fft))
    f = np.fft.rfftfreq(n_fft, 1.0 / oversample)
    return f, hf


def response_metrics(scale, taps, beta=None):
    """Passband deviation (dB, 0..pass edge) and stopband rejection (dB, from stop edge) of the prototype."""
    f, hf = prototype_response(scale, taps, beta)
    pb = f <= SRC_PASS_EDGE * scale
    sb = f >= SRC_STOP_EDGE * scale
    hdb = 20.0 * np.log10(np.maximum(hf, 1e-300))
    return {
        "passband_dev_db": float(np.max(np.abs(hdb[pb]))),
        "stopband_rejection_db": float(-np.max(hdb[sb])),
    }


def design_unity_taps(atten_db=SRC_ATTEN_DB):
    """Smallest even tap count whose prototype meets `atten_db` stopband rejection at scale 1."""
    beta = kaiser_beta(atten_db)
    taps = 2 * math.ceil((atten_db - 7.95) / (2.285 * 2.0 * math.pi * (SRC_STOP_EDGE - SRC_PASS_EDGE)) / 2.0) - 8
    while response_metrics(1.0, taps, beta)["stopband_rejection_db"] < atten_db:
        taps += 2
    return taps


def design_src():
    """Design parameters plus the measured prototype metrics (exported to tables.rs)."""
    taps = design_unity_taps()
    m = response_metrics(1.0, taps)
    return {
        "cutoff": SRC_CUTOFF,
        "pass_edge": SRC_PASS_EDGE,
        "stop_edge": SRC_STOP_EDGE,
        "atten_db": SRC_ATTEN_DB,
        "beta": SRC_KAISER_BETA,
        "taps_unity": taps,
        "asrc_phases": SRC_ASRC_PHASES,
        **m,
    }


def main():
    d = design_src()
    print(
        f"SRC prototype: taps(scale 1)={d['taps_unity']}, beta={d['beta']:.4f}, "
        f"passband dev={d['passband_dev_db']:.2e} dB, rejection={d['stopband_rejection_db']:.1f} dB"
    )
    for label, s in (("48k->44.1k", 147 / 160), ("96k->44.1k", 147 / 320)):
        t = taps_for_scale(s, d["taps_unity"])
        m = response_metrics(s, t)
        print(f"  {label}: taps={t}, passband dev={m['passband_dev_db']:.2e} dB, rejection={m['stopband_rejection_db']:.1f} dB")


if __name__ == "__main__":
    main()
