"""`[resampler] modulator_ntf` presets (Design.md §4.3): every preset's compiled modulator against its pinned design.

The other reports measure the default preset only. This one checks, per preset, order and rate, what depends on the
preset: the compiled table equals the pinned design (`crfb_designs.json`, bit-exact direct sine), the production
modulator holds at the table's `L` with every dither stream of LIMIT_SEEDS (and how it fares PROBE_ABOVE_L_DB above it),
and the production chain meets the preset's target anchors (`Design.md §8.6`).
A cell whose design equals an earlier preset's is measured once and reported for both.
"""

import os
from concurrent.futures import ProcessPoolExecutor, ThreadPoolExecutor

from design_crfb import NTF_PRESETS, TARGET_ANCHORS, load_designs, preset_design

from .analysis import bitexact_mismatches
from .common import DIRECT_RATES, N_SAMPLES, ORDERS, TMP_DIR, Section, run_harness
from .signal import (
    LEVEL_CHECK_MAX_HZ,
    PRODUCTION_MARGIN_DB,
    _fmt_level,
    _fmt_ratio,
    _production_stimuli,
    _run_one,
    analysis_floor_dbfs,
    anchor_misses,
    quality_metrics,
)
from .stability import INPUT_LIMIT_SAMPLES

# Direct-sine level for the bit-exact check: -6 dBFS like the modulator report, or 1 dB below L where L is lower
# (fixed_h_inf_1_5 at 7th order), so neither loop overloads.
DIRECT_AMP_DB = -6.0
# Dither streams of the input-limit check: the production default (the harness's 12345) and 8 more. Any overload at L
# fails the run (`[gain] dsd_safety_margin_db = 0` is a supported setting, Design.md §4.4).
LIMIT_SEEDS = (12345, 1, 2, 3, 4, 5, 6, 7, 8)
# The same check one guard step above L, reported only: a margin reading (does L leave headroom in this cell?).
PROBE_ABOVE_L_DB = 0.25


def _cells():
    """(order, rate, preset, design, preset it duplicates or None) for every cell, presets in NTF_PRESETS order."""
    designs = load_designs()
    out = []
    for order in ORDERS:
        for rate_str, _, _ in DIRECT_RATES:
            seen = {}
            for preset in NTF_PRESETS:
                coeffs = designs[str(order)][rate_str.upper()]["presets"][preset]
                key = repr(coeffs["coefficients"])
                out.append((order, rate_str, preset, preset_design(designs, order, rate_str.upper(), preset),
                            coeffs.get("meets_target"), seen.get(key)))
                seen.setdefault(key, preset)
    return out


def _direct(harness_bin, order, rate_str, preset, amp_db):
    meta, bin_out = run_harness(
        harness_bin, f"preset_{preset}_direct_{rate_str}_ord{order}",
        ["--mode", "direct-sine", "--rate", rate_str, "--order", str(order), "--samples", str(N_SAMPLES),
         "--amp-db", f"{amp_db:g}", "--dither", "none"],
        with_bin=True, ntf=preset,
    )
    return meta, bin_out


def _limit(harness_bin, order, rate_str, preset, seed, margin_db):
    meta, _ = run_harness(
        harness_bin, f"preset_{preset}_limit_{rate_str}_ord{order}_seed{seed}_m{margin_db:g}",
        ["--mode", "input-limit", "--rate", rate_str, "--order", str(order), "--samples", str(INPUT_LIMIT_SAMPLES),
         "--seed", str(seed), "--margin-db", f"{margin_db:g}"],
        ntf=preset,
    )
    return meta


def run(harness_bin):
    print("\n=== NTF presets: bit-exact, input limit and quality per preset ===")
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    cells = _cells()
    measured = [c for c in cells if c[5] is None]
    stimuli = [s for s in _production_stimuli() if s[3] == "tpdf"]
    workers = max(1, (os.cpu_count() or 2) // 2)

    with ThreadPoolExecutor(workers) as ex:
        chain_jobs = {
            (o, r, p): [ex.submit(_run_one, harness_bin, o, r, *s, True, p) for s in stimuli] for o, r, p, *_ in measured
        }
        amps = {(o, r, p): min(DIRECT_AMP_DB, d["max_stable_input_dbfs"] - 1.0) for o, r, p, d, *_ in measured}
        direct_jobs = {key: ex.submit(_direct, harness_bin, *key, amps[key]) for key in amps}
        limit_jobs = {
            key: {m: [ex.submit(_limit, harness_bin, *key, s, m) for s in LIMIT_SEEDS] for m in (0.0, -PROBE_ABOVE_L_DB)}
            for key in amps
        }
        chain = {key: [f.result() for f in fs] for key, fs in chain_jobs.items()}
        direct = {key: f.result() for key, f in direct_jobs.items()}
        limit = {
            key: {"at_l": [f.result() for f in by_m[0.0]], "above": [f.result() for f in by_m[-PROBE_ABOVE_L_DB]]}
            for key, by_m in limit_jobs.items()
        }

    # The reference loop is pure Python (seconds per record): one process per record.
    designs = {(o, r, p): d for o, r, p, d, *_ in measured}
    with ProcessPoolExecutor(workers) as ex:
        futs = {
            key: ex.submit(bitexact_mismatches, bin_out, designs[key], N_SAMPLES, meta["k_bin"], amps[key])
            for key, (meta, bin_out) in direct.items()
        }
        mismatches = {key: f.result() for key, f in futs.items()}

    results = {}
    for order, rate_str, preset, design, meets_target, dup in cells:
        src = (order, rate_str, dup or preset)
        recs = chain[src]
        by_id = {(r["id"], r["margin"]): r for r in recs}
        floor = analysis_floor_dbfs(designs[src], design["f_bit_hz"], by_id[("silence", PRODUCTION_MARGIN_DB)]["gain_db"])
        results[(order, rate_str, preset)] = {
            "design": design,
            "meets_target": meets_target,
            "dup_of": dup,
            "direct_overloads": direct[src][0]["overload_count"],
            "direct_amp_db": amps[src],
            "mismatches": mismatches[src],
            "limit": limit[src],
            "records": recs,
            "floor": floor,
            "quality": quality_metrics(recs, by_id[("1k_-60", PRODUCTION_MARGIN_DB)], floor),
        }
        q = results[(order, rate_str, preset)]["quality"]
        print(f"  [{preset:16s}] Order {order} {rate_str.upper():10s}: H_inf {design['H_inf']:.2f}, "
              f"L {design['max_stable_input_dbfs']:.2f}, DR(A) {q['dr']:6.1f} dB, bit mismatches {mismatches[src]}"
              + (f" (same design as {dup})" if dup else ""))
    return results


def shortfall_count(results):
    """Cells of targeted presets whose design marks the target as missed."""
    return sum(1 for (_, _, p), r in results.items() if p in TARGET_ANCHORS and not r["meets_target"])


def _verdict(r, preset):
    """(verdict cell, failure or None) for the preset's target anchors; fixed presets have none."""
    anchors = TARGET_ANCHORS.get(preset)
    if anchors is None:
        return "—", None
    misses = anchor_misses(r["quality"], anchors)
    if r["meets_target"]:
        if misses:
            return "**FAIL** (" + "; ".join(misses) + ")", "; ".join(misses)
        return "**PASS**", None
    # The design misses the target when no point meets it throughout its H_inf window; the 1.40 fallback itself may
    # still meet it on this record set, so a shortfall cell is reported either way and never fails the run.
    chain = "; ".join(misses) if misses else "met on this chain, not throughout the design window"
    return f"**SHORTFALL** ({chain}; documented, H_inf 1.40 kept)", None


def _streams(runs):
    """Overloads per stimulus summed over the dither streams, and the seeds of the streams with any overload."""
    per_stim = [sum(x) for x in zip(*([st["overloads"] for st in m["stimuli"]] for m in runs))]
    seeds = [m["seed"] for m in runs if any(st["overloads"] for st in m["stimuli"])]
    return per_stim, seeds


def _probe_cell(runs):
    """Probe column: streams that overload above L, and at which stimuli."""
    per_stim, seeds = _streams(runs)
    if not seeds:
        return f"0/{len(runs)}"
    names = [("DC" if st["f_hz"] == 0 else f"{st['f_hz']:g} Hz") for st, n in zip(runs[0]["stimuli"], per_stim) if n]
    return f"**{len(seeds)}/{len(runs)}** ({', '.join(names)})"


def render(results):
    failures = []
    text = f"""
---

## NTF presets

Each `[resampler] modulator_ntf` preset (`Design.md §4.3`) for every order and rate, measured on the compiled code with the
table it selects (harness `--ntf`). Per cell:

- **Bit-exact:** a direct sine (no dither, {DIRECT_AMP_DB:g} dBFS or 1 dB below `L` where that is lower) equals, bit for bit,
  the Python reference loop with the pinned coefficients of that preset (`crfb_designs.json`).
- **Overloads at L:** the production modulator (TPDF dither) at exactly the table's `L`, DC and 50 Hz-20 kHz sines, 2^21 bits
  each (as in the modulator report), with {len(LIMIT_SEEDS)} dither streams (seeds {', '.join(map(str, LIMIT_SEEDS))}; 12345 is the
  production default); per stimulus, the overloads summed over the streams. Any overload fails the run.
- **Above L:** the same check {PROBE_ABOVE_L_DB:g} dB above `L` (one guard step), reported only: the streams that overload
  there and at which stimuli. Overloads here mean `L` has no measured headroom in that cell.
- **Quality:** the production chain records of the signal report (dither on, DC blocker on: level sweep incl. 0 dBFS at
  `M = 0`, silence, DC, SMPTE, CCIF), reduced the same way. The verdict is against the preset's target anchors
  (`Design.md §8.6`): `reference` audibility and 24-bit transparency at every rate, `comfortable` audibility; the fixed
  presets have no target. A **SHORTFALL** is a cell the design marks as missing its target (`meets_target` false in
  `crfb_designs.json`: no H_inf meets it throughout its window; listed in `Design.md §8.6`); it keeps H_inf 1.40 and is
  reported, not failed.
- A cell whose coefficients equal an earlier preset's is measured once; its H_inf is marked with the preset it shares.
"""
    for preset in NTF_PRESETS:
        text += f"""
### `{preset}`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Streams overloading at L + {PROBE_ABOVE_L_DB:g} | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---|---:|---:|---:|---:|---:|---|
"""
        for order in ORDERS:
            for rate_str, _, _ in DIRECT_RATES:
                r = results[(order, rate_str, preset)]
                d, q, fl = r["design"], r["quality"], r["floor"]
                tag = f"{order} {rate_str} {preset}"
                lim, lim_seeds = _streams(r["limit"]["at_l"])
                chain_ovl = sum(x["overloads"] for x in r["records"])
                if r["mismatches"] or r["direct_overloads"]:
                    failures.append(f"Presets: {tag} differs from its pinned design ({r['mismatches']} bits)")
                at_l0 = r["limit"]["at_l"][0]
                if abs(at_l0["limit_dbfs"] - d["max_stable_input_dbfs"]) > 1e-9:
                    failures.append(f"Presets: {tag} compiled L {at_l0['limit_dbfs']} != design {d['max_stable_input_dbfs']}")
                if lim_seeds:
                    failures.append(f"Presets: {tag} overloads at L (dither seeds {', '.join(map(str, lim_seeds))})")
                if chain_ovl:
                    failures.append(f"Presets: {tag} chain overloads")
                for x in r["records"]:
                    for f, dev, tol in zip(x["tone_hz"], x["level_dev_db"], x["level_tol_db"]):
                        if abs(dev) > tol:
                            failures.append(f"Presets: {tag} level error {dev:+.4f} dB at {f / 1e3:.2f} kHz ({x['id']})")
                verdict, miss = _verdict(r, preset)
                if miss:
                    failures.append(f"Presets: {tag} target: {miss}")
                h_inf = f"{d['H_inf']:.2f}" + (f" (= `{r['dup_of']}`)" if r["dup_of"] else "")
                bitexact = "yes" if r["mismatches"] == 0 else f"no ({r['mismatches']} bits)"
                text += (
                    f"| {order} | `{rate_str.upper()}` | {h_inf} | {d['max_stable_input_dbfs']:.2f} | {bitexact} | "
                    f"{', '.join(map(str, lim))} | {_probe_cell(r['limit']['above'])} | {chain_ovl} | "
                    f"{_fmt_ratio(0.0, -q['dr'], fl)} | {q['spur']:.1f} | "
                    f"{_fmt_level(q['noise'], fl)} | {_fmt_level(q['dist'], fl)} | {verdict} |\n"
                )
    text += f"""
Levels within 10 dB of the analysis floor are printed as bounds. Tone levels up to {LEVEL_CHECK_MAX_HZ / 1e3:g} kHz are
checked as in the signal report (a level error fails the run).
"""
    return Section(text, failures)
