#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "numpy",
#     "scipy",
# ]
# ///
"""Spectral verification and measurement harness for audigma-dsp (Design.md §8).

Writes one report per measurement family to docs/measurements/ plus an index (report.md):

- modulator.md: windowed coherent-FFT verification of the CRFB modulators against the synthesis oracle
  (tools/design/design_crfb.py, tools/design/dsm_analysis.py) and the stability envelope.
- cascade.md: the half-band cascade as implemented (impulse response, tone cross-check, against design_halfband.py)
  and cascade + modulator end to end.
- resampler.md: compiled SRC banks vs design_src.py, polyphase rational SRC and ASRC through PolyphaseResampler.
- signal.md: the production DSD chain (gain L - M, DC blocker, cascade, dithered modulator) with signal-dependent stimuli:
  level sweep, dither on/off, DC blocker on/off, two-tone IMD, ultrasonic noise, 44.1k vs 48k family; the quality tiers
  (Design.md §8.6).
- benchmark.md: CPU throughput and deadline budget. Part of a default run; it needs a quiet machine, so coordinate
  with the user (AGENTS.md) or pass --skip-benchmark (the report is then re-rendered from the last results kept in
  docs/measurements/benchmark.json).

Usage:
    uv run tools/measure/measure_spectral.py                       # everything, including the benchmark + index
    uv run tools/measure/measure_spectral.py --skip-benchmark      # spectral families + index, benchmark not run
    uv run tools/measure/measure_spectral.py --only resampler      # one family; the index is not updated
    uv run tools/measure/measure_spectral.py --only benchmark      # only the benchmark

Layout: this file only orchestrates; each family lives in tools/measure/spectral/ (modules with run() for measurement
and render() for markdown). Always use `uv run` (scipy is required).
"""

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from spectral import (  # noqa: E402
    bench,
    cascade,
    cascade_filters,
    compliance,
    direct,
    oracle,
    report,
    resampler,
    signal,
    stability,
    summary,
)
from spectral.common import TMP_DIR, ensure_harness_binary  # noqa: E402
from spectral.environment import get_system_environment  # noqa: E402

SPECTRAL_FAMILIES = ["modulator", "cascade", "resampler", "signal"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--only", nargs="+", choices=SPECTRAL_FAMILIES + ["benchmark"], help="run only these families")
    ap.add_argument("--skip-benchmark", action="store_true", help="do not run the CPU benchmark (the saved results are reused)")
    args = ap.parse_args()
    families = set(args.only or SPECTRAL_FAMILIES + ["benchmark"])
    if args.skip_benchmark:
        families.discard("benchmark")
    run_bench = "benchmark" in families
    full = set(SPECTRAL_FAMILIES) <= families

    harness_bin = ensure_harness_binary()
    TMP_DIR.mkdir(parents=True, exist_ok=True)

    print("\n=== Inspecting Host Runtime Environment ===")
    env_info = get_system_environment(harness_bin)
    print(f"Host CPU:       {env_info['cpu']}")
    print(f"Host OS:        {env_info['os']}")
    print(f"Rust Compiler:  {env_info['rustc']}")
    print(f"HW FTZ/DAZ:     {env_info['ftz_daz_status']}")

    need_oracle = bool(families & {"modulator", "cascade", "signal"}) or full
    oracle_results = oracle.synthesize_oracle() if need_oracle else None
    design = oracle.load_halfband_design() if "cascade" in families or full else None

    results = {}
    failures = {}
    headlines = {}
    stability_ok = True

    if "modulator" in families or full:
        direct_results = direct.run(harness_bin, oracle_results)
        stability_results = stability.run(harness_bin)
        d_sec, s_sec = direct.render(direct_results), stability.render(stability_results)
        stability_ok = not s_sec.failures
        matrix = summary.matrix(direct_results, oracle_results, stability_results)
        report.write_family("modulator", env_info, [d_sec, s_sec], preamble=f"\n## Modulator Architecture & Selection Matrix\n\n{matrix}\n")
        failures["modulator"] = d_sec.failures + s_sec.failures
        results["modulator"] = (direct_results, stability_results)
        d64_5 = next(r for r in direct_results[5] if r["rate"] == "DSD64")
        d64_7 = next(r for r in direct_results[7] if r["rate"] == "DSD64")
        headlines["modulator"] = (
            f"Stable in all stress cases; DSD64 in-band SNR {d64_5['snr_db']:.0f} dB (5th) / {d64_7['snr_db']:.0f} dB (7th); "
            f"{len(failures['modulator'])} failing check(s)"
        )

    if "cascade" in families or full:
        filt = cascade_filters.run(harness_bin, design)
        casc = cascade.run(harness_bin, oracle_results)
        f_sec, c_sec = cascade_filters.render(filt), cascade.render(casc)
        report.write_family("cascade", env_info, [f_sec, c_sec])
        failures["cascade"] = f_sec.failures + c_sec.failures
        results["cascade"] = filt
        imp = filt["impulse"]
        headlines["cascade"] = (
            f"Worst image rejection {min(m['worst_image_db'] for m in imp.values()):.1f} dB over {len(imp)} filter/stage combinations; "
            f"{len(failures['cascade'])} failing check(s)"
        )

    if "resampler" in families or full:
        res = resampler.run(harness_bin)
        r_sec = resampler.render(res)
        report.write_family("resampler", env_info, [r_sec])
        failures["resampler"] = r_sec.failures
        up = next(c for c in res["rational"] if c["label"] == "44.1k -> 48k" and c["mult"] == 1)
        down = next(c for c in res["rational"] if c["label"] == "48k -> 44.1k" and c["mult"] == 1)
        drift = next(c for c in res["asrc"] if c["ppm"] == 50.0 and c["label"] == "44.1k -> 44.1k")

        def worst_spur(case):
            return max(r["spur_dbc"] for b, r in zip(case["tones_base"], case["rows"]) if b <= 18000)

        inband = max(r["inband_error_db"] for c in res["rational"] + res["rational_extra"] + res["asrc"] for r in c["rows"])
        headlines["resampler"] = (
            f"Worst spur on 1-18 kHz tones: 44.1k->48k {worst_spur(up):.0f} dBc, 48k->44.1k {worst_spur(down):.0f} dBc, "
            f"ASRC at +50 ppm {worst_spur(drift):.0f} dBc; worst in-band error {inband:.1f} dB re input (requirement <= -141); "
            f"{len(failures['resampler'])} failing check(s)"
        )

    if "signal" in families or full:
        sig = signal.run(harness_bin, oracle_results)
        sig_sec = signal.render(sig)
        report.write_family("signal", env_info, [sig_sec])
        failures["signal"] = sig_sec.failures
        r = sig["rows"]
        dr = {o: -r[(o, "dsd64", "1k_-60", "tpdf", signal.PRODUCTION_MARGIN_DB)]["thdn_a_db"] for o in (5, 7)}
        headlines["signal"] = (
            f"Production chain: DSD64 DR (A) {dr[5]:.0f} dB (5th) / {dr[7]:.0f} dB (7th); quality tiers (§8.6) "
            f"{'met' if not any('quality requirement' in f for f in failures['signal']) else 'NOT met'}, documented exception "
            f"DSD64 5th order (DR (A)); {len(failures['signal'])} failing check(s)"
        )

    bench_results, bench_stats = bench.load()
    if run_bench:
        print("\n!! Running the CPU benchmark: the machine must be quiet (AGENTS.md). !!")
        bench_results = bench.run(harness_bin)
        bench_stats = bench.stats(bench_results)
        bench.save(bench_results, bench_stats)
    if bench_results:
        b_sec = bench.render(bench_results, bench_stats)
        report.write_family(
            "benchmark", env_info, [b_sec],
            extra=f"> **Benchmark placement:** `{bench_stats['placement']}` (measured `{bench_stats['measured_at']}`)  \n",
            generated=bench_stats["measured_at"],
        )
        failures["benchmark"] = b_sec.failures
        headlines["benchmark"] = (
            f"{bench_stats['min_tp']:.1f}x to {bench_stats['max_tp']:.1f}x real-time; p99.9 up to {bench_stats['max_p999_budget']:.1f}% of budget "
            f"(last run {bench_stats['measured_at']})"
        )
    else:
        headlines["benchmark"] = "never run (run without --skip-benchmark on a quiet machine)"

    if full:
        direct_results, stability_results = results["modulator"]
        text, all_failed = compliance.render(
            env_info, direct_results, stability_results, bench_stats, results["cascade"],
            [f for fam in failures.values() for f in fam], stability_ok,
        )
        path = report.write_index(env_info, headlines, text, bench_stats)
        print(f"\nWrote {path} and the family reports.")
    else:
        print("\nFamily report(s) written; report.md (index) is only updated by a full run.")


if __name__ == "__main__":
    main()
