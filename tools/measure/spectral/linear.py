"""Analysis of the linear paths (half-band cascade, polyphase resamplers).

The Rust harness (`measure_harness`, `linear_modes.rs`) dumps raw little-endian f64 samples
produced by the production types; everything here is analysis only.
"""

import numpy as np

from dsm_analysis import LOBE_HALF_WIDTH, analysis_window

AUDIO_BAND_HZ = 20_000.0
# Zero-padding factor of the impulse-response FFT (frequency interpolation only).
IMPULSE_PAD = 8


def load_f64(path):
    return np.fromfile(path, dtype="<f8")


def db20(x):
    return 20.0 * np.log10(np.maximum(np.abs(x), 1e-300))


def impulse_metrics(h, mult, base_hz, design_bands=None):
    """Frequency-domain metrics of a cascade impulse response.

    `h` is the output (rate base_hz * mult) for a unit impulse at the baseband rate; the DC gain
    of a unity interpolator is sum(h) / mult. Image band k is [k*base - 20 kHz, k*base + 20 kHz]
    up to the output Nyquist: content of the source that must be rejected.
    """
    fs = base_hz * mult
    n = len(h) * IMPULSE_PAD
    spec = np.fft.rfft(h, n) / mult
    f = np.fft.rfftfreq(n, 1.0 / fs)
    mag_db = db20(spec)

    def band_max(lo, hi):
        m = (f >= lo) & (f <= hi)
        return float(mag_db[m].max())

    def at(freq):
        return float(np.interp(freq, f, mag_db))

    pb = mag_db[f <= AUDIO_BAND_HZ]
    images = []
    k = 1
    while k * base_hz - AUDIO_BAND_HZ < fs / 2.0:
        images.append(-band_max(k * base_hz - AUDIO_BAND_HZ, min(k * base_hz + AUDIO_BAND_HZ, fs / 2.0)))
        k += 1

    # Group delay in output samples: Re(FFT(n*h) / FFT(h)).
    nh = np.arange(len(h)) * h
    gd_spec = np.fft.rfft(nh, n) / np.where(np.abs(np.fft.rfft(h, n)) > 0, np.fft.rfft(h, n), 1.0)
    gd = np.real(gd_spec)

    def gd_ms(freq):
        return float(np.interp(freq, f, gd)) / fs * 1000.0

    res = {
        "dc_gain_db": float(db20(np.sum(h) / mult)),
        "ripple_db": float(np.max(np.abs(pb))),
        "gain_17k5_db": at(17_500.0),
        "gain_20k_db": at(AUDIO_BAND_HZ),
        "gain_nyq_db": at(base_hz / 2.0),
        "image_rejection_db": images,
        "worst_image_db": min(images) if images else float("inf"),
        "gd_1k_ms": gd_ms(1000.0),
        "gd_20k_ms": gd_ms(AUDIO_BAND_HZ),
        "tail_energy_frac": float(np.sum(h[-len(h) // 10 :] ** 2) / max(np.sum(h**2), 1e-300)),
    }
    if design_bands is not None:
        pb_hz, sb_hz = design_bands
        res["design_ripple_db"] = float(np.max(np.abs(mag_db[f <= pb_hz])))
        res["design_rejection_db"] = -band_max(sb_hz, fs / 2.0)
    return res


def dtft_db(h, mult, freq_hz, base_hz):
    """|H| in dB (unity-interpolator normalised) at one exact frequency."""
    fs = base_hz * mult
    n = np.arange(len(h))
    return float(db20(np.sum(h * np.exp(-2j * np.pi * freq_hz / fs * n)) / mult))


def tone_metrics(x, k, fs_hz, amp_in):
    """Coherent tone analysis of a record that holds k cycles (k may exceed N/2: it is folded).

    Returns the tone gain in dB, the worst spur (strongest 2*LOBE+1 bin group outside the tone
    lobe) in dBc with its frequency, the total residual (everything else) in dBc, and the
    residual restricted to 20 Hz-20 kHz.
    """
    n = len(x)
    if 2 * k > n:
        k = n - k
    w = analysis_window(n)
    p = np.abs(np.fft.rfft(x * w)) ** 2
    lo, hi = max(k - LOBE_HALF_WIDTH, 0), k + LOBE_HALF_WIDTH
    tone_power = float(p[lo : hi + 1].sum())
    amp = np.sqrt(4.0 * tone_power / (n * float(np.sum(w**2))))
    q = p.copy()
    q[lo : hi + 1] = 0.0
    groups = np.convolve(q, np.ones(2 * LOBE_HALF_WIDTH + 1), mode="same")
    i = int(np.argmax(groups))
    f = np.fft.rfftfreq(n, 1.0 / fs_hz)
    band = (f >= 20.0) & (f <= AUDIO_BAND_HZ)
    return {
        "gain_db": float(db20(amp / amp_in)),
        "spur_dbc": float(10.0 * np.log10(max(groups[i], 1e-300) / tone_power)),
        "spur_hz": float(f[i]),
        "resid_dbc": float(10.0 * np.log10(max(q.sum(), 1e-300) / tone_power)),
        "resid_inband_dbc": float(10.0 * np.log10(max(q[band].sum(), 1e-300) / tone_power)),
        "tone_hz": float(f[k]),
    }
