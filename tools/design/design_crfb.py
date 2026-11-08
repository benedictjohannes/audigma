#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "numpy",
#     "scipy",
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
- Rigorous stability verification across tone amplitudes to confirm maximum stable
  input amplitude L (dBFS) and default headroom M (dB).
"""

import numpy as np
import scipy.signal as signal
from scipy.optimize import root_scalar

from dsm_analysis import analyze_record, simulate_crfb


B_AUDIO_HZ = 20000.0


def synthesize_crfb(f_bit_hz, order=5, H_inf=1.40, name="DSD"):
    """
    Synthesizes a 5th-order or 7th-order CRFB modulator for bit rate `f_bit_hz` and target `H_inf`.
    """
    if order not in (5, 7):
        raise ValueError(f"Unsupported CRFB order: {order} (only 5 and 7 supported)")

    osr = f_bit_hz / (2.0 * B_AUDIO_HZ)
    wb = np.pi / osr  # Baseband edge in rad/sample

    if order == 5:
        # Optimal zero locations for 5th-order loop:
        # 1 zero at DC (w = 0)
        # 2 conjugate pairs at w1 and w2
        w1 = 0.6568 * wb
        w2 = 0.9429 * wb
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
        # Optimal zero locations for 7th-order loop (Schreier optimal Chebyshev zeros):
        # 1 zero at DC (w = 0)
        # 3 conjugate pairs at w1, w2, w3
        w1 = 0.40584371 * wb
        w2 = 0.74153078 * wb
        w3 = 0.94910785 * wb
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

    # Stability search across input amplitudes to determine L (dBFS)
    # Target MSA check: test from -3.0 dBFS down to -6.0 dBFS
    stable_msa = -6.0
    for amp_db in [-3.0, -3.2, -3.5, -3.8, -4.0, -4.5, -5.0, -6.0]:
        amp = 10.0 ** (amp_db / 20.0)
        N = 100000
        t = np.arange(N) / f_bit_hz
        u = amp * np.sin(2 * np.pi * 1000.0 * t)
        x = np.zeros(order)
        stable = True
        for n in range(N):
            y = x[-1]
            v = 1.0 if y >= 0 else -1.0
            if order == 5:
                x1 = x[0] - g[0] * x[1] + b1 * u[n] - a[0] * v
                x2 = x[1] + c[0] * x1 - a[1] * v
                x3 = x[2] - g[1] * x[3] + c[1] * x2 - a[2] * v
                x4 = x[3] + c[2] * x3 - a[3] * v
                x5 = x[4] + c[3] * x4 - a[4] * v
                x[0] = x1; x[1] = x2; x[2] = x3; x[3] = x4; x[4] = x5
            else:
                x1 = x[0] - g[0] * x[1] + b1 * u[n] - a[0] * v
                x2 = x[1] + c[0] * x1 - a[1] * v
                x3 = x[2] - g[1] * x[3] + c[1] * x2 - a[2] * v
                x4 = x[3] + c[2] * x3 - a[3] * v
                x5 = x[4] - g[2] * x[5] + c[3] * x4 - a[4] * v
                x6 = x[5] + c[4] * x5 - a[5] * v
                x7 = x[6] + c[5] * x6 - a[6] * v
                x[0] = x1; x[1] = x2; x[2] = x3; x[3] = x4; x[4] = x5; x[5] = x6; x[6] = x7

            if np.any(np.abs(x) > 50.0):
                stable = False
                break
        if stable:
            stable_msa = amp_db
            break

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
        "stable_msa_dbfs": stable_msa,
    }
    if order == 7:
        res_dict["g3"] = float(g[2])

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
        res = synthesize_crfb(f_bit, order=5, H_inf=h_inf, name=name)
        print(
            f"{name:10s} (f_bit={f_bit:8d}, OSR={res['osr']:6.2f}): "
            f"MSA={res['stable_msa_dbfs']:4.1f} dBFS, in-band SNR={res['measured_snr_db']:5.1f} dB"
        )
        print(f"  a = {np.array(res['a'])}")
        print(f"  b1 = {res['b1']:.8e}, g1 = {res['g1']:.8e}, g2 = {res['g2']:.8e}")

    print("\n=== 7th-Order CRFB Synthesis Results ===")
    for name, f_bit, h_inf in rates:
        res = synthesize_crfb(f_bit, order=7, H_inf=h_inf, name=name)
        print(
            f"{name:10s} (f_bit={f_bit:8d}, OSR={res['osr']:6.2f}): "
            f"MSA={res['stable_msa_dbfs']:4.1f} dBFS, in-band SNR={res['measured_snr_db']:5.1f} dB"
        )
        print(f"  a = {np.array(res['a'])}")
        print(f"  b1 = {res['b1']:.8e}, g1 = {res['g1']:.8e}, g2 = {res['g2']:.8e}, g3 = {res['g3']:.8e}")


if __name__ == "__main__":
    main()
