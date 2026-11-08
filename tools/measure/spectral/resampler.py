"""Resampler family: the polyphase rational SRC and the ASRC, measured through `PolyphaseResampler`.

Harness modes `src-tone` (coherent tones through the production type) and `src-tables` (the bank a
resampler builds at construction). No quality thresholds are applied (audit step 5); the only failures
raised are method checks (compiled banks must equal `design_src.py`).
"""

import numpy as np

from .common import Section, run_harness
from .linear import load_f64, tone_metrics

try:
    import design_src
except ImportError:  # scipy missing
    design_src = None

# Tone grid for the 1x case (Hz at 44.1k/48k base rates); scaled by the rate multiplier for 2x..16x.
TONES_HZ = [1000, 10000, 18000, 20000, 21000]
# 48k-family input at and above the 44.1k-family Nyquist (22.05 kHz). 22-23.5 kHz is the transition band (its aliases
# land in 20.6-22.05 kHz, above the audio band); 24.5 and 26 kHz are in the stopband and would alias to 19.6 / 18.1 kHz.
DOWN_EXTRA_HZ = [22000, 23000, 23500, 24500, 26000]
MULTIPLIERS = [1, 2, 4, 8, 16]
# Rational conversions outside the matched-tier pairs (PCM targets below the matched tier).
RATIONAL_EXTRA = [
    # (label, in_hz, out_hz, tones at the input)
    ("96k -> 44.1k", 96_000, 44_100, TONES_HZ + DOWN_EXTRA_HZ + [30_000, 40_000]),
    ("88.2k -> 48k", 88_200, 48_000, TONES_HZ + [24_500, 26_000, 30_000, 40_000]),
]
ASRC_CASES = [
    # (label, in_hz, out_hz, ppm)
    ("44.1k -> 44.1k", 44_100, 44_100, 0.0),
    ("44.1k -> 44.1k", 44_100, 44_100, 10.0),
    ("44.1k -> 44.1k", 44_100, 44_100, 50.0),
    ("44.1k -> 44.1k", 44_100, 44_100, -50.0),
    ("44.1k -> 44.1k", 44_100, 44_100, 500.0),
    ("176.4k -> 176.4k", 176_400, 176_400, 50.0),
    ("48k -> 48k", 48_000, 48_000, 50.0),
    ("44.1k -> 48k", 44_100, 48_000, 0.0),
    ("48k -> 44.1k", 48_000, 44_100, 0.0),
    ("48k -> 44.1k", 48_000, 44_100, 100.0),
]
# Banks compared with design_src.py: (mode, in_hz, out_hz).
BANK_CASES = [
    ("rational", 44_100, 48_000),
    ("rational", 48_000, 44_100),
    ("rational", 96_000, 44_100),
    ("asrc", 44_100, 44_100),
    ("asrc", 48_000, 44_100),
]
N_OUT = 131_072
# Rust and numpy evaluate the same closed form; allow a few ulps of libm difference.
BANK_MISMATCH_TOL = 1e-14


def _run_src(harness_bin, stem, src_mode, in_hz, out_hz, ppm, tones_in):
    meta, bin_out = run_harness(
        harness_bin,
        stem,
        ["--mode", "src-tone", "--src-mode", src_mode, "--in-hz", str(in_hz), "--out-hz", str(out_hz),
         "--ppm", str(ppm), "--tones-hz", ",".join(map(str, tones_in)), "--n-out", str(N_OUT)],
        with_bin=True,
    )
    data = load_f64(bin_out).reshape(len(tones_in), N_OUT)
    bin_out.unlink()
    rows = []
    for t, x in zip(meta["tones"], data):
        r = tone_metrics(x, t["k"], out_hz, meta["amplitude"])
        r["f_in_hz"] = t["f_in_hz"]
        rows.append(r)
    return {"meta": meta, "rows": rows}


def _bank(harness_bin, mode, in_hz, out_hz):
    meta, bin_out = run_harness(
        harness_bin,
        f"src_bank_{mode}_{in_hz}_{out_hz}",
        ["--mode", "src-tables", "--src-mode", mode, "--in-hz", str(in_hz), "--out-hz", str(out_hz)],
        with_bin=True,
    )
    taps, phases = meta["taps"], meta["phases"]
    bank = load_f64(bin_out).reshape(meta["rows"], taps)
    bin_out.unlink()
    first = meta["first_phase"]
    entry = {"mode": mode, "in_hz": in_hz, "out_hz": out_hz, "phases": phases, "taps": taps, "rows": meta["rows"], "scale": meta["scale"]}
    if design_src is not None:
        ref_taps = design_src.taps_for_scale(meta["scale"], design_src.design_unity_taps())
        entry["ref_taps"] = ref_taps
        if ref_taps == taps:
            ref = np.array([design_src.bank_row((first + r) / phases, meta["scale"], taps)[::-1] for r in range(meta["rows"])])
            entry["max_diff"] = float(np.max(np.abs(ref - bank)))
    # Prototype sampled at `phases` points per input sample, rebuilt from the rows for phases 0..phases-1
    # (rows are stored reversed, oldest input first): h[i * phases + p] = row_p[taps - 1 - i].
    rows = bank[-first : -first + phases, ::-1]
    h = rows.T.reshape(-1)
    nfft = 1 << 20
    spec = np.fft.rfft(h, nfft) / phases
    f = np.fft.rfftfreq(nfft, 1.0 / phases) * in_hz  # Hz
    db = 20 * np.log10(np.maximum(np.abs(spec), 1e-300))
    low = min(in_hz, out_hz)
    entry["gain"] = {x: float(np.interp(x, f, db)) for x in (18_000 / 44_100 * low, 20_000 / 44_100 * low, 21_000 / 44_100 * low, 0.5 * low)}
    pb = f <= 20_000 / 44_100 * low
    entry["passband_dev_db"] = float(np.max(np.abs(db[pb])))
    entry["stop_rejection_db"] = float(-db[f >= 24_100 / 44_100 * low].max())
    return entry


def run(harness_bin):
    print("\n=== Resampler: PolyphaseResampler tables, rational SRC and ASRC ===")
    res = {"banks": [_bank(harness_bin, *c) for c in BANK_CASES], "rational": [], "rational_extra": [], "asrc": []}
    for m in MULTIPLIERS:
        for label, in_base, out_base, extra in (
            ("44.1k -> 48k", 44_100, 48_000, []),
            ("48k -> 44.1k", 48_000, 44_100, DOWN_EXTRA_HZ),
        ):
            tones = [t * m for t in TONES_HZ + extra]
            case = _run_src(harness_bin, f"src_rat_{in_base}_{out_base}_x{m}", "rational", in_base * m, out_base * m, 0.0, tones)
            case.update(label=label, mult=m, tones_base=TONES_HZ + extra)
            res["rational"].append(case)
            worst = max(r["spur_dbc"] for r in case["rows"][:4])
            print(f"  rational {label} x{m}: worst spur on 1-20 kHz tones {worst:.1f} dBc")
    for label, in_hz, out_hz, tones in RATIONAL_EXTRA:
        case = _run_src(harness_bin, f"src_rat_{in_hz}_{out_hz}", "rational", in_hz, out_hz, 0.0, tones)
        case.update(label=label, mult=1, tones_base=tones)
        res["rational_extra"].append(case)
        worst = max(r["spur_dbc"] for r in case["rows"][:4])
        print(f"  rational {label}: worst spur on 1-20 kHz tones {worst:.1f} dBc")
    for label, in_hz, out_hz, ppm in ASRC_CASES:
        scale = in_hz / (48_000 if in_hz % 48_000 == 0 else 44_100)
        tones = [t * scale for t in TONES_HZ]
        case = _run_src(harness_bin, f"src_asrc_{in_hz}_{out_hz}_{ppm:g}", "asrc", in_hz, out_hz, ppm, tones)
        case.update(label=label, ppm=ppm, tones_base=TONES_HZ)
        res["asrc"].append(case)
        worst = max(r["spur_dbc"] for r in case["rows"][:4])
        print(f"  asrc {label} {ppm:+g} ppm: worst spur on 1-20 kHz tones {worst:.1f} dBc")
    return res


def _tone_table(case):
    text = "| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |\n|---:|---:|---:|---:|---:|---:|\n"
    for base, r in zip(case["tones_base"], case["rows"]):
        text += (
            f"| {base / 1000:g} kHz | {r['gain_db']:.4f} | {r['spur_dbc']:.1f} | {r['spur_hz'] / 1000:.2f} | "
            f"{r['resid_dbc']:.1f} | {r['resid_inband_dbc']:.1f} |\n"
        )
    return text


def _summary_row(case, extra=""):
    rows = {base: r for base, r in zip(case["tones_base"], case["rows"])}
    pb = [rows[b] for b in (1000, 10000, 18000)]
    return (
        f"| {extra}| {rows[1000]['gain_db']:.4f} | {rows[18000]['gain_db']:.3f} | {rows[20000]['gain_db']:.2f} | "
        f"{rows[1000]['spur_dbc']:.1f} | {rows[10000]['spur_dbc']:.1f} | {rows[18000]['spur_dbc']:.1f} | "
        f"{max(r['spur_dbc'] for r in pb):.1f} |\n"
    )


def render(res):
    failures = []
    text = """
---

## Banks

`PolyphaseResampler` builds its bank at construction from the closed-form prototype (`fir::src_prototype`, Kaiser-windowed sinc,
parameters exported from `tools/design/design_src.py`). The band edges are relative to the **lower** of the two rates (scale
`min(1, out/in)`), so a downsampler is also the anti-alias filter: passband to 20 kHz and stopband from 24.1 kHz at 44.1 kHz, scaled
with the rate. Rational banks have one row per exact output phase; the ASRC bank has 256 rows plus guard rows and is cubic interpolated
between rows. Harness mode `src-tables` dumps the compiled bank; it is compared with `design_src.bank_row`, and the prototype is rebuilt
from the rows (`h[i*phases + p] = row_p[i]`). Gains are relative to DC at the stated frequency of the lower rate (e.g. 20 kHz for 44.1 kHz).

| Mode | In -> out (Hz) | Phases | Taps | Max abs diff vs `design_src.py` | Gain @ 18 kHz (dB) | @ 20 kHz (dB) | @ 21 kHz (dB) | @ low Nyquist (dB) | Passband deviation 0-20 kHz (dB) | Rejection >= 24.1 kHz (dB) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for e in res["banks"]:
        g = list(e["gain"].values())
        diff = e.get("max_diff")
        if design_src is not None and (diff is None or diff > BANK_MISMATCH_TOL):
            failures.append(f"Resampler bank {e['mode']} {e['in_hz']}->{e['out_hz']} differs from design_src.py")
        text += (
            f"| {e['mode']} | {e['in_hz']} -> {e['out_hz']} | {e['phases']} | {e['taps']} | "
            f"{'n/a' if diff is None else f'{diff:.1e}'} | {g[0]:.6f} | {g[1]:.6f} | {g[2]:.2f} | {g[3]:.2f} | "
            f"{e['passband_dev_db']:.1e} | {e['stop_rejection_db']:.1f} |\n"
        )
    text += """
The frequencies are those of a 44.1 kHz-family lower rate scaled to the actual one (e.g. 20 kHz at 44.1 kHz is 21.77 kHz at 48 kHz).

---

## Rational SRC (44.1 kHz family <-> 48 kHz family)

Harness mode `src-tone`: `PolyphaseResampler::new_rational(in, out)`, a coherent -6 dBFS tone (input frequency in the first
column), 131,072 output frames analysed with the Nuttall window after discarding the first 4,096. Rates are the pipeline's matched tiers
(1x = 44.1k/48k, 2x = 88.2k/96k, ... 16x = 705.6k/768k); the tone grid is scaled with the multiplier so results are
comparable. The spur is the strongest component outside the tone (image, alias or error); the residual is everything outside the tone lobe.
For an input tone above the output Nyquist the "gain" is the level of its alias; for a tone in the stopband only that column is
meaningful (the spur and residual are relative to the tiny alias). Images or aliases of 20-22.05 kHz content land in the transition band
(20-24.1 kHz), above the audio band, by design (the same convention as the half-band cascade). Output positions are exact rational phases (integer
phase accumulator, one bank row per phase), so there is no phase quantisation.
"""
    for case in res["rational"]:
        if case["mult"] != 1:
            continue
        m = case["meta"]
        text += f"""
### {case['label']} (1x): input/output ratio {m['ratio']:.6f}, {m['table_phases']} exact phases, {m['taps']} taps

""" + _tone_table(case)
    for case in res["rational_extra"]:
        m = case["meta"]
        text += f"""
### {case['label']}: input/output ratio {m['ratio']:.6f}, {m['table_phases']} exact phases, {m['taps']} taps

Reached when a PCM target is below the input's matched tier (e.g. 96 kHz input, 44.1 kHz PCM output).

""" + _tone_table(case)
    text += """
### Summary across rate tiers

Tone frequencies in the header are for the 1x tier and scale with the multiplier (spur and gain columns are per scaled tone).

| Direction | Tier | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for case in res["rational"]:
        text += _summary_row(case, f"{case['label']} | x{case['mult']} ")

    text += """
---

## ASRC (continuously variable ratio)

`PolyphaseResampler::new_asrc(in, out)` with `set_drift_ppm(ppm)`: ratio = nominal x (1 + ppm x 1e-6). The fractional position selects
four adjacent bank rows (256 phases per input sample) and the outputs of the four are combined with 4-point Lagrange weights. Live
adaptive mode (`live_pipeline.rs`) uses this with nominal ratio `input_rate / intermediate_rate` (1 when they are equal) and drift within
`max_correction_ppm` (default 500). With 0 ppm and a nominal ratio of 1 the position never leaves phase 0, so only drifting or non-unity
ratios exercise the interpolation.

### Summary

| Case | Drift | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for case in res["asrc"]:
        text += _summary_row(case, f"{case['label']} | {case['ppm']:+g} ppm ")
    phases = res["asrc"][0]["meta"]["table_phases"]
    text += f"""
Interpolation error estimate: for a prototype sampled at {phases} points per input sample, the 4-point Lagrange error for a tone at f
is about `(2*pi*f/(fs*{phases}))^4 * 0.023`, i.e. {20 * np.log10((2 * np.pi * 20_000 / 44_100 / phases) ** 4 * 0.0234):.0f} dBc at 20 kHz (44.1 kHz input):
below the prototype's own stopband, so the measured spurs are set by the prototype and the analysis, not by the interpolation.

### Tone detail, 44.1 kHz -> 44.1 kHz at +50 ppm
"""
    detail = next(c for c in res["asrc"] if c["label"] == "44.1k -> 44.1k" and c["ppm"] == 50.0)
    text += "\n" + _tone_table(detail)
    return Section(text, failures)
