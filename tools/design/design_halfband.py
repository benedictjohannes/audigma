#!/usr/bin/env python3
"""Half-band and apodizing FIR designer for audigma-dsp (Design.md §4.2).

Every filter targets >= 140 dB image rejection; there is no quality preset.

Half-band stages (2x interpolation stage k, output rate 88.2 kHz * 2^k at the 44.1 kHz family):

* Exact half-band construction (Vaidyanathan-Nguyen): a one-band minimax filter ``g`` of length
  (N+1)/2 on [0, 2 * pass / fs] is interleaved with the 0.5 centre tap. DC is pinned inside the
  minimax (a narrow, heavily weighted band at DC), so the unity-DC renormalisation afterwards is a
  ~1e-9 correction instead of the ~6 dB rejection loss of zeroing a two-band Remez design.
* The passband is 20 kHz at every stage; the stopband is the mirror ``fs/2 - 20 kHz`` (24.1 kHz at
  stage 0, 68.2 kHz at stage 1, then relaxing quickly). Stage 0 and 1 keep fixed lengths (199 and
  31 taps, ~146 dB); stages 2+ use the shortest length that reaches the target.
* A half-band has equal pass and stop ripple, so the rejection fixes the passband ripple
  (140 dB -> 8.7e-7 dB); there is no separate ripple goal.
* The decimator runs the same designs in reverse: its last stage (into the output rate) is stage 0,
  the one before it stage 1, and so on.

Stage-0 apodizing filters (``apodization = <pass edge Hz>``): Kaiser-windowed linear-phase low-pass
with the stopband at 22.05 kHz (the source Nyquist, which is what makes it apodizing) and a
selectable pass edge. Exported as general (non-half-band) polyphase filters.

Every linear-phase design also has a homomorphic minimum-phase equivalent (same magnitude).
All full-length filters are renormalised to exact unity DC gain (sum(h) == 1). Band edges are for the
44.1 kHz family; the 48 kHz family runs the same tables, so every edge scales by 48/44.1.
"""

import numpy as np
import scipy.signal as signal

# Image rejection every filter must reach (Design.md §4.2).
TARGET_REJECTION_DB = 140.0

# FFT size of the homomorphic cepstral minimum-phase transform; >= 65536 keeps the 140 dB floor.
MIN_PHASE_FFT_SIZE = 65536

PASSBAND_HZ = 20_000.0
STAGE0_FS_HZ = 88_200.0

# Number of 2x stages that can occur: 44.1 kHz -> DSD512 (512x) is 9 stages, in either direction.
MAX_STAGES = 9

# Fixed half-band lengths for stage 0 and 1 (N % 4 == 3); stages 2+ get the shortest that reaches the target.
FIXED_HALFBAND_TAPS = {0: 199, 1: 31}

# Apodizing stage 0: stopband at the source Nyquist, selectable pass edge (Hz, 44.1 kHz family).
APODIZING_STOPBAND_HZ = 22_050.0
APODIZING_PASS_EDGES_HZ = (18_000, 18_500, 19_000, 20_000)
# Kaiser design attenuation, a few dB above the target for window/measurement differences.
APODIZING_ATTENUATION_DB = 144.0

MEASURE_FFT_POINTS = 1 << 18


def stage_fs_hz(stage):
    """Output rate of 2x interpolation stage ``stage`` (input rate of the matching decimation stage)."""
    return STAGE0_FS_HZ * (1 << stage)


def halfband_stopband_hz(fs_hz):
    """A half-band's stopband is the mirror of its passband around fs/4."""
    return fs_hz / 2.0 - PASSBAND_HZ


def _measure_response(h, fs_hz, passband_hz, stopband_hz):
    """Returns (max passband deviation dB, stopband rejection dB)."""
    w, response = signal.freqz(h, worN=MEASURE_FFT_POINTS, fs=fs_hz)
    response_db = 20.0 * np.log10(np.abs(response) + 1e-300)
    passband = response_db[w <= passband_hz]
    stopband = response_db[w >= stopband_hz]
    return float(np.max(np.abs(passband))), float(-np.max(stopband))


def exact_halfband_taps(numtaps, passband_hz, fs_hz):
    """Exact half-band: minimax ``g`` of length (N+1)/2 with DC pinned, interleaved with the 0.5 centre tap."""
    if numtaps % 4 != 3:
        raise ValueError(f"half-band length must be 4m + 3, got {numtaps}")
    m = (numtaps + 1) // 2
    fp = 2.0 * passband_hz / fs_hz
    grid_density = max(16, int(64 / (fp * m)) + 16)
    eps = fp * 1e-3
    g = signal.remez(
        m,
        [0.0, eps, 2.0 * eps, fp],
        [1.0, 1.0],
        weight=[1e3, 1.0],
        fs=1.0,
        grid_density=grid_density,
    )
    g = g / np.sum(g)
    h = np.zeros(numtaps)
    h[0::2] = g / 2.0
    h[(numtaps - 1) // 2] = 0.5
    return h


def _halfband_record(h, fs_hz):
    numtaps = len(h)
    center = (numtaps - 1) // 2
    stopband_hz = halfband_stopband_hz(fs_hz)
    ripple, rejection = _measure_response(h, fs_hz, PASSBAND_HZ, stopband_hz)
    return {
        "numtaps": numtaps,
        "full_taps": h.tolist(),
        # Positive-offset odd taps h[c+1], h[c+3], ... (the symmetric kernel's coefficients).
        "unique_odd_taps": [float(h[center + k]) for k in range(1, numtaps - center, 2)],
        "fs_hz": fs_hz,
        "passband_hz": PASSBAND_HZ,
        "stopband_hz": stopband_hz,
        "pb_ripple_db": ripple,
        "sb_rejection_db": rejection,
    }


def design_halfband_stage(stage, target_db=TARGET_REJECTION_DB):
    """Designs the linear-phase half-band of 2x stage ``stage`` (fixed length for 0/1, else minimal)."""
    fs_hz = stage_fs_hz(stage)
    if stage in FIXED_HALFBAND_TAPS:
        return _halfband_record(exact_halfband_taps(FIXED_HALFBAND_TAPS[stage], PASSBAND_HZ, fs_hz), fs_hz)
    numtaps = 7
    while numtaps < 400:
        rec = _halfband_record(exact_halfband_taps(numtaps, PASSBAND_HZ, fs_hz), fs_hz)
        if rec["sb_rejection_db"] >= target_db:
            return rec
        numtaps += 4
    raise RuntimeError(f"stage {stage}: no half-band up to {numtaps} taps reaches {target_db} dB")


def minimum_phase_filter(h, n_fft=MIN_PHASE_FFT_SIZE):
    """Causal minimum-phase equivalent of a linear-phase FIR with the same magnitude (``half=False``)."""
    h_min = signal.minimum_phase(np.asarray(h, dtype=np.float64), method="homomorphic", n_fft=n_fft, half=False)
    h_min = np.asarray(h_min, dtype=np.float64)
    dc_gain = float(np.sum(h_min))
    if abs(dc_gain) < 1e-15:
        raise ValueError("minimum-phase transform produced zero DC gain")
    return h_min / dc_gain


def _minimum_phase_record(source):
    h_min = minimum_phase_filter(source["full_taps"])
    ripple, rejection = _measure_response(h_min, source["fs_hz"], source["passband_hz"], source["stopband_hz"])
    return {
        "numtaps": len(h_min),
        "full_taps": h_min.tolist(),
        "fs_hz": source["fs_hz"],
        "passband_hz": source["passband_hz"],
        "stopband_hz": source["stopband_hz"],
        "pb_ripple_db": ripple,
        "sb_rejection_db": rejection,
    }


def design_apodizing(pass_edge_hz, fs_hz=STAGE0_FS_HZ):
    """Kaiser-windowed linear-phase stage-0 low-pass: pass edge ``pass_edge_hz``, stopband 22.05 kHz."""
    width = (APODIZING_STOPBAND_HZ - pass_edge_hz) / (fs_hz / 2.0)
    numtaps, beta = signal.kaiserord(APODIZING_ATTENUATION_DB, width)
    numtaps += 1 - numtaps % 2
    cutoff_hz = 0.5 * (pass_edge_hz + APODIZING_STOPBAND_HZ)
    h = np.asarray(signal.firwin(numtaps, cutoff_hz, window=("kaiser", beta), fs=fs_hz), dtype=np.float64)
    h /= np.sum(h)
    ripple, rejection = _measure_response(h, fs_hz, pass_edge_hz, APODIZING_STOPBAND_HZ)
    return {
        "numtaps": int(numtaps),
        "full_taps": h.tolist(),
        "fs_hz": fs_hz,
        "passband_hz": float(pass_edge_hz),
        "stopband_hz": APODIZING_STOPBAND_HZ,
        "pb_ripple_db": ripple,
        "sb_rejection_db": rejection,
    }


def _verify(label, rec):
    if rec["sb_rejection_db"] < TARGET_REJECTION_DB:
        raise RuntimeError(f"{label}: rejection {rec['sb_rejection_db']:.2f} dB is below {TARGET_REJECTION_DB} dB")
    if abs(sum(rec["full_taps"]) - 1.0) > 1e-12:
        raise RuntimeError(f"{label}: DC gain is not unity")


def design_all():
    """Returns {"halfband": [{"linear", "minimum"} per stage 0..MAX_STAGES-1], "apodizing": {edge: {...}}}."""
    halfband = []
    for stage in range(MAX_STAGES):
        linear = design_halfband_stage(stage)
        minimum = _minimum_phase_record(linear)
        _verify(f"halfband stage {stage} linear", linear)
        _verify(f"halfband stage {stage} minimum", minimum)
        halfband.append({"linear": linear, "minimum": minimum})

    apodizing = {}
    for edge in APODIZING_PASS_EDGES_HZ:
        linear = design_apodizing(edge)
        minimum = _minimum_phase_record(linear)
        _verify(f"apodizing {edge} linear", linear)
        _verify(f"apodizing {edge} minimum", minimum)
        apodizing[edge] = {"linear": linear, "minimum": minimum}

    print("=== Half-band stages ===")
    for stage, modes in enumerate(halfband):
        for mode, rec in modes.items():
            print(
                f"stage {stage} {mode:8s}: taps={rec['numtaps']:3d}, fs={rec['fs_hz'] / 1e3:8.1f}k, "
                f"stop={rec['stopband_hz'] / 1e3:7.1f}k, ripple={rec['pb_ripple_db']:.2e} dB, "
                f"rejection={rec['sb_rejection_db']:.2f} dB"
            )
    print("\n=== Apodizing stage 0 ===")
    for edge, modes in apodizing.items():
        for mode, rec in modes.items():
            print(
                f"pass {edge} Hz {mode:8s}: taps={rec['numtaps']:3d}, ripple={rec['pb_ripple_db']:.2e} dB, "
                f"rejection={rec['sb_rejection_db']:.2f} dB"
            )
    return {"halfband": halfband, "apodizing": apodizing}


if __name__ == "__main__":
    design_all()
