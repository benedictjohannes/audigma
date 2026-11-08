"""Bitstream analysis helpers shared by the direct and cascade sections."""

import numpy as np

from dsm_analysis import (
    B_AUDIO_HZ,
    FLOOR_MARGIN_DB,
    LOBE_HALF_WIDTH,
    analysis_window,
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
