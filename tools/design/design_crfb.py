#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "numpy",
#     "scipy",
#     "numba",
# ]
# ///
"""5th- and 7th-order CRFB delta-sigma modulator design for audigma-dsp (Design.md §4.3).

Uses the exact baseband oversampling ratio OSR = f_bit / (2 * B), B = 20 kHz (Design.md §4.1).

`synthesize_crfb` builds one modulator for a bit rate, order and out-of-band NTF gain H_inf:
- NTF with 1 zero at DC (z = 1) and complex-conjugate zero pairs on the unit circle at the optimal (Legendre)
  positions in [0, 20 kHz];
- closed-loop poles from a Butterworth high-pass prototype (bilinear transform), its cutoff tuned so the NTF peaks at
  H_inf;
- canonical CRFB (Cascade of Resonators with Feedback) state-space realization via dual pole placement;
- optionally the input limit L (dBFS) from a DC + tone level sweep (`measure_msa_dbfs`).

Running the script designs every `[resampler] modulator_ntf` preset (Design.md §4.3) for every order and rate and
writes `crfb_designs.json`; `export_tables.py` turns that file into `tables.rs`. The sweep takes tens of minutes, so
the committed JSON pins the coefficients that were measured: a different numpy/scipy could move the synthesis
slightly and invalidate a stored L.
"""

import argparse
import functools
import json
import math
import os
import platform
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import numpy as np
import scipy
import scipy.signal as signal
from scipy.optimize import root_scalar

from dsm_analysis import (
    AUDIBILITY_DR_A_DB,
    AUDIBILITY_SPUR_DBFS,
    FLOOR_MARGIN_DB,
    TRANSPARENCY_NOISE_DBFS,
    TRANSPARENCY_SPUR_DBFS,
    analysis_floor_snr_db,
    analyze_components,
    analyze_record,
    feedback_gammas,
    simulate_crfb,
)


B_AUDIO_HZ = 20000.0
DESIGNS_PATH = Path(__file__).resolve().parent / "crfb_designs.json"
DESIGNS_SCHEMA = 3

ORDERS = (5, 7)
RATES = (
    ("DSD64", 2822400),
    ("DSD128", 5644800),
    ("DSD256", 11289600),
    ("DSD512", 22579200),
    ("DSD64_48", 3072000),
    ("DSD128_48", 6144000),
    ("DSD256_48", 12288000),
    ("DSD512_48", 24576000),
)

# `[resampler] modulator_ntf` presets (Design.md §4.3). The targeted presets trade in-band noise the anchors do not
# need for input limit L: they search H_inf downwards from the baseline and never above it, so L never drops below the
# baseline's.
H_INF_BASELINE = 1.40
# Below ~1.1 the Butterworth prototype can no longer reach the requested peak (7th order) and the loop degenerates.
H_INF_FLOOR = 1.10
# One step moves the in-band noise by ~1.7 dB and L by ~0.1 dB (Design.md §4.3); finer steps only select on the
# noise of the measurement.
H_INF_STEP = 0.01
# Anchors (Design.md §8.6) each targeted preset must meet, as checked by `measure_quality`.
TARGET_ANCHORS = {
    "reference": ("audibility", "transparency"),
    "comfortable": ("audibility",),
}
FIXED_H_INF = {"fixed_h_inf_1_4": 1.40, "fixed_h_inf_1_5": 1.50}
NTF_PRESETS = tuple(TARGET_ANCHORS) + tuple(FIXED_H_INF)


def optimal_zero_ratios(order):
    """Positive NTF zero frequencies (as fractions of the band edge) minimising in-band noise.

    For NTF zeros on the unit circle and a high OSR, the integrated in-band noise
    int_0^1 prod (x^2 - r_i^2)^2 dx is minimised by the roots of the Legendre polynomial
    P_order (Schreier, "Understanding Delta-Sigma Data Converters", sec. 4.3; `ds_optzeros`).
    Odd orders also have the root at 0 (the DC zero), which is not returned.
    """
    roots = np.polynomial.legendre.legroots([0.0] * order + [1.0])
    return sorted(float(r) for r in np.real(roots) if r > 1e-9)


# Maximum-stable-amplitude sweep (Design.md §4.3-§4.4). DC is the hardest case, high tones the easiest.
MSA_STIMULI_HZ = (0.0, 50.0, 1000.0, 5000.0, 10000.0, 15000.0, 20000.0)
MSA_SAMPLES = 1 << 21
MSA_STEP_DB = 0.25
MSA_START_DBFS = -6.0
# A design unstable at MSA_START_DBFS (e.g. H_inf 1.50, 7th order at DSD512) is searched downwards to this level instead.
MSA_MIN_DBFS = -20.0
# Production dithers the quantizer (TPDF, peak 0.005: `modulator::TpdfDither::default`). Near the edge, stability
# depends on the exact trajectory, so every level must hold without dither and with these dither seeds.
MSA_DITHER_PEAK = 0.005
MSA_DITHER_SEEDS = (1, 2)
# Guard band below the last level that held in every trial. At the edge itself stability is statistical (a
# trajectory with other dither can still diverge), so L is placed one grid step lower, where the production
# modulator holds reliably (checked in the modulator report, "Input limit L").
MSA_GUARD_DB = MSA_STEP_DB
# Same divergence bound as the Rust overload guard (`modulator::STABILITY_THRESHOLD`).
MSA_STATE_BOUND = 50.0

# Quality gate (Design.md §8.6) on the modulator alone, with the production records of the signal report (Design.md
# §8.3: dither on, the level sweep at the default margin plus 0 dBFS at M = 0, silence, SMPTE and CCIF). DC offsets are
# left out: the DC blocker (Design.md §4.1) removes them before the modulator. Levels are dB re the PCM full scale,
# which reaches the modulator at L - M; the cascade ahead of the modulator is linear and flat in band, so it is left out.
GATE_SAMPLES = 1 << 20
GATE_WARMUP = 1 << 17
# Default `[gain] dsd_safety_margin_db` (Design.md §4.4), the margin the signal report gates at.
GATE_MARGIN_DB = 0.25
GATE_SWEEP_DBFS = (0.0, -1.0, -3.0, -6.0, -10.0, -20.0, -40.0, -60.0, -80.0, -100.0, -120.0)
# AES17 dynamic range: A-weighted THD+N of a -60 dBFS 1 kHz tone.
GATE_DR_ID = "1k_-60"
# Two-tone records, peak sum 0 dBFS; recorded 2^20 x (f_bit / DSD64) bits so the 60 Hz sidebands stay resolved.
GATE_IMD = {
    "smpte": ((60.0, 20.0 * math.log10(0.8)), (7000.0, 20.0 * math.log10(0.2))),
    "ccif": ((19000.0, 20.0 * math.log10(0.5)), (20000.0, 20.0 * math.log10(0.5))),
}
GATE_DITHER_SEED = 12345


@functools.cache
def _loop_kernel():
    """The CRFB loop compiled with numba (imported here: tools/measure uses this module without numba)."""
    import numba

    @numba.njit(cache=True, nogil=True)
    def run_loop(order, g, gamma, b1, u, d, bound, bits):
        """Index of the first sample whose state exceeds `bound` (-1 if none). Writes the decisions to `bits` (1 = +1)
        unless it is empty. Same loop as `simulate_crfb` (`dsm_analysis.crfb_step`, same operation order)."""
        x = np.zeros(order)
        p = np.zeros(order)
        keep = bits.shape[0] > 0
        for n in range(u.shape[0]):
            v = np.copysign(1.0, x[order - 1] + d[n])
            if keep:
                bits[n] = 1 if v > 0.0 else 0
            w0 = (x[0] + b1 * u[n]) - g[0] * x[1]
            w2 = x[2] - g[1] * x[3]
            t01 = w0 + x[1]
            t23 = w2 + x[3]
            p3 = t01 + t23
            p[0] = w0
            p[1] = t01
            p[2] = t01 + w2
            p[3] = p3
            if order == 5:
                p[4] = p3 + x[4]
            else:
                w4 = x[4] - g[2] * x[5]
                t45 = w4 + x[5]
                t456 = t45 + x[6]
                p[4] = p3 + w4
                p[5] = p3 + t45
                p[6] = p3 + t456
            for i in range(order):
                x[i] = p[i] - gamma[i] * v
            for i in range(order):
                if abs(x[i]) > bound:
                    return n
        return -1

    return run_loop


def _loop_args(res):
    g = np.array([res["g1"], res["g2"], res.get("g3", 0.0)])
    return res["order"], g, np.array(res["gamma"], dtype=float), res["b1"]


def measure_msa_dbfs(res):
    """Input limit L (dBFS, MSA_STEP_DB grid): one guard step below the last level stable for every stimulus.

    For each stimulus (DC and sines of peak amplitude A) the level rises from MSA_START_DBFS in
    MSA_STEP_DB steps until the state exceeds MSA_STATE_BOUND within MSA_SAMPLES, without dither or with any
    of the MSA_DITHER_SEEDS; the stimulus' MSA is the last stable level. If MSA_START_DBFS itself is unstable, the
    level falls in MSA_STEP_DB steps to the first stable one (down to MSA_MIN_DBFS). L is the minimum over all stimuli
    minus MSA_GUARD_DB. The safety margin M is not subtracted here: the headroom inequality adds it (`Design.md §4.4`).
    """
    kernel = _loop_kernel()
    order, g, gamma, b1 = _loop_args(res)
    no_bits = np.empty(0, dtype=np.uint8)
    n = np.arange(MSA_SAMPLES)
    dithers = [np.zeros(MSA_SAMPLES)] + [
        np.random.default_rng(seed).triangular(-MSA_DITHER_PEAK, 0.0, MSA_DITHER_PEAK, MSA_SAMPLES)
        for seed in MSA_DITHER_SEEDS
    ]

    def stable(u):
        return all(kernel(order, g, gamma, b1, u, d, MSA_STATE_BOUND, no_bits) < 0 for d in dithers)

    worst = 0.0
    for f in MSA_STIMULI_HZ:
        shape = np.ones(MSA_SAMPLES) if f == 0.0 else np.sin(2.0 * np.pi * f * n / res["f_bit_hz"])
        level = MSA_START_DBFS
        if stable(10.0 ** (level / 20.0) * shape):
            while level + MSA_STEP_DB <= 0.0 and stable(10.0 ** ((level + MSA_STEP_DB) / 20.0) * shape):
                level += MSA_STEP_DB
        else:
            while True:
                level -= MSA_STEP_DB
                if level < MSA_MIN_DBFS:
                    raise ValueError(f"{res['name']} order {order}: unstable down to {MSA_MIN_DBFS} dBFS for {f} Hz")
                if stable(10.0 ** (level / 20.0) * shape):
                    break
        worst = min(worst, level)
    return float(worst - MSA_GUARD_DB)


def _gate_stimuli(f_bit_hz):
    """(id, tones [(Hz, dBFS re PCM full scale)], margin M in dB, record length) of the quality gate."""
    long_n = GATE_SAMPLES << round(math.log2(f_bit_hz / 2822400.0))
    out = [("1k_L", ((1000.0, 0.0),), 0.0, GATE_SAMPLES)]
    out += [(f"1k_{db:g}", ((1000.0, db),), GATE_MARGIN_DB, GATE_SAMPLES) for db in GATE_SWEEP_DBFS]
    out += [("silence", (), GATE_MARGIN_DB, GATE_SAMPLES)]
    out += [(sid, tones, GATE_MARGIN_DB, long_n) for sid, tones in GATE_IMD.items()]
    return out


def measure_quality(res, limit_dbfs):
    """Quality anchors (Design.md §8.6) of a modulator whose PCM full scale reaches it at `limit_dbfs` - M.

    Same reductions as the signal report's tier check: DR (A) from the -60 dBFS tone, the worst spur, noise and
    harmonic/IMD level over every record; levels within FLOOR_MARGIN_DB of the analysis floor count as that bound.
    Returns the levels, `audibility` / `transparency` (anchor met) and the 0 dBFS SINAD, or {"overload": id} if a
    record diverged.
    """
    kernel = _loop_kernel()
    order, g, gamma, b1 = _loop_args(res)
    f_bit = res["f_bit_hz"]
    recs = {}
    for sid, tones, margin, n in _gate_stimuli(f_bit):
        gain_db = limit_dbfs - margin
        t = np.arange(-GATE_WARMUP, n, dtype=float)
        u = np.zeros(len(t))
        bins = []
        for f, db in tones:
            k = int(round(f * n / f_bit))
            bins.append(k)
            u += 10.0 ** ((db + gain_db) / 20.0) * np.sin(2.0 * np.pi * k * t / n)
        d = np.random.default_rng(GATE_DITHER_SEED).triangular(-MSA_DITHER_PEAK, 0.0, MSA_DITHER_PEAK, len(t))
        bits = np.empty(len(t), dtype=np.uint8)
        if kernel(order, g, gamma, b1, u, d, MSA_STATE_BOUND, bits) >= 0:
            return {"overload": sid}
        recs[sid] = analyze_components(np.where(bits[GATE_WARMUP:] == 1, 1.0, -1.0), f_bit, bins, ref_db=gain_db)

    # Analysis floor as the signal report derives it: the synthetic record at -6 dB modulator level, re PCM full scale.
    k_1k = int(round(1000.0 * GATE_SAMPLES / f_bit))
    floor = -6.0 - analysis_floor_snr_db(res, f_bit, GATE_SAMPLES, k_1k, -6.0) - (limit_dbfs - GATE_MARGIN_DB)

    def bounded(x):
        return max(x, floor + FLOOR_MARGIN_DB)

    dr = min(-recs[GATE_DR_ID]["thdn_a_db"], -(floor + FLOOR_MARGIN_DB))
    spur = max(r["spur_db"] for r in recs.values())
    noise = max(bounded(r["noise_db"]) for r in recs.values())
    dist = max(bounded(x) for r in recs.values() for x in (r.get("harm_db"), r.get("imd_level_db")) if x is not None)
    return {
        "dr_a_db": dr,
        "spur_dbfs": spur,
        "noise_dbfs": noise,
        "distortion_dbfs": dist,
        "sinad_0dbfs_db": recs["1k_0"]["sinad_db"],
        "audibility": bool(dr >= AUDIBILITY_DR_A_DB and spur <= AUDIBILITY_SPUR_DBFS),
        "transparency": bool(
            noise <= TRANSPARENCY_NOISE_DBFS and dist <= TRANSPARENCY_NOISE_DBFS and spur <= TRANSPARENCY_SPUR_DBFS
        ),
    }


def synthesize_crfb(f_bit_hz, order=5, H_inf=1.40, name="DSD", sweep_msa=False):
    """
    Synthesizes a 5th-order or 7th-order CRFB modulator for bit rate `f_bit_hz` and target `H_inf`.

    With `sweep_msa`, also measures the maximum stable input `stable_msa_dbfs` (`measure_msa_dbfs`, slow).
    """
    if order not in (5, 7):
        raise ValueError(f"Unsupported CRFB order: {order} (only 5 and 7 supported)")

    osr = f_bit_hz / (2.0 * B_AUDIO_HZ)
    wb = np.pi / osr  # Baseband edge in rad/sample

    if order == 5:
        # 1 zero at DC and 2 conjugate pairs at the optimal (Legendre) positions
        w1, w2 = (r * wb for r in optimal_zero_ratios(5))
        zeros = np.exp(1j * np.array([0.0, w1, -w1, w2, -w2]))

        g1 = float(2.0 * (1.0 - np.cos(w1)))
        g2 = float(2.0 * (1.0 - np.cos(w2)))
        g = [g1, g2]
        c = [1.0, 1.0, 1.0, 1.0]

        L = np.eye(5)
        for i in range(4):
            L[i + 1, i] = -c[i]
        R = np.eye(5)
        R[0, 1] = -g1
        R[2, 3] = -g2
        C = np.array([[0.0, 0.0, 0.0, 0.0, 1.0]])
        B_u_unit = np.linalg.inv(L) @ np.array([1.0, 0.0, 0.0, 0.0, 0.0])

    else:  # order == 7
        # 1 zero at DC and 3 conjugate pairs at the optimal (Legendre) positions
        w1, w2, w3 = (r * wb for r in optimal_zero_ratios(7))
        zeros = np.exp(1j * np.array([0.0, w1, -w1, w2, -w2, w3, -w3]))

        g1 = float(2.0 * (1.0 - np.cos(w1)))
        g2 = float(2.0 * (1.0 - np.cos(w2)))
        g3 = float(2.0 * (1.0 - np.cos(w3)))
        g = [g1, g2, g3]
        c = [1.0] * 6

        L = np.eye(7)
        for i in range(6):
            L[i + 1, i] = -c[i]
        R = np.eye(7)
        R[0, 1] = -g1
        R[2, 3] = -g2
        R[4, 5] = -g3
        C = np.array([[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]])
        B_u_unit = np.linalg.inv(L) @ np.array([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])

    # Find Butterworth cutoff wc to achieve peak gain H_inf
    def get_peak_and_poles(wc):
        z_s, p_s, k_s = signal.butter(order, wc, btype="high", analog=True, output="zpk")
        z_d, p_d, k_d = signal.bilinear_zpk(z_s, p_s, k_s, fs=2.0)
        w, h = signal.freqz_zpk(zeros, p_d, 1.0, worN=8192)
        return np.max(np.abs(h)), p_d

    # The peak gain rises monotonically with wc over this bracket (~1.04-1.06 at 0.05 for both orders; below ~0.03 the
    # poles approach the band and it rises again), which covers H_INF_FLOOR.
    res = root_scalar(
        lambda wc: get_peak_and_poles(wc)[0] - H_inf,
        bracket=[0.05, 1.5],
        method="brentq",
    )
    peak, target_poles = get_peak_and_poles(res.root)

    # Build discrete-time state-space system:
    # L * x[n+1] = R * x[n] + b*u - a*v
    # A_ol = L^{-1} * R
    A_ol = np.linalg.inv(L) @ R

    # Dual pole placement to find feedback vector `a`
    res_poles = signal.place_poles(A_ol.T, C.T, target_poles)
    k_vec = res_poles.gain_matrix.T.flatten()
    a = (L @ k_vec).tolist()

    # Closed-loop matrix with unity quantizer gain
    A_cl = A_ol - np.outer(k_vec, C[0])
    # Compute feed-in b1 for unity DC gain (STF(1) == 1.0)
    dc_gain = (C @ np.linalg.inv(np.eye(order) - A_cl) @ B_u_unit)[0]
    b1 = float(1.0 / dc_gain)

    res_dict = {
        "name": name,
        "order": order,
        "f_bit_hz": f_bit_hz,
        "osr": osr,
        "H_inf": H_inf,
        "g1": float(g[0]),
        "g2": float(g[1]),
        "c": c,
        "a": a,
        # Feedback of v into each state of the expanded loop (prefix sums of a; the loop's coefficients).
        "gamma": feedback_gammas(a),
        "b1": b1,
    }
    if order == 7:
        res_dict["g3"] = float(g[2])
    if sweep_msa:
        res_dict["stable_msa_dbfs"] = measure_msa_dbfs(res_dict)
    res_dict["measured_snr_db"] = oracle_snr_db(res_dict)

    return res_dict


def oracle_snr_db(res):
    """In-band SNR with a coherent -6 dBFS tone and small TPDF dither (Design.md §4.3),
    analysed with the shared windowed method (a rectangular window is invalid here)."""
    order, f_bit_hz = res["order"], res["f_bit_hz"]
    N_fft = 262144
    N_warm = 65536 if order == 5 else 131072
    k_bin = int(np.round(1000.0 * N_fft / f_bit_hz))
    amp_test = 10.0 ** (-6.0 / 20.0)
    tot_N = N_warm + N_fft
    u_test = amp_test * np.sin(2 * np.pi * k_bin * np.arange(tot_N) / N_fft)
    dither = np.random.default_rng(12345).triangular(-0.005, 0.0, 0.005, size=tot_N)
    bits = simulate_crfb(res, u_test.tolist(), dither.tolist())[N_warm:]
    v_steady = np.where(bits == 1, 1.0, -1.0)
    return analyze_record(v_steady, f_bit_hz, k_bin)["snr_db"]


COEFFICIENT_KEYS = ("g1", "g2", "g3", "a", "gamma", "b1")

# Robust picks. A single MSA climb is one trajectory per dither sequence, so neighbouring H_inf differ by +-0.25-0.5 dB
# and the best of many points is biased upwards (one H_inf passed its climb, then overloaded at L with the gate's
# dither). The anchor gate has the same problem near the floor: one dither sequence per record, and DR (A) jumps by up
# to 25 dB between neighbouring H_inf there (a pick at 121.9 dB measured 118.9 dB on the production chain).
# - L: each point takes the lowest climb result within +-WINDOW_STEPS swept grid steps, then must hold at that level
#   for every MSA stimulus with each of L_CONFIRM_SEEDS and in every quality-gate record; it is lowered in MSA_STEP_DB
#   steps until it does.
# - Anchors: a point meets a preset's anchors only if every swept point within +-WINDOW_STEPS meets them too.
# - One step each side: +-2 steps gave 0.25-0.5 dB lower picks in about half the cells, yet the production input-limit
#   check (presets report: 9 dither streams at L, plus a probe one guard step above) passed at L for both windows and
#   found as many designs without that headroom (4 each), so the wider window's extra margin was not shown to be needed.
# - Every grid edge whose neighbours would only be on the lower-H_inf side gets +-WINDOW_STEPS points beyond it, so both
#   windows apply there too: the baseline (1.41, never picked by a targeted preset) and each fixed preset off the
#   targeted grid (1.49, 1.51). The climb falls as H_inf rises, so the lower side never sets the minimum: a one-sided
#   window at the top edge is inert (1.40's window equalled its own climb in all 16 cells of run 4, and 1.50's own climb
#   overloaded in production at 7th DSD128_48). The floor (1.10) needs nothing: its window extends upwards, the
#   protective side.
WINDOW_STEPS = 1
L_CONFIRM_SEEDS = tuple(range(3, 11))


def h_inf_grid():
    """Swept H_inf values: the baseline down to the floor in H_INF_STEP steps, plus the +-WINDOW_STEPS neighbours of the
    baseline and of every fixed preset."""
    steps = round((H_INF_BASELINE - H_INF_FLOOR) / H_INF_STEP)
    grid = {round(H_INF_BASELINE - i * H_INF_STEP, 2) for i in range(steps + 1)}
    grid |= {
        round(h + k * H_INF_STEP, 2)
        for h in {H_INF_BASELINE, *FIXED_H_INF.values()}
        for k in range(-WINDOW_STEPS, WINDOW_STEPS + 1)
    }
    return sorted(grid, reverse=True)


def climb_point(order, name, f_bit_hz, h_inf):
    """Phase 1 of a sweep point: the design at `h_inf` and its MSA climb (`measure_msa_dbfs`)."""
    try:
        res = synthesize_crfb(f_bit_hz, order=order, H_inf=h_inf, name=name)
        return {"H_inf": h_inf, "msa_dbfs": measure_msa_dbfs(res), "design": res}
    except ValueError as e:
        return {"H_inf": h_inf, "error": str(e)}


def window(points, h_inf):
    """The swept points within +-WINDOW_STEPS grid steps of `h_inf` (`points`: {H_inf: point}), `h_inf` included."""
    hs = (round(h_inf + k * H_INF_STEP, 2) for k in range(-WINDOW_STEPS, WINDOW_STEPS + 1))
    return [points[h] for h in hs if h in points]


def window_msa_dbfs(points, h_inf):
    """Lowest climb result in the window of `h_inf` (None if any point in it is invalid)."""
    win = window(points, h_inf)
    if any("error" in p for p in win):
        return None
    return min(p["msa_dbfs"] for p in win)


def _msa_holds(res, level_dbfs):
    """Every MSA stimulus at exactly `level_dbfs` stays within MSA_STATE_BOUND with each of L_CONFIRM_SEEDS."""
    kernel = _loop_kernel()
    order, g, gamma, b1 = _loop_args(res)
    no_bits = np.empty(0, dtype=np.uint8)
    n = np.arange(MSA_SAMPLES)
    amp = 10.0 ** (level_dbfs / 20.0)
    for seed in L_CONFIRM_SEEDS:
        d = np.random.default_rng(seed).triangular(-MSA_DITHER_PEAK, 0.0, MSA_DITHER_PEAK, MSA_SAMPLES)
        for f in MSA_STIMULI_HZ:
            u = amp * (np.ones(MSA_SAMPLES) if f == 0.0 else np.sin(2.0 * np.pi * f * n / res["f_bit_hz"]))
            if kernel(order, g, gamma, b1, u, d, MSA_STATE_BOUND, no_bits) >= 0:
                return False
    return True


def settle_point(climb, window_dbfs):
    """Phase 2 of a sweep point: L from the window value, confirmed (see WINDOW_STEPS), and the anchors at that L."""
    point = {"H_inf": climb["H_inf"]}
    if "error" in climb:
        return {**point, "error": climb["error"]}
    if window_dbfs is None:
        return {**point, "msa_dbfs": climb["msa_dbfs"], "error": "invalid design within the L window"}
    res = climb["design"]
    level = window_dbfs
    while level >= MSA_MIN_DBFS:
        if _msa_holds(res, level):
            quality = measure_quality(res, level)
            if "overload" not in quality:
                return {
                    **point,
                    "msa_dbfs": climb["msa_dbfs"],
                    "window_msa_dbfs": window_dbfs,
                    "max_stable_input_dbfs": level,
                    **quality,
                    "coefficients": {k: res[k] for k in COEFFICIENT_KEYS if k in res},
                }
        level -= MSA_STEP_DB
    return {**point, "msa_dbfs": climb["msa_dbfs"], "error": f"no confirmed level down to {MSA_MIN_DBFS} dBFS"}


def select_presets(sweep):
    """The design of every `modulator_ntf` preset from one cell's sweep (points as returned by `settle_point`).

    Targeted presets: among points at or below the baseline H_inf that meet the preset's anchors throughout their window
    (see WINDOW_STEPS) and keep L at least the baseline's, take the highest L; on that L (the MSA_STEP_DB grid makes
    plateaus), the highest H_inf, which keeps the most in-band noise shaping for no loss of L. With no such point the
    baseline is kept (`meets_target` false).
    """
    by_h = {p["H_inf"]: p for p in sweep}
    baseline = by_h[H_INF_BASELINE]
    if "error" in baseline:
        raise ValueError(f"baseline H_inf {H_INF_BASELINE} is not a valid design: {baseline}")

    def meets(p, anchors):
        return "error" not in p and all(p[a] for a in anchors)

    chosen = {}
    for preset, anchors in TARGET_ANCHORS.items():
        passing = [
            p for p in sweep
            if p["H_inf"] <= H_INF_BASELINE
            and all(meets(q, anchors) for q in window(by_h, p["H_inf"]))
            and p["max_stable_input_dbfs"] >= baseline["max_stable_input_dbfs"]
        ]
        if passing:
            best = max(p["max_stable_input_dbfs"] for p in passing)
            pick = max((p for p in passing if p["max_stable_input_dbfs"] == best), key=lambda p: p["H_inf"])
        else:
            pick = baseline
        chosen[preset] = {**pick, "meets_target": bool(passing)}
    for preset, h_inf in FIXED_H_INF.items():
        pick = by_h[h_inf]
        if "error" in pick:
            raise ValueError(f"fixed preset {preset} is not a valid design: {pick}")
        chosen[preset] = dict(pick)
    return chosen


def _parameters():
    return {
        "h_inf_baseline": H_INF_BASELINE,
        "h_inf_floor": H_INF_FLOOR,
        "h_inf_step": H_INF_STEP,
        "fixed_h_inf": FIXED_H_INF,
        "target_anchors": TARGET_ANCHORS,
        "msa": {
            "stimuli_hz": MSA_STIMULI_HZ,
            "samples": MSA_SAMPLES,
            "step_db": MSA_STEP_DB,
            "start_dbfs": MSA_START_DBFS,
            "min_dbfs": MSA_MIN_DBFS,
            "dither_peak": MSA_DITHER_PEAK,
            "dither_seeds": MSA_DITHER_SEEDS,
            "guard_db": MSA_GUARD_DB,
            "window_steps": WINDOW_STEPS,
            "confirm_seeds": L_CONFIRM_SEEDS,
            "state_bound": MSA_STATE_BOUND,
        },
        "gate": {
            "samples": GATE_SAMPLES,
            "warmup": GATE_WARMUP,
            "margin_db": GATE_MARGIN_DB,
            "sweep_dbfs": GATE_SWEEP_DBFS,
            "dr_stimulus": GATE_DR_ID,
            "imd": GATE_IMD,
            "dither_seed": GATE_DITHER_SEED,
            "audibility": {"dr_a_db": AUDIBILITY_DR_A_DB, "spur_dbfs": AUDIBILITY_SPUR_DBFS},
            "transparency": {"noise_dbfs": TRANSPARENCY_NOISE_DBFS, "spur_dbfs": TRANSPARENCY_SPUR_DBFS},
        },
    }


def _climb_job(args):
    return args, climb_point(*args)


def _settle_job(args):
    key, climb, window_dbfs = args
    return key, settle_point(climb, window_dbfs)


def design_all(jobs):
    """Sweeps every order x rate over `h_inf_grid` on `jobs` processes and selects the presets.

    Phase 1 climbs every point; phase 2 needs a point's neighbours, so it starts once phase 1 is complete.
    """
    _loop_kernel()  # compile (and cache) once before the workers start
    tasks = [(order, name, f_bit, h) for order in ORDERS for name, f_bit in RATES for h in h_inf_grid()]
    # Slowest first: high rates run the longest two-tone records, low H_inf the longest MSA climbs.
    tasks.sort(key=lambda t: (-t[2], t[3]))
    climbs, points = {}, {}
    with ProcessPoolExecutor(jobs) as ex:
        for done, (key, climb) in enumerate(ex.map(_climb_job, tasks), 1):
            climbs[key] = climb
            order, name, _, h = key
            status = climb.get("error") or f"MSA climb {climb['msa_dbfs']:6.2f}"
            print(f"[1: {done:3d}/{len(tasks)}] order {order} {name:10s} H_inf {h:.2f}: {status}", flush=True)

        def window_of(key):
            order, name, f_bit, h = key
            return window_msa_dbfs({hh: climbs[(order, name, f_bit, hh)] for hh in h_inf_grid()}, h)

        settle_tasks = [(key, climbs[key], window_of(key)) for key in tasks]
        for done, (key, point) in enumerate(ex.map(_settle_job, settle_tasks), 1):
            points[key] = point
            order, name, _, h = key
            status = point.get("error") or (
                f"L {point['max_stable_input_dbfs']:6.2f} (climb {point['msa_dbfs']:6.2f}), "
                f"DR(A) {point['dr_a_db']:6.1f}, noise {point['noise_dbfs']:7.1f}, spur {point['spur_dbfs']:7.1f}"
            )
            print(f"[2: {done:3d}/{len(tasks)}] order {order} {name:10s} H_inf {h:.2f}: {status}", flush=True)

    designs = {}
    for order in ORDERS:
        for name, f_bit in RATES:
            sweep = [points[(order, name, f_bit, h)] for h in h_inf_grid()]
            presets = select_presets(sweep)
            designs.setdefault(str(order), {})[name] = {
                "f_bit_hz": f_bit,
                "osr": f_bit / (2.0 * B_AUDIO_HZ),
                "presets": presets,
                "sweep": [{k: v for k, v in p.items() if k != "coefficients"} for p in sweep],
            }
    return designs


def load_designs(path=DESIGNS_PATH):
    """The committed designs (`crfb_designs.json`): {order (str): {rate: {"presets": {...}, "sweep": [...]}}}."""
    data = json.loads(Path(path).read_text())
    if data.get("schema") != DESIGNS_SCHEMA:
        raise ValueError(f"{path}: schema {data.get('schema')}, expected {DESIGNS_SCHEMA}; rerun design_crfb.py")
    return data["designs"]


def preset_design(designs, order, rate, preset):
    """The `synthesize_crfb`-shaped design (plus `max_stable_input_dbfs`) pinned for one preset."""
    cell = designs[str(order)][rate]
    pick = cell["presets"][preset]
    return {
        "name": rate, "order": order, "f_bit_hz": cell["f_bit_hz"], "osr": cell["osr"], "H_inf": pick["H_inf"],
        "c": [1.0] * (order - 1), **pick["coefficients"],
        "max_stable_input_dbfs": pick["max_stable_input_dbfs"],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2),
                    help="worker processes (each needs up to ~0.5 GB at DSD512)")
    ap.add_argument("--out", type=Path, default=DESIGNS_PATH)
    args = ap.parse_args()

    import numba

    designs = design_all(args.jobs)
    data = {
        "schema": DESIGNS_SCHEMA,
        "generated_by": "tools/design/design_crfb.py",
        "versions": {
            "python": platform.python_version(),
            "numpy": np.__version__,
            "scipy": scipy.__version__,
            "numba": numba.__version__,
        },
        "parameters": _parameters(),
        "designs": designs,
    }
    args.out.write_text(json.dumps(data, indent=1) + "\n")
    print(f"\nWrote {args.out}")
    for order in ORDERS:
        for name, _ in RATES:
            cell = designs[str(order)][name]["presets"]
            row = ", ".join(
                f"{p} H_inf {d['H_inf']:.2f} L {d['max_stable_input_dbfs']:.2f}"
                + ("" if d.get("meets_target", True) else " (target missed)")
                for p, d in cell.items()
            )
            print(f"order {order} {name:10s}: {row}")


if __name__ == "__main__":
    main()
