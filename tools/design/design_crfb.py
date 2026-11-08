#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "numpy",
#     "scipy",
#     "numba",
# ]
# ///
"""5th-Order and 7th-Order CRFB Delta-Sigma Modulator NTF synthesis & coefficient generator for audigma-dsp (Design.md §4.3).

Uses exact baseband Over-Sampling Ratio:
    OSR = f_bit / (2 * B)  where B = 20,000 Hz (Design.md §4.1)

Synthesizes:
- 5th-order and 7th-order NTF with 1 zero at DC (z = 1) and complex-conjugate zero pairs
  on the unit circle optimized across [0, 20 kHz].
- Closed-loop poles parameterized by Butterworth prototype mapped via bilinear transform,
  tuned to target out-of-band gain H_inf (typically ~1.40).
- Canonical CRFB (Cascade of Resonators with Feedback) state-space realization
  via dual pole placement.
- Maximum stable input amplitude L (dBFS) from a DC + tone level sweep (`measure_msa_dbfs`),
  exported per rate and order; the headroom inequality adds the safety margin M.
"""

import numpy as np
import scipy.signal as signal
from scipy.optimize import root_scalar

from dsm_analysis import analyze_record, simulate_crfb


B_AUDIO_HZ = 20000.0


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


def _msa_kernel():
    import numba

    @numba.njit(cache=True)
    def first_divergence(order, g, a, b1, u, d, bound):
        """Index of the first sample whose state exceeds `bound` (-1 if none); same loop as `simulate_crfb`."""
        x = np.zeros(order)
        for n in range(u.shape[0]):
            v = 1.0 if x[order - 1] + d[n] >= 0.0 else -1.0
            x[0] = x[0] - g[0] * x[1] + b1 * u[n] - a[0] * v
            x[1] = x[1] + x[0] - a[1] * v
            x[2] = x[2] - g[1] * x[3] + x[1] - a[2] * v
            x[3] = x[3] + x[2] - a[3] * v
            if order == 5:
                x[4] = x[4] + x[3] - a[4] * v
            else:
                x[4] = x[4] - g[2] * x[5] + x[3] - a[4] * v
                x[5] = x[5] + x[4] - a[5] * v
                x[6] = x[6] + x[5] - a[6] * v
            for i in range(order):
                if abs(x[i]) > bound:
                    return n
        return -1

    return first_divergence


def measure_msa_dbfs(res):
    """Input limit L (dBFS, MSA_STEP_DB grid): one guard step below the last level stable for every stimulus.

    For each stimulus (DC and sines of peak amplitude A) the level rises from MSA_START_DBFS in
    MSA_STEP_DB steps until the state exceeds MSA_STATE_BOUND within MSA_SAMPLES, without dither or with any
    of the MSA_DITHER_SEEDS; the stimulus' MSA is the last stable level. L is the minimum over all stimuli
    minus MSA_GUARD_DB. The safety margin M is not subtracted here: the headroom inequality adds it (`Design.md §4.4`).
    """
    kernel = _msa_kernel()
    order = res["order"]
    g = np.array([res["g1"], res["g2"], res.get("g3", 0.0)])
    a = np.array(res["a"], dtype=float)
    n = np.arange(MSA_SAMPLES)
    dithers = [np.zeros(MSA_SAMPLES)] + [
        np.random.default_rng(seed).triangular(-MSA_DITHER_PEAK, 0.0, MSA_DITHER_PEAK, MSA_SAMPLES)
        for seed in MSA_DITHER_SEEDS
    ]

    def stable(u):
        return all(kernel(order, g, a, res["b1"], u, d, MSA_STATE_BOUND) < 0 for d in dithers)

    worst = 0.0
    for f in MSA_STIMULI_HZ:
        shape = np.ones(MSA_SAMPLES) if f == 0.0 else np.sin(2.0 * np.pi * f * n / res["f_bit_hz"])
        level = MSA_START_DBFS
        if not stable(10.0 ** (level / 20.0) * shape):
            raise ValueError(f"{res['name']} order {order}: unstable at {level} dBFS for {f} Hz")
        while level + MSA_STEP_DB <= 0.0 and stable(10.0 ** ((level + MSA_STEP_DB) / 20.0) * shape):
            level += MSA_STEP_DB
        worst = min(worst, level)
    return float(worst - MSA_GUARD_DB)


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

    res = root_scalar(
        lambda wc: get_peak_and_poles(wc)[0] - H_inf,
        bracket=[0.1, 1.5],
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
        "b1": b1,
    }
    if order == 7:
        res_dict["g3"] = float(g[2])
    if sweep_msa:
        res_dict["stable_msa_dbfs"] = measure_msa_dbfs(res_dict)

    # In-band SNR with a coherent -6 dBFS tone and small TPDF dither (Design.md §4.3),
    # analysed with the shared windowed method (a rectangular window is invalid here).
    N_fft = 262144
    N_warm = 65536 if order == 5 else 131072
    k_bin = int(np.round(1000.0 * N_fft / f_bit_hz))
    amp_test = 10.0 ** (-6.0 / 20.0)
    tot_N = N_warm + N_fft
    u_test = amp_test * np.sin(2 * np.pi * k_bin * np.arange(tot_N) / N_fft)
    dither = np.random.default_rng(12345).triangular(-0.005, 0.0, 0.005, size=tot_N)
    bits = simulate_crfb(res_dict, u_test.tolist(), dither.tolist())[N_warm:]
    v_steady = np.where(bits == 1, 1.0, -1.0)
    res_dict["measured_snr_db"] = analyze_record(v_steady, f_bit_hz, k_bin)["snr_db"]

    return res_dict


def main():
    rates = [
        ("DSD64", 2822400, 1.40),
        ("DSD128", 5644800, 1.40),
        ("DSD256", 11289600, 1.40),
        ("DSD512", 22579200, 1.40),
        ("DSD64_48", 3072000, 1.40),
        ("DSD128_48", 6144000, 1.40),
        ("DSD256_48", 12288000, 1.40),
        ("DSD512_48", 24576000, 1.40),
    ]

    print("=== 5th-Order CRFB Synthesis Results ===")
    for name, f_bit, h_inf in rates:
        res = synthesize_crfb(f_bit, order=5, H_inf=h_inf, name=name, sweep_msa=True)
        print(
            f"{name:10s} (f_bit={f_bit:8d}, OSR={res['osr']:6.2f}): "
            f"MSA={res['stable_msa_dbfs']:4.1f} dBFS, in-band SNR={res['measured_snr_db']:5.1f} dB"
        )
        print(f"  a = {np.array(res['a'])}")
        print(f"  b1 = {res['b1']:.8e}, g1 = {res['g1']:.8e}, g2 = {res['g2']:.8e}")

    print("\n=== 7th-Order CRFB Synthesis Results ===")
    for name, f_bit, h_inf in rates:
        res = synthesize_crfb(f_bit, order=7, H_inf=h_inf, name=name, sweep_msa=True)
        print(
            f"{name:10s} (f_bit={f_bit:8d}, OSR={res['osr']:6.2f}): "
            f"MSA={res['stable_msa_dbfs']:4.1f} dBFS, in-band SNR={res['measured_snr_db']:5.1f} dB"
        )
        print(f"  a = {np.array(res['a'])}")
        print(f"  b1 = {res['b1']:.8e}, g1 = {res['g1']:.8e}, g2 = {res['g2']:.8e}, g3 = {res['g3']:.8e}")


if __name__ == "__main__":
    main()
