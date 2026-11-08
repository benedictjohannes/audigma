"""Cascade family, part 1: the implemented half-band cascade (impulse response and tone cross-check).

Measured through `HalfBandCascade` itself (harness modes `cascade-impulse` / `cascade-tone`), so
this is what the engine runs, not what `design_halfband.py` intended. Nothing here has a quality
threshold: values are reported, thresholds come with audit step 5. Two *method* checks do raise
failures: the implemented cascade must reproduce the design numbers, and tones must agree with the
impulse-response prediction.
"""

import itertools

from .common import Section, run_harness
from .linear import dtft_db, impulse_metrics, load_f64, tone_metrics

PRESETS = ["balanced", "reference", "low-latency"]
PHASES = ["linear", "minimum"]
APODS = ["off", "gentle"]
IMPULSE_STAGES = [1, 2, 3, 4, 6, 9]
TONE_STAGES = [1, 6]
TONE_HZ = [1000, 10000, 18000, 20000, 21000, 22000]
IMPULSE_FRAMES = 512
TONE_FRAMES = 4096
BASE_HZ = 44_100
# Method-check tolerances (implementation vs design, tone vs impulse prediction).
DESIGN_REJECTION_TOL_DB = 1.0
DESIGN_RIPPLE_TOL_DB = 1e-4
TONE_GAIN_TOL_DB = 1e-3
TAIL_ENERGY_MAX = 1e-12


def combos():
    return list(itertools.product(PRESETS, PHASES, APODS))


def design_key(preset):
    return preset.replace("-", "_")


def run(harness_bin, design):
    """`design` is design_halfband.design_all_presets() output (or None without scipy)."""
    print("\n=== Cascade filters: impulse response and tone cross-check (HalfBandCascade) ===")
    impulse, tones = {}, {}
    for preset, phase, apod in combos():
        for k in IMPULSE_STAGES:
            stem = f"imp_{preset}_{phase}_{apod}_k{k}"
            meta, bin_out = run_harness(
                harness_bin,
                stem,
                ["--mode", "cascade-impulse", "--stages", str(k), "--preset", preset, "--phase", phase,
                 "--apodization", apod, "--n-bb", str(IMPULSE_FRAMES)],
                with_bin=True,
            )
            h = load_f64(bin_out)
            bands = None
            if k == 1:
                bands = (17_500.0 if apod == "gentle" else 20_000.0, 22_050.0 if apod == "gentle" else 24_100.0)
            m = impulse_metrics(h, meta["mult"], BASE_HZ, bands)
            m["harness_group_delay_samples"] = meta["group_delay_output_samples"]
            m["h"] = h if k in TONE_STAGES else None
            impulse[(preset, phase, apod, k)] = m
            bin_out.unlink()
        print(f"  impulse {preset:11s} {phase:7s} {apod:6s}: "
              + ", ".join(f"K={k}: worst image {impulse[(preset, phase, apod, k)]['worst_image_db']:.1f} dB"
                          for k in (1, 6)))
    for preset, phase, apod in combos():
        for k in TONE_STAGES:
            stem = f"tone_{preset}_{phase}_{apod}_k{k}"
            meta, bin_out = run_harness(
                harness_bin,
                stem,
                ["--mode", "cascade-tone", "--stages", str(k), "--preset", preset, "--phase", phase,
                 "--apodization", apod, "--n-bb", str(TONE_FRAMES), "--tones-hz", ",".join(map(str, TONE_HZ))],
                with_bin=True,
            )
            n_out = meta["n_bb"] * meta["mult"]
            data = load_f64(bin_out).reshape(len(meta["tones"]), n_out)
            h = impulse[(preset, phase, apod, k)]["h"]
            rows = []
            for t, x in zip(meta["tones"], data):
                # Coherent in the baseband record; every output record holds k_out = k * mult cycles.
                res = tone_metrics(x, t["k"], BASE_HZ * meta["mult"], meta["amplitude"])
                res["f_hz"] = t["f_hz"]
                res["predicted_db"] = dtft_db(h, meta["mult"], t["f_hz"], BASE_HZ)
                rows.append(res)
            tones[(preset, phase, apod, k)] = rows
            bin_out.unlink()
    for m in impulse.values():
        m["h"] = None
    return {"impulse": impulse, "tones": tones, "design": design}


def _design_entry(design, preset, phase, apod):
    if design is None:
        return None
    return design["stage0"][design_key(preset)][f"{phase}_{apod}"]


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
"droop" is the gain at that frequency relative to DC.

### Stage 0 (K = 1) against the design

| Preset | Phase | Apod. | Design ripple (dB) | Measured ripple (dB) | Design rejection (dB) | Measured rejection (dB) | Match |
|---|---|---|---:|---:|---:|---:|:---:|
"""
    for preset, phase, apod in combos():
        m = impulse[(preset, phase, apod, 1)]
        d = _design_entry(design, preset, phase, apod)
        if d is None:
            text += f"| `{preset}` | {phase} | {apod} | n/a | {m['design_ripple_db']:.6f} | n/a | {m['design_rejection_db']:.1f} | n/a |\n"
            continue
        ok = (
            abs(m["design_rejection_db"] - d["sb_rejection_db"]) <= DESIGN_REJECTION_TOL_DB
            and m["design_ripple_db"] <= d["pb_ripple_db"] + DESIGN_RIPPLE_TOL_DB
        )
        if not ok:
            failures.append(f"Cascade stage 0 vs design: {preset} {phase}/{apod}")
        text += (
            f"| `{preset}` | {phase} | {apod} | {d['pb_ripple_db']:.6f} | {m['design_ripple_db']:.6f} | "
            f"{d['sb_rejection_db']:.1f} | {m['design_rejection_db']:.1f} | {'yes' if ok else '**NO**'} |\n"
        )
    text += (
        f"\nBands: `off` passband 0-20 kHz, stopband 24.1 kHz-Nyquist; `gentle` passband 0-17.5 kHz, stopband 22.05 kHz-Nyquist "
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

| Preset | Phase | Apod. | Ripple 0-20 kHz (dB) | Droop @ 17.5 kHz (dB) | Droop @ 20 kHz (dB) | Gain @ 22.05 kHz (dB) | Image rejection, band 1 (dB) | Worst image rejection (dB) | Group delay @ 1 kHz (ms) | @ 20 kHz (ms) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for preset, phase, apod in combos():
        m = impulse[(preset, phase, apod, 6)]
        text += (
            f"| `{preset}` | {phase} | {apod} | {m['ripple_db']:.6f} | {m['gain_17k5_db']:.3f} | {m['gain_20k_db']:.3f} | "
            f"{m['gain_nyq_db']:.1f} | {m['image_rejection_db'][0]:.1f} | {m['worst_image_db']:.1f} | "
            f"{m['gd_1k_ms']:.3f} | {m['gd_20k_ms']:.3f} |\n"
        )

    text += """
### Stage count (K) scaling, `linear` / `off`

K = 1..4 are the PCM tiers (88.2k..705.6k), K = 6 is DSD64 and K = 9 is DSD512.

| Preset | K | Output rate | DC gain (dB) | Ripple 0-20 kHz (dB) | Worst image rejection (dB) | Image rejection per band 1, 2, 3 (dB) | Group delay @ 1 kHz (ms) |
|---|---:|---:|---:|---:|---:|---|---:|
"""
    for preset in PRESETS:
        for k in IMPULSE_STAGES:
            m = impulse[(preset, "linear", "off", k)]
            bands = ", ".join(f"{x:.1f}" for x in m["image_rejection_db"][:3])
            text += (
                f"| `{preset}` | {k} | {44.1 * (1 << k):,.1f} kHz | {m['dc_gain_db']:.6f} | {m['ripple_db']:.6f} | "
                f"{m['worst_image_db']:.1f} | {bands} | {m['gd_1k_ms']:.3f} |\n"
            )
    text += f"""
Worst image rejection over all {len(impulse)} combinations (3 presets x 2 phases x 2 apodizations x K in {IMPULSE_STAGES}): \
**{min(m['worst_image_db'] for m in impulse.values()):.1f} dB**; largest DC-gain deviation \
{max(abs(m['dc_gain_db']) for m in impulse.values()):.2e} dB.

---

## Cascade tone cross-check

Coherent tones ({", ".join(f"{t / 1000:g}" for t in TONE_HZ)} kHz, -6 dBFS) pushed through the same cascade (harness mode `cascade-tone`, {TONE_FRAMES} baseband
frames, Nuttall window). "Predicted" is `|H(f)|` of the impulse response above evaluated at the exact tone frequency. This validates the
impulse method (tile boundaries, state handling, FTZ/DAZ); the spur column is the strongest component outside the tone, so it is the
actual image/alias level seen with a real signal.

| Preset | Phase | Apod. | K | Max measured-predicted gain difference (dB) | Gain @ 20 kHz measured (dB) | Gain @ 22 kHz measured (dB) | Worst spur, tones <= 20 kHz (dBc) | Spur frequency (kHz) | Agrees |
|---|---|---|---:|---:|---:|---:|---:|---:|:---:|
"""
    for (preset, phase, apod, k), rows in tones.items():
        diff = max(abs(r["gain_db"] - r["predicted_db"]) for r in rows)
        # Tones within 2 kHz of the source Nyquist are excluded: their own image sits next to them (a transition-band effect, see the gain columns).
        worst = max((r for r in rows if r["f_hz"] <= 20_500), key=lambda r: r["spur_dbc"])
        g20 = next(r for r in rows if abs(r["f_hz"] - 20000) < 100)
        g22 = next(r for r in rows if abs(r["f_hz"] - 22000) < 100)
        ok = diff <= TONE_GAIN_TOL_DB
        if not ok:
            failures.append(f"Cascade tone vs impulse: {preset} {phase}/{apod} K={k}")
        text += (
            f"| `{preset}` | {phase} | {apod} | {k} | {diff:.2e} | {g20['gain_db']:.4f} | {g22['gain_db']:.3f} | "
            f"{worst['spur_dbc']:.1f} | {worst['spur_hz'] / 1000:.2f} | {'yes' if ok else '**NO**'} |\n"
        )
    text += f"\nAgreement tolerance: {TONE_GAIN_TOL_DB} dB.\n"
    return Section(text, failures)
