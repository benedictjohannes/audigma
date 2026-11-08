"""Cascade family, part 1: the implemented half-band cascade (impulse response and tone cross-check).

Measured through `HalfBandCascade` itself (harness modes `cascade-impulse` / `cascade-tone`), so
this is what the engine runs, not what `design_halfband.py` intended. Nothing here has a quality
threshold: values are reported, thresholds come with audit step 5. Three *method* checks do raise
failures: the implemented cascade must equal the design cascade built from `design_halfband.py`
(every stage, sample by sample), stage 0 must reproduce the design numbers, and tones must agree
with the impulse-response prediction.
"""

import itertools

import numpy as np

from .common import Section, run_harness
from .linear import dtft_db, impulse_metrics, load_f64, tone_metrics

PHASES = ["linear", "minimum"]
# Apodization settings as the harness and the TOML spell them ("off" or the pass edge in Hz).
APODS = ["off", "18000", "18500", "19000", "20000"]
IMPULSE_STAGES = [1, 2, 3, 4, 6, 9]
TONE_STAGES = [1, 6]
TONE_HZ = [1000, 10000, 18000, 20000, 21000, 22000]
IMPULSE_FRAMES = 512
TONE_FRAMES = 4096
BASE_HZ = 44_100
# Method-check tolerances (implementation vs design, tone vs impulse prediction).
DESIGN_REJECTION_TOL_DB = 1.0
DESIGN_RIPPLE_TOL_DB = 1e-4
DESIGN_IMPULSE_TOL = 1e-12
TONE_GAIN_TOL_DB = 1e-3
TAIL_ENERGY_MAX = 1e-12


def combos():
    return list(itertools.product(PHASES, APODS))


def stage_design(design, stage, phase, apod):
    """Design record of interpolation stage `stage` (the apodizing filter replaces stage 0)."""
    if stage == 0 and apod != "off":
        return design["apodizing"][int(apod)][phase]
    stages = design["halfband"]
    return stages[min(stage, len(stages) - 1)][phase]


def design_impulse(design, phase, apod, k, n_out):
    """Impulse response of the designed K-stage cascade: zero-stuff by 2, then filter with 2*h, per stage."""
    x = np.array([1.0])
    for stage in range(k):
        h = np.asarray(stage_design(design, stage, phase, apod)["full_taps"])
        up = np.zeros(2 * len(x))
        up[0::2] = x
        x = np.convolve(up, 2.0 * h)[: n_out]
    out = np.zeros(n_out)
    out[: len(x)] = x[:n_out]
    return out


def design_deviation(measured, designed):
    """Largest |measured - designed| relative to the peak, after the best integer alignment.

    The symmetric half-band kernel places its centre tap one output sample from where a plain
    zero-stuffed convolution puts it, so a small constant lag per stage is expected; magnitude
    and shape must be identical.
    """
    base = int(np.argmax(np.abs(measured))) - int(np.argmax(np.abs(designed)))
    peak = float(np.max(np.abs(designed)))
    best = (np.inf, 0)
    for lag in range(base - 4, base + 5):
        if lag >= 0:
            a, b = measured[lag:], designed[: len(measured) - lag]
        else:
            a, b = measured[: len(measured) + lag], designed[-lag:]
        err = float(np.max(np.abs(a - b))) / peak
        best = min(best, (err, lag))
    return best


def run(harness_bin, design):
    """`design` is design_halfband.design_all() output (or None without scipy)."""
    print("\n=== Cascade filters: impulse response and tone cross-check (HalfBandCascade) ===")
    impulse, tones = {}, {}
    for phase, apod in combos():
        for k in IMPULSE_STAGES:
            stem = f"imp_{phase}_{apod}_k{k}"
            meta, bin_out = run_harness(
                harness_bin,
                stem,
                ["--mode", "cascade-impulse", "--stages", str(k), "--phase", phase,
                 "--apodization", apod, "--n-bb", str(IMPULSE_FRAMES)],
                with_bin=True,
            )
            h = load_f64(bin_out)
            bands = None
            if k == 1:
                bands = (20_000.0, 24_100.0) if apod == "off" else (float(apod), 22_050.0)
            m = impulse_metrics(h, meta["mult"], BASE_HZ, bands)
            m["harness_group_delay_samples"] = meta["group_delay_output_samples"]
            # The same normalised filters serve 48 kHz sources: image bands k*48 kHz +- 20 kHz, apodizing edge x 48/44.1.
            m["fam48"] = impulse_metrics(h, meta["mult"], 48_000.0)
            if design is not None:
                m["design_dev"], m["design_lag"] = design_deviation(h, design_impulse(design, phase, apod, k, len(h)))
            m["h"] = h if k in TONE_STAGES else None
            impulse[(phase, apod, k)] = m
            bin_out.unlink()
        print(f"  impulse {phase:7s} {apod:6s}: "
              + ", ".join(f"K={k}: worst image {impulse[(phase, apod, k)]['worst_image_db']:.1f} dB"
                          for k in (1, 6)))
    for phase, apod in combos():
        for k in TONE_STAGES:
            stem = f"tone_{phase}_{apod}_k{k}"
            meta, bin_out = run_harness(
                harness_bin,
                stem,
                ["--mode", "cascade-tone", "--stages", str(k), "--phase", phase,
                 "--apodization", apod, "--n-bb", str(TONE_FRAMES), "--tones-hz", ",".join(map(str, TONE_HZ))],
                with_bin=True,
            )
            n_out = meta["n_bb"] * meta["mult"]
            data = load_f64(bin_out).reshape(len(meta["tones"]), n_out)
            h = impulse[(phase, apod, k)]["h"]
            rows = []
            for t, x in zip(meta["tones"], data):
                # Coherent in the baseband record; every output record holds k_out = k * mult cycles.
                res = tone_metrics(x, t["k"], BASE_HZ * meta["mult"], meta["amplitude"])
                res["f_hz"] = t["f_hz"]
                res["predicted_db"] = dtft_db(h, meta["mult"], t["f_hz"], BASE_HZ)
                rows.append(res)
            tones[(phase, apod, k)] = rows
            bin_out.unlink()
    for m in impulse.values():
        m["h"] = None
    return {"impulse": impulse, "tones": tones, "design": design}


def _apod_label(apod):
    return "off" if apod == "off" else f"{apod} Hz"


def render(res):
    failures = []
    impulse, tones, design = res["impulse"], res["tones"], res["design"]
    text = f"""
---

## Implemented half-band cascade (impulse response)

Measured through `HalfBandCascade` (harness mode `cascade-impulse`): a unit impulse at the 44.1 kHz baseband rate, response
FFT-ed with {IMPULSE_FRAMES} baseband frames (zero padded x8). The cascade is linear and time-invariant from the baseband
input, so this response describes it completely. No pass/fail thresholds are applied here; values are reported for audit
step 5. "Image rejection" is the worst level inside the bands `k*44.1 kHz +- 20 kHz` (k = 1, 2, ...) that must be removed;
"droop" is the gain at that frequency relative to DC. Every filter is designed for >= 140 dB (`design_halfband.py`); apodization
is `off` (half-band, stopband 24.1 kHz) or the pass edge of an apodizing stage 0 whose stopband is 22.05 kHz.
"""
    if design is not None:
        text += """
### Stage designs (`design_halfband.py`)

Interpolation stage k outputs at 88.2 kHz x 2^k; pass 20 kHz, stop at the mirror `fs/2 - 20 kHz`. The decimator runs the same
designs from its output backwards (its last stage is stage 0).

| Stage | Output rate | Taps | Stopband from | Design rejection, linear (dB) | Design rejection, minimum (dB) |
|---:|---:|---:|---:|---:|---:|
"""
        for k, modes in enumerate(design["halfband"]):
            lin, mn = modes["linear"], modes["minimum"]
            text += (
                f"| {k} | {lin['fs_hz'] / 1e3:,.1f} kHz | {lin['numtaps']} | {lin['stopband_hz'] / 1e3:,.1f} kHz | "
                f"{lin['sb_rejection_db']:.1f} | {mn['sb_rejection_db']:.1f} |\n"
            )
        text += """
| Apodizing stage 0, pass edge | Taps | Design rejection >= 22.05 kHz, linear (dB) | minimum (dB) |
|---:|---:|---:|---:|
"""
        for edge, modes in design["apodizing"].items():
            text += (
                f"| {edge} Hz | {modes['linear']['numtaps']} | {modes['linear']['sb_rejection_db']:.1f} | "
                f"{modes['minimum']['sb_rejection_db']:.1f} |\n"
            )

        text += f"""
### Every stage against the design

The design cascade is rebuilt in Python from the `design_halfband.py` taps of each stage (zero-stuff by 2, filter with 2h) and
compared sample by sample with the measured impulse response, after the best integer alignment. Deviation is relative to the
response peak; tolerance {DESIGN_IMPULSE_TOL:g}.

| Phase | Apod. | """ + " | ".join(f"K = {k}" for k in IMPULSE_STAGES) + " | Match |\n|---|---|" + "---:|" * len(IMPULSE_STAGES) + ":---:|\n"
        for phase, apod in combos():
            devs = [impulse[(phase, apod, k)]["design_dev"] for k in IMPULSE_STAGES]
            ok = max(devs) <= DESIGN_IMPULSE_TOL
            if not ok:
                failures.append(f"Cascade vs design (every stage): {phase}/{apod}")
            text += f"| {phase} | {_apod_label(apod)} | " + " | ".join(f"{d:.1e}" for d in devs) + f" | {'yes' if ok else '**NO**'} |\n"

    text += """
### Stage 0 (K = 1) against the design

| Phase | Apod. | Design ripple (dB) | Measured ripple (dB) | Design rejection (dB) | Measured rejection (dB) | Match |
|---|---|---:|---:|---:|---:|:---:|
"""
    for phase, apod in combos():
        m = impulse[(phase, apod, 1)]
        if design is None:
            text += f"| {phase} | {_apod_label(apod)} | n/a | {m['design_ripple_db']:.2e} | n/a | {m['design_rejection_db']:.1f} | n/a |\n"
            continue
        d = stage_design(design, 0, phase, apod)
        ok = (
            abs(m["design_rejection_db"] - d["sb_rejection_db"]) <= DESIGN_REJECTION_TOL_DB
            and m["design_ripple_db"] <= d["pb_ripple_db"] + DESIGN_RIPPLE_TOL_DB
        )
        if not ok:
            failures.append(f"Cascade stage 0 vs design: {phase}/{apod}")
        text += (
            f"| {phase} | {_apod_label(apod)} | {d['pb_ripple_db']:.2e} | {m['design_ripple_db']:.2e} | "
            f"{d['sb_rejection_db']:.1f} | {m['design_rejection_db']:.1f} | {'yes' if ok else '**NO**'} |\n"
        )
    text += (
        f"\nBands: `off` passband 0-20 kHz, stopband 24.1 kHz-Nyquist; apodizing passband 0-edge, stopband 22.05 kHz-Nyquist "
        f"(the design's own definition). Match = rejection within {DESIGN_REJECTION_TOL_DB} dB and ripple within "
        f"{DESIGN_RIPPLE_TOL_DB} dB of the design. The tails of all impulse records are below {TAIL_ENERGY_MAX:g} of the "
        f"energy, so {IMPULSE_FRAMES} frames capture the full response.\n"
    )
    worst_tail = max(m["tail_energy_frac"] for m in impulse.values())
    if worst_tail > TAIL_ENERGY_MAX:
        failures.append("Cascade impulse record truncated (tail energy)")
        text += f"\n**Impulse record too short: tail energy {worst_tail:.2e}.**\n"

    text += """
### Full cascade at DSD64 (K = 6, 64x), all filter settings

| Phase | Apod. | Ripple 0-20 kHz (dB) | Gain @ 17.5 kHz (dB) | Gain @ 20 kHz (dB) | Gain @ 22.05 kHz (dB) | Image rejection, band 1 (dB) | Worst image rejection (dB) | Group delay @ 1 kHz (ms) | @ 20 kHz (ms) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for phase, apod in combos():
        m = impulse[(phase, apod, 6)]
        text += (
            f"| {phase} | {_apod_label(apod)} | {m['ripple_db']:.6f} | {m['gain_17k5_db']:.3f} | {m['gain_20k_db']:.3f} | "
            f"{m['gain_nyq_db']:.1f} | {m['image_rejection_db'][0]:.1f} | {m['worst_image_db']:.1f} | "
            f"{m['gd_1k_ms']:.3f} | {m['gd_20k_ms']:.3f} |\n"
        )

    text += """
### Rate family: 44.1 kHz vs 48 kHz sources (K = 6, DSD64 / DSD64_48)

The cascade uses the same normalised filters for both families, so a 48 kHz source sees every frequency scaled by 48/44.1
(apodizing pass edge 19 kHz -> 20.7 kHz). The audio band stays 20 kHz, so the 48k family has more passband margin and a wider
first image gap (28 kHz instead of 24.1 kHz). Image bands: `k*fs +- 20 kHz`.

| Phase | Apod. | Gain @ 20 kHz, 44.1k / 48k (dB) | Ripple 0-20 kHz, 44.1k / 48k (dB) | Worst image rejection, 44.1k / 48k (dB) | Group delay @ 1 kHz, 44.1k / 48k (ms) |
|---|---|---:|---:|---:|---:|
"""
    for phase, apod in combos():
        a = impulse[(phase, apod, 6)]
        b = a["fam48"]
        text += (
            f"| {phase} | {_apod_label(apod)} | {a['gain_20k_db']:.3f} / {b['gain_20k_db']:.3f} | {a['ripple_db']:.2e} / {b['ripple_db']:.2e} | "
            f"{a['worst_image_db']:.1f} / {b['worst_image_db']:.1f} | {a['gd_1k_ms']:.3f} / {b['gd_1k_ms']:.3f} |\n"
        )
    text += (
        f"\nWorst image rejection over all {len(impulse)} combinations, 48k family: "
        f"**{min(m['fam48']['worst_image_db'] for m in impulse.values()):.1f} dB** "
        f"(44.1k family: {min(m['worst_image_db'] for m in impulse.values()):.1f} dB).\n"
    )

    text += """
### Stage count (K) scaling, `linear` / `off`

K = 1..4 are the PCM tiers (88.2k..705.6k), K = 6 is DSD64 and K = 9 is DSD512.

| K | Output rate | DC gain (dB) | Ripple 0-20 kHz (dB) | Worst image rejection (dB) | Image rejection per band 1, 2, 3 (dB) | Group delay @ 1 kHz (ms) |
|---:|---:|---:|---:|---:|---|---:|
"""
    for k in IMPULSE_STAGES:
        m = impulse[("linear", "off", k)]
        bands = ", ".join(f"{x:.1f}" for x in m["image_rejection_db"][:3])
        text += (
            f"| {k} | {44.1 * (1 << k):,.1f} kHz | {m['dc_gain_db']:.2e} | {m['ripple_db']:.2e} | "
            f"{m['worst_image_db']:.1f} | {bands} | {m['gd_1k_ms']:.3f} |\n"
        )
    text += f"""
Worst image rejection over all {len(impulse)} combinations ({len(PHASES)} phases x {len(APODS)} apodizations x K in {IMPULSE_STAGES}): \
**{min(m['worst_image_db'] for m in impulse.values()):.1f} dB**; largest DC-gain deviation \
{max(abs(m['dc_gain_db']) for m in impulse.values()):.2e} dB.

---

## Cascade tone cross-check

Coherent tones ({", ".join(f"{t / 1000:g}" for t in TONE_HZ)} kHz, -6 dBFS) pushed through the same cascade (harness mode `cascade-tone`, {TONE_FRAMES} baseband
frames, Nuttall window). "Predicted" is `|H(f)|` of the impulse response above evaluated at the exact tone frequency. This validates the
impulse method (tile boundaries, state handling, FTZ/DAZ); the spur column is the strongest component outside the tone, so it is the
actual image/alias level seen with a real signal.

| Phase | Apod. | K | Max measured-predicted gain difference (dB) | Gain @ 20 kHz measured (dB) | Gain @ 22 kHz measured (dB) | Worst spur, tones <= 20 kHz (dBc) | Spur frequency (kHz) | Agrees |
|---|---|---:|---:|---:|---:|---:|---:|:---:|
"""
    for (phase, apod, k), rows in tones.items():
        diff = max(abs(r["gain_db"] - r["predicted_db"]) for r in rows)
        # Tones within 2 kHz of the source Nyquist are excluded: their own image sits next to them (a transition-band effect, see the gain columns).
        worst = max((r for r in rows if r["f_hz"] <= 20_500), key=lambda r: r["spur_dbc"])
        g20 = next(r for r in rows if abs(r["f_hz"] - 20000) < 100)
        g22 = next(r for r in rows if abs(r["f_hz"] - 22000) < 100)
        ok = diff <= TONE_GAIN_TOL_DB
        if not ok:
            failures.append(f"Cascade tone vs impulse: {phase}/{apod} K={k}")
        text += (
            f"| {phase} | {_apod_label(apod)} | {k} | {diff:.2e} | {g20['gain_db']:.4f} | {g22['gain_db']:.3f} | "
            f"{worst['spur_dbc']:.1f} | {worst['spur_hz'] / 1000:.2f} | {'yes' if ok else '**NO**'} |\n"
        )
    text += f"\nAgreement tolerance: {TONE_GAIN_TOL_DB} dB.\n"
    return Section(text, failures)
