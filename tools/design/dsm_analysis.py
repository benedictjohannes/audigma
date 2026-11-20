"""Shared delta-sigma analysis helpers for tools/design and tools/measure (Design.md §8).

Single source for:
- `analysis_window` / `analyze_record`: windowed coherent-FFT analysis of a +-1 record.
- `simulate_crfb`: Python reference of the CRFB loop (same operation order as
  `crates/audigma-dsp/src/modulator.rs`, so it is bit-exact for identical input).
- `analyze_components`: level-referenced analysis of any number of coherent tones (noise, A-weighted
  THD+N, harmonic and IMD levels, spur prominence) and the quality anchors it is gated against.
- `ntf_zp` / `analysis_floor_snr_db`: the NTF of a synthesized modulator and the highest
  in-band SNR the analysis can resolve for that NTF.

Window choice. A noise-shaped record is not periodic: at the record wrap every integrator
state is discontinuous, so a rectangular window spreads the huge out-of-band noise over all
bins. The window's far-sidelobe decay rate is what matters, not its first sidelobe level:
the 4-term Blackman-Harris window has a tiny edge step (coefficients sum to 6e-5), decays
only 6 dB/octave and floors the in-band SNR at ~145-150 dB. The Nuttall 4-term window with
continuous first derivative (-93 dB sidelobes, 18 dB/octave decay) resolves 240+ dB at
DSD256 and 250+ dB at DSD512 on synthetic records with a known in-band level.
"""

import math

import numpy as np

B_AUDIO_HZ = 20000.0
# Bins on each side of a coherent tone that belong to the window main lobe.
LOBE_HALF_WIDTH = 4
# Results closer than this to the analysis floor are reported as lower bounds.
FLOOR_MARGIN_DB = 10.0
# Quality requirements (Design.md §8.6). Levels are in-band (20 Hz-20 kHz) dBFS re a full-scale sine.
# Anchor 1, audibility: 0 dBFS at ~120 dB SPL peak, so noise and every discrete product stay below ~0 dB SPL.
AUDIBILITY_DR_A_DB = 120.0
AUDIBILITY_SPUR_DBFS = -120.0
# Anchor 2, 24-bit transparency: a TPDF-dithered 24-bit quantizer's in-band noise is -141.5 dBFS.
TRANSPARENCY_NOISE_DBFS = -141.0
TRANSPARENCY_SPUR_DBFS = -141.0

_NUTTALL_C1 = (0.355768, 0.487396, 0.144232, 0.012604)


def analysis_window(n):
    """Periodic Nuttall 4-term window, continuous first derivative (exact lobe for coherent tones)."""
    x = 2.0 * np.pi * np.arange(n) / n
    a0, a1, a2, a3 = _NUTTALL_C1
    return a0 - a1 * np.cos(x) + a2 * np.cos(2 * x) - a3 * np.cos(3 * x)


def _db(x):
    return float(10.0 * np.log10(max(x, 1e-300)))


def analyze_record(v, f_bit_hz, k_bin):
    """Windowed coherent FFT analysis of a +-1 (or any real) record with a tone on bin `k_bin`.

    The tone is coherent (integer cycles), so with a periodic cosine-sum window it occupies
    exactly +-LOBE_HALF_WIDTH bins; noise and tone share the window, so ratios need no ENBW
    correction.
    """
    n = len(v)
    w = analysis_window(n)
    p = np.abs(np.fft.rfft(v * w)) ** 2
    freqs = np.fft.rfftfreq(n, 1.0 / float(f_bit_hz))
    nyq_bin = len(p) - 1

    def lobe(center):
        return np.arange(max(center - LOBE_HALF_WIDTH, 1), min(center + LOBE_HALF_WIDTH, nyq_bin) + 1)

    tone_bins = lobe(k_bin)
    tone_power = float(np.sum(p[tone_bins]))
    # Parseval for a windowed sinusoid: one-sided lobe sum = (A/2)^2 * N * sum(w^2); power A^2/2.
    tone_dbfs = _db(2.0 * tone_power / (n * float(np.sum(w**2))))

    in_band = np.where((freqs >= 20.0) & (freqs <= B_AUDIO_HZ))[0]
    harmonic_set = set()
    h = 2
    while h * k_bin <= nyq_bin and freqs[h * k_bin] <= B_AUDIO_HZ:
        harmonic_set.update(lobe(h * k_bin).tolist())
        h += 1
    tone_set = set(tone_bins.tolist())

    harm_power = float(np.sum(p[sorted(harmonic_set)])) if harmonic_set else 1e-300
    noise_bins = np.array([b for b in in_band if b not in tone_set and b not in harmonic_set])
    noise_power = float(np.sum(p[noise_bins]))
    sinad_power = float(np.sum(p[[b for b in in_band if b not in tone_set]]))

    # Worst non-harmonic in-band spur: strongest (2*LOBE+1)-bin group of the noise bins,
    # relative to the tone (a pure tone spur fills one lobe).
    pn = np.zeros_like(p)
    pn[noise_bins] = p[noise_bins]
    sliding = np.convolve(pn, np.ones(2 * LOBE_HALF_WIDTH + 1), mode="same")
    spur_bin = int(np.argmax(sliding))

    ultra1 = (freqs > B_AUDIO_HZ) & (freqs <= 100000.0)
    return {
        "tone_dbfs": tone_dbfs,
        "snr_db": _db(tone_power / max(noise_power, 1e-300)),
        "sinad_db": _db(tone_power / max(sinad_power, 1e-300)),
        "thd_db": _db(harm_power / tone_power),
        "ultra1_db": _db(float(np.sum(p[ultra1])) / tone_power),
        "spur_dbc": _db(sliding[spur_bin] / tone_power),
        "spur_hz": float(freqs[spur_bin]),
        "floor_dbc_per_bin": _db(float(np.median(p[noise_bins])) / tone_power),
    }


def a_weighting_db(f):
    """IEC 61672 A-weighting in dB (0 dB at 1 kHz)."""
    f2 = np.asarray(f, dtype=float) ** 2
    ra = (12194.0**2 * f2**2) / (
        (f2 + 20.6**2) * np.sqrt((f2 + 107.7**2) * (f2 + 737.9**2)) * (f2 + 12194.0**2)
    )
    return 20.0 * np.log10(np.maximum(ra, 1e-300)) + 2.0


# Width (bins) of the running median that estimates the local noise floor around a candidate spur.
SPUR_LOCAL_BINS = 81
# Highest |m| + |n| of the intermodulation products m*f1 + n*f2 counted as distortion.
IMD_MAX_ORDER = 7
ULTRASONIC_BANDS_HZ = [(20_000.0, 50_000.0), (50_000.0, 100_000.0), (20_000.0, 100_000.0)]


def analyze_components(v, f_bit_hz, k_bins, ref_db=0.0):
    """Coherent analysis of a +-1 record holding any number of coherent tones (bins `k_bins`; none = silence).

    Levels are dB relative to a full-scale sine (A = 1), minus `ref_db`: pass the chain gain (L - M) to get
    levels relative to the PCM input's full scale. Same window and lobe width as `analyze_record`.

    Distortion bins: harmonics of each tone up to 20 kHz and, with two or more tones, the intermodulation
    products m*f1 + n*f2 (both non-zero, |m| + |n| <= IMD_MAX_ORDER) of the first two tones. Noise is every other
    in-band (20 Hz-20 kHz) bin outside the tone, distortion and DC lobes.

    The worst spur is the 9-bin group (one tone lobe) of noise bins with the highest *prominence*: its power over
    9 x the running median (SPUR_LOCAL_BINS noise bins) of the noise around it. Pure noise gives a few dB (see
    `spur_prominence_noise_db`); an idle tone or a residual image stands out above that.
    """
    from scipy.ndimage import median_filter

    n = len(v)
    w = analysis_window(n)
    p = np.abs(np.fft.rfft(v * w)) ** 2
    c = n * float(np.sum(w**2)) / 4.0
    freqs = np.fft.rfftfreq(n, 1.0 / float(f_bit_hz))
    nyq_bin = len(p) - 1

    def level(x):
        return 10.0 * np.log10(max(float(x), 1e-300) / c) - ref_db

    def lobe(center):
        return range(max(center - LOBE_HALF_WIDTH, 0), min(center + LOBE_HALF_WIDTH, nyq_bin) + 1)

    in_band = (freqs >= 20.0) & (freqs <= B_AUDIO_HZ)
    tones = [int(k) for k in k_bins if k > 0]
    tone_set = set()
    for k in tones:
        tone_set.update(lobe(k))
    harm_set, imd_set = set(), set()
    for k in tones:
        h = 2
        while h * k <= nyq_bin and freqs[h * k] <= B_AUDIO_HZ:
            harm_set.update(lobe(h * k))
            h += 1
    if len(tones) >= 2:
        k1, k2 = tones[0], tones[1]
        for m in range(-IMD_MAX_ORDER, IMD_MAX_ORDER + 1):
            for q in range(-IMD_MAX_ORDER, IMD_MAX_ORDER + 1):
                if m == 0 or q == 0 or abs(m) + abs(q) > IMD_MAX_ORDER:
                    continue
                b = abs(m * k1 + q * k2)
                if 0 < b <= nyq_bin and freqs[b] <= B_AUDIO_HZ:
                    imd_set.update(lobe(b))
    imd_set -= tone_set
    harm_set -= tone_set
    dist_set = harm_set | imd_set
    excluded = tone_set | dist_set | set(lobe(0))

    band_idx = np.nonzero(in_band)[0]
    noise_bins = np.array([b for b in band_idx if b not in excluded], dtype=int)
    nonfund_bins = np.array([b for b in band_idx if b not in tone_set and b not in set(lobe(0))], dtype=int)
    aw = 10.0 ** (a_weighting_db(np.maximum(freqs, 1e-9)) / 10.0)

    def band_sum(bins, weight=None):
        bins = [b for b in bins if in_band[b]]
        if not bins:
            return 1e-300
        idx = np.array(sorted(bins))
        return float(np.sum(p[idx] * (aw[idx] if weight is not None else 1.0)))

    tone_power = [float(np.sum(p[list(lobe(k))])) for k in tones]
    total_tone = sum(tone_power) if tones else 0.0
    noise = float(np.sum(p[noise_bins]))
    noise_a = float(np.sum(p[noise_bins] * aw[noise_bins]))
    thdn_a = float(np.sum(p[nonfund_bins] * aw[nonfund_bins]))
    harm = band_sum(harm_set)
    imd = band_sum(imd_set)
    has_harm = any(in_band[b] for b in harm_set)

    q = p[noise_bins]
    groups = np.convolve(q, np.ones(2 * LOBE_HALF_WIDTH + 1), mode="same")
    med = median_filter(q, size=SPUR_LOCAL_BINS, mode="nearest")
    prom = groups / ((2 * LOBE_HALF_WIDTH + 1) * np.maximum(med, 1e-300))
    # Spur centres stay one lobe inside the band: a component just outside 20 Hz-20 kHz (an out-of-band product,
    # the DC lobe) would otherwise leak into the edge bins and read as an in-band spur.
    df = freqs[1]
    inner = (freqs[noise_bins] >= 20.0 + (LOBE_HALF_WIDTH + 1) * df) & (freqs[noise_bins] <= B_AUDIO_HZ - (LOBE_HALF_WIDTH + 1) * df)
    prom = np.where(inner, prom, 0.0)
    i = int(np.argmax(prom))

    def lobe_noise(k):
        """Expected noise power inside a tone lobe: 9 x the median noise bin within SPUR_LOCAL_BINS of it."""
        near = q[np.abs(noise_bins - k) <= SPUR_LOCAL_BINS]
        return (2 * LOBE_HALF_WIDTH + 1) * float(np.median(near)) if len(near) else 0.0

    res = {
        "tone_levels_db": [level(t) for t in tone_power],
        # Tone lobe power over the noise expected inside it: bounds the tone-level reading error.
        "tone_lobe_snr_db": [10.0 * np.log10(t / max(lobe_noise(k), 1e-300)) for t, k in zip(tone_power, tones)],
        "noise_db": level(noise),
        "noise_a_db": level(noise_a),
        "thdn_a_db": level(thdn_a),
        # Windowed mean of the record (DC), dB re the DSD full scale minus ref_db (dBFS for the chain).
        "dc_db": 20.0 * np.log10(max(abs(float(np.sum(v * w)) / float(np.sum(w))), 1e-300)) - ref_db,
        "spur_db": level(groups[i]),
        "spur_hz": float(freqs[noise_bins[i]]),
        "spur_prominence_db": float(10.0 * np.log10(prom[i])),
        "ultrasonic_db": {f"{lo / 1e3:g}-{hi / 1e3:g}k": level(np.sum(p[(freqs > lo) & (freqs <= hi)]))
                          for lo, hi in ULTRASONIC_BANDS_HZ},
    }
    if tones:
        res["snr_db"] = 10.0 * np.log10(total_tone / max(noise, 1e-300))
        res["sinad_db"] = 10.0 * np.log10(total_tone / max(noise + harm + imd, 1e-300))
        res["thd_db"] = 10.0 * np.log10(max(harm, 1e-300) / total_tone) if has_harm else None
        res["imd_db"] = 10.0 * np.log10(max(imd, 1e-300) / total_tone) if len(tones) >= 2 else None
        res["harm_db"] = level(harm) if has_harm else None
        res["imd_level_db"] = level(imd) if len(tones) >= 2 else None
    return res


def spur_prominence_noise_db(n, seeds=8):
    """Highest spur prominence of `analyze_components` on pure white Gaussian noise (median over `seeds`)."""
    vals = []
    for s in range(seeds):
        rng = np.random.default_rng(1000 + s)
        # A 1 MHz rate puts the 20 Hz-20 kHz band on enough bins; only the in-band statistics matter.
        vals.append(analyze_components(rng.standard_normal(n), 2_000_000, [])["spur_prominence_db"])
    return float(np.median(vals)), float(np.max(vals))


def _gains(res):
    return [res["g1"], res["g2"]] + ([res["g3"]] if res["order"] == 7 else [])


def feedback_gammas(a):
    """Feedback of `v` into each state of the expanded loop: gamma_k = a_0 + ... + a_k, summed left to right.

    The CRFB update x' = L^-1 (R x + b1 u e_0 - a v) has L^-1 = lower-triangular ones (all c = 1), so the `v` term of
    state k is the prefix sum of `a`. Every gamma_k must be >= 0: the loop applies it as copysign(gamma_k, y).
    """
    gammas, acc = [], 0.0
    for ak in a:
        acc = acc + float(ak)
        gammas.append(acc)
    if any(gk < 0.0 for gk in gammas):
        raise ValueError(f"negative feedback gamma {gammas}: the copysign comparator requires gamma_k >= 0")
    return gammas


def crfb_step(order, g, gamma, b1, x, un, v):
    """One CRFB state update in the expanded prefix-sum form; same operation order as `modulator.rs`.

    w_k = x_k - g x_{k+1} on resonator stages (x_k elsewhere, b1 u folded into w_0), p = prefix sums of w as a depth-3
    parallel prefix, x'_k = p_k - gamma_k v. `v` is +-1, so gamma_k * v equals Rust's copysign(gamma_k, y) exactly.
    """
    w0 = (x[0] + b1 * un) - g[0] * x[1]
    w1 = x[1]
    w2 = x[2] - g[1] * x[3]
    w3 = x[3]
    t01 = w0 + w1
    t23 = w2 + w3
    p3 = t01 + t23
    if order == 5:
        p = (w0, t01, t01 + w2, p3, p3 + x[4])
    else:
        w4 = x[4] - g[2] * x[5]
        w5 = x[5]
        w6 = x[6]
        t45 = w4 + w5
        t456 = t45 + w6
        p = (w0, t01, t01 + w2, p3, p3 + w4, p3 + t45, p3 + t456)
    return [pk - gk * v for pk, gk in zip(p, gamma)]


def simulate_crfb(res, u, dither=None):
    """Reference CRFB loop; returns bits (uint8, 1 = +1). Same operation order as modulator.rs (bit-exact).

    Comparator: v = copysign(1, y), so y = -0.0 decides -1 like the Rust sign-bit comparator.
    `u` and `dither` are sequences of floats; `dither` is added to the quantizer input only.
    Pure Python (about 1 us/sample).
    """
    order = res["order"]
    gamma, b1, g = res["gamma"], res["b1"], _gains(res)
    x = [0.0] * order
    out = np.empty(len(u), dtype=np.uint8)
    for n, un in enumerate(u):
        y = x[-1] if dither is None else x[-1] + dither[n]
        v = math.copysign(1.0, y)
        out[n] = v > 0.0
        x = crfb_step(order, g, gamma, b1, x, un, v)
    return out


def harness_direct_sine(n, k_bin, amp_db, warmup=32768):
    """Input sequence of `measure_harness --mode direct-sine` (warm-up included), bit-identical.

    Uses `math.sin` (libm), matching Rust's `f64::sin` on Linux, and the same expression order.
    """
    amp = 10.0 ** (amp_db / 20.0)
    return [amp * math.sin(2.0 * math.pi * float(k_bin) * float(i) / float(n)) for i in range(-warmup, n)]


def ntf_zp(res):
    """NTF zeros and poles of a synthesized CRFB (`synthesize_crfb` result), from its state space."""
    order = res["order"]
    L = np.eye(order)
    R = np.eye(order)
    for i in range(order - 1):
        L[i + 1, i] = -1.0
    for j, gg in enumerate(_gains(res)):
        R[2 * j, 2 * j + 1] = -gg
    a_ol = np.linalg.inv(L) @ R
    k_vec = np.linalg.inv(L) @ np.array(res["a"])
    c = np.zeros(order)
    c[-1] = 1.0
    return np.linalg.eigvals(a_ol), np.linalg.eigvals(a_ol - np.outer(k_vec, c))


def analysis_floor_snr_db(res, f_bit_hz, n, k_bin, amp_db, seed=1):
    """Highest in-band SNR `analyze_record` resolves for this modulator's out-of-band noise.

    Builds a non-periodic synthetic record: a 4N-periodic Gaussian noise with the NTF's
    shape (scaled to the power a 1-bit stream leaves besides the tone), an in-band level
    set 300 dB below the tone, cut to N samples, plus the coherent tone. The SNR measured on
    it is the leakage floor of the method for this case.
    """
    m = 4 * n
    rng = np.random.default_rng(seed)
    zeros, poles = ntf_zp(res)
    f = np.fft.rfftfreq(m, 1.0 / f_bit_hz)
    z = np.exp(2j * np.pi * f / f_bit_hz)
    h2 = np.abs(np.prod([z - q for q in zeros], 0) / np.prod([z - q for q in poles], 0)) ** 2
    amp = 10.0 ** (amp_db / 20.0)
    h2 *= (1.0 - amp * amp / 2.0) / h2.sum()
    in_band = (f >= 20.0) & (f <= B_AUDIO_HZ)
    h2[in_band] = (amp * amp / 2.0) * 1e-30 / in_band.sum()
    # rfft scaling: one-sided bin power = 2|X|^2 / M^2.
    spec = np.sqrt(h2 * m * m / 2.0) * np.exp(2j * np.pi * rng.random(len(h2)))
    spec[0] = 0.0
    spec[-1] = 0.0
    x = np.fft.irfft(spec, m)[:n] + amp * np.sin(2.0 * np.pi * k_bin * np.arange(n) / n)
    return analyze_record(x, f_bit_hz, k_bin)["snr_db"]
