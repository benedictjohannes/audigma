"""Resampler family: the polyphase rational SRC and the ASRC, measured through `PolyphaseResampler`.

Harness modes `src-tone` (coherent tones through the production type) and `src-tables` (the exported
tables). No quality thresholds are applied (audit step 5); the only failures raised are method checks
(exported tables must equal the generator output).
"""

from fractions import Fraction

import numpy as np

from .common import Section, run_harness
from .linear import load_f64, tone_metrics

try:
    from design_src import design_polyphase_sinc
except ImportError:  # scipy missing
    design_polyphase_sinc = None

# Tone grid for the 1x case (Hz at 44.1k/48k base rates); scaled by the rate multiplier for 2x..16x.
TONES_HZ = [1000, 10000, 18000, 20000, 21000]
# 48k-family input at and above the 44.1k-family Nyquist (22.05 kHz): aliases back into 20.6-22 kHz unless rejected.
DOWN_EXTRA_HZ = [22000, 23000, 23500]
MULTIPLIERS = [1, 2, 4, 8, 16]
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
]
N_OUT = 131_072
# Table design parameters (tools/design/export_tables.py).
TABLE_CUTOFF = 0.4535
TABLE_BETA = 12.5
TABLE_MISMATCH_TOL = 0.0


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


def _delay_error(ratio, phases):
    """Largest fractional-delay error (input samples) caused by flooring positions onto the table grid.

    Output positions advance by `ratio` input samples; the fractional parts form the set
    {(j * p mod q) / q} for ratio = p / q. Each is floored to a multiple of 1/phases.
    """
    frac = Fraction(ratio).limit_denominator(2000)
    q = frac.denominator
    pos = [Fraction((j * frac.numerator) % q, q) for j in range(q)]
    err = [p - Fraction(int(p * phases), phases) for p in pos]
    return float(max(err)), q


def _tables(harness_bin):
    meta, bin_out = run_harness(harness_bin, "src_tables", ["--mode", "src-tables"], with_bin=True)
    flat = load_f64(bin_out)
    bin_out.unlink()
    taps = meta["taps"]
    rat = flat[: meta["rational_phases"] * taps].reshape(meta["rational_phases"], taps)
    asrc = flat[meta["rational_phases"] * taps :].reshape(meta["asrc_phases"], taps)
    out = {}
    for name, table in (("rational", rat), ("asrc", asrc)):
        phases = table.shape[0]
        entry = {"phases": phases, "taps": taps}
        if design_polyphase_sinc is not None:
            ref = np.array(design_polyphase_sinc(phases, taps, TABLE_CUTOFF, TABLE_BETA)["table"])
            entry["max_diff"] = float(np.max(np.abs(ref - table)))
        # Prototype reconstructed from the exported table: h[j * phases + p] = table[p, j].
        h = table.T.reshape(-1)
        nfft = 1 << 18
        spec = np.fft.rfft(h, nfft) / phases
        f = np.fft.rfftfreq(nfft, 1.0 / phases)  # in units of the input rate
        db = 20 * np.log10(np.maximum(np.abs(spec), 1e-300))
        entry["gain"] = {x: float(np.interp(x, f, db)) for x in (18_000 / 44_100, 0.4535, 21_000 / 44_100, 0.5)}
        entry["stop_rejection_db"] = float(-db[f >= 1.0 - TABLE_CUTOFF].max())
        out[name] = entry
    return out


def run(harness_bin):
    print("\n=== Resampler: PolyphaseResampler tables, rational SRC and ASRC ===")
    res = {"tables": _tables(harness_bin), "rational": [], "asrc": []}
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
    tb = res["tables"]
    text = """
---

## Exported tables

Harness mode `src-tables` dumps `SRC_RATIONAL_160_PHASES` and `SRC_ASRC_256_PHASES` as compiled. The prototype filter
is rebuilt from them (`h[j*phases + p] = table[p][j]`) and its response is given relative to the input rate. The tap and phase
counts come from `audigma_dsp::{SRC_TAPS_PER_PHASE, SRC_RATIONAL_PHASES, SRC_ASRC_PHASES}`, which are derived from the tables.

| Table | Phases | Taps/phase | Max abs diff vs `design_src.py` | Gain @ 18 kHz (dB) | @ 20 kHz (dB) | @ 21 kHz (dB) | @ Nyquist (dB) | Stopband rejection (>= 0.5465 fs_in) (dB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for name, e in tb.items():
        g = list(e["gain"].values())
        diff = e.get("max_diff")
        if diff is not None and diff > TABLE_MISMATCH_TOL:
            failures.append(f"Resampler table {name} differs from design_src.py")
        text += (
            f"| {name} | {e['phases']} | {e['taps']} | {'n/a' if diff is None else f'{diff:.1e}'} | "
            f"{g[0]:.3f} | {g[1]:.2f} | {g[2]:.2f} | {g[3]:.1f} | {e['stop_rejection_db']:.1f} |\n"
        )
    text += "\nGain is relative to DC and expressed for a 44.1 kHz input rate (frequencies scale with the input rate).\n"

    text += """
---

## Rational SRC (44.1 kHz family <-> 48 kHz family)

Harness mode `src-tone`: `PolyphaseResampler::new_rational(in, out)`, a coherent -6 dBFS tone (input frequency in the first
column), 131,072 output frames analysed with the Nuttall window after discarding the first 4,096. Rates are the pipeline's matched tiers
(1x = 44.1k/48k, 2x = 88.2k/96k, ... 16x = 705.6k/768k); the tone grid is scaled with the multiplier so results are
comparable. The spur is the strongest component outside the tone (image, alias or error); the residual is everything outside the tone lobe.
"""
    for case in res["rational"]:
        if case["mult"] != 1:
            continue
        m = case["meta"]
        err, q = _delay_error(m["ratio"], m["table_phases"])
        text += f"""
### {case['label']} (1x): input/output ratio {m['ratio']:.6f}, {m['table_phases']} table phases

Fractional output positions are multiples of 1/{q} input samples; flooring them onto the {m['table_phases']}-phase table gives a \
worst-case delay error of {err:.5f} input samples ({'on the table grid: no quantisation' if err < 1e-12 else '**not on the table grid**'}).

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

`PolyphaseResampler::new_asrc(in, out)` with `set_drift_ppm(ppm)`: ratio = nominal x (1 + ppm x 1e-6), the ASRC table indexed by the
floored fractional position. Live adaptive mode (`live_pipeline.rs`) uses this with nominal ratio `input_rate / intermediate_rate` (1 when they are equal) and drift within `max_correction_ppm` (default 500).
With 0 ppm and a nominal ratio of 1 the fractional position never leaves phase 0, so only drifting or non-unity ratios exercise the phase selection.

### Summary

| Case | Drift | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for case in res["asrc"]:
        text += _summary_row(case, f"{case['label']} | {case['ppm']:+g} ppm ")
    asrc_phases = res["asrc"][0]["meta"]["table_phases"]  # reported by the harness from audigma_dsp
    exp = 1 / asrc_phases
    rms = exp / 12**0.5

    def first_order(f_hz):
        return 20 * np.log10(2 * np.pi * f_hz / 44_100 * rms)

    text += f"""
Phase selection floors the position to 1/{asrc_phases} of an input sample (`audigma_dsp::SRC_ASRC_PHASES`), so the delay error is uniform
in [0, {exp:.5f}) samples (rms {rms:.5f}). First-order error spur for a tone at f: `2*pi*(f/fs)*err_rms`, i.e. about
{first_order(1000):.0f} dBc at 1 kHz, {first_order(10000):.0f} dBc at 10 kHz and {first_order(20000):.0f} dBc at 20 kHz (44.1 kHz input); compare the measured spur columns.

### Tone detail, 44.1 kHz -> 44.1 kHz at +50 ppm
"""
    detail = next(c for c in res["asrc"] if c["label"] == "44.1k -> 44.1k" and c["ppm"] == 50.0)
    text += "\n" + _tone_table(detail)
    return Section(text, failures)
