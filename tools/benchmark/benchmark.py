#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""CPU throughput and real-time deadline budget benchmarking harness (Design.md §8.5).

Two sections, both single channel on one pinned core:
  - Modulator: half-band cascade + CRFB modulator across DSD rates and apodization filters.
  - Full pipeline: the production worker input stage (DSD decode, SRC, cascade, modulator) for
    PCM44.1 / PCM96 / DSD64 input to PCM384 / DSD256 / DSD512 output, for the lightest and the
    heaviest filter + modulator configuration.

Writes JSON telemetry and Markdown reports to docs/benchmarks/benchmark.<tag>.{json,md}.
Git tracks *.reference.* and .gitignore in docs/benchmarks; local runs (e.g. tag=current)
are ignored by default.

Usage:
    uv run tools/benchmark/benchmark.py                               # default run (tag=current, compare=reference)
    uv run tools/benchmark/benchmark.py --tag=reference              # official maintainer reference run
    uv run tools/benchmark/benchmark.py --source=reference            # re-render reference markdown from saved JSON
    uv run tools/benchmark/benchmark.py --source=current --compare=reference # re-render with comparison
"""

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

# Add tools/measure to path for shared harness and environment utilities
REPO_ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO_ROOT / "tools" / "measure"))

from spectral.common import DSD_RATES, ORDERS, ensure_harness_binary, run_harness  # noqa: E402
from spectral.environment import get_system_environment  # noqa: E402

BENCHMARKS_DIR = REPO_ROOT / "docs" / "benchmarks"
BENCH_APODS = ["off", "20000"]

# Goal for stereo on one core: 2x real-time headroom with p99.9 <= 70% of the deadline budget.
# Stereo costs twice the mono time, so the mono measurements must reach 4x and 35%.
STEREO_GOAL_X_RT = 2.0
STEREO_GOAL_P999_PCT = 70.0
GOAL_X_RT = 2 * STEREO_GOAL_X_RT
GOAL_P999_PCT = STEREO_GOAL_P999_PCT / 2

# (harness name, label)
PIPELINE_INPUTS = [("pcm44", "PCM 44.1 kHz"), ("pcm96", "PCM 96 kHz"), ("dsd64", "DSD64")]
PIPELINE_OUTPUTS = [("pcm384", "PCM 384 kHz"), ("dsd256", "DSD256"), ("dsd512", "DSD512")]
# Lightest and heaviest combination of the chain.
PIPELINE_CONFIGS = [
    {"key": "light", "phase": "linear", "apodization": "off", "order": 5,
     "title": "Linear Phase, Apodization Off, (DSD output) 5th-Order Modulator"},
    {"key": "heavy", "phase": "minimum", "apodization": "20000", "order": 7,
     "title": "Minimum Phase, Apodization 20kHz, (DSD output) 7th-Order Modulator"},
]


def _filter_label(b):
    if "apodization" in b:
        return f"apodization {b['apodization']}"
    return f"preset {b.get('preset', '?')} (old run)"


def _passes_goal(b):
    return b["throughput_x_realtime"] >= GOAL_X_RT and b["p999_percent_of_budget"] <= GOAL_P999_PCT


def resolve_bench_path(tag_or_path: str) -> Path:
    p = Path(tag_or_path)
    if p.is_file():
        return p
    tagged = BENCHMARKS_DIR / f"benchmark.{tag_or_path}.json"
    if tagged.is_file():
        return tagged
    as_file = BENCHMARKS_DIR / tag_or_path
    if as_file.is_file():
        return as_file
    return tagged


def load_benchmark(path: Path):
    if not path.is_file():
        raise FileNotFoundError(f"Benchmark file not found: {path}")
    payload = json.loads(path.read_text())
    if not isinstance(payload, dict):
        raise ValueError(f"Malformed benchmark JSON in {path}: root is not an object")
    missing = [k for k in ("modulator", "pipeline", "stats", "environment") if k not in payload]
    if missing:
        raise ValueError(f"Malformed benchmark JSON in {path}: missing required key(s): {', '.join(missing)}")
    modulator = {int(o): v for o, v in payload["modulator"].items()}
    return modulator, payload["pipeline"], payload["stats"], payload["environment"]


def save_benchmark(path: Path, modulator, pipeline, bench_stats, env_info):
    payload = {
        "modulator": {str(o): v for o, v in modulator.items()},
        "pipeline": pipeline,
        "stats": bench_stats,
        "environment": env_info,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=1) + "\n")


def _summarize(rows):
    return {
        "min_tp": min(b["throughput_x_realtime"] for b in rows) if rows else 0.0,
        "max_tp": max(b["throughput_x_realtime"] for b in rows) if rows else 0.0,
        "max_p999_budget": max(b["p999_percent_of_budget"] for b in rows) if rows else 0.0,
    }


def compute_stats(modulator, pipeline):
    mod_rows = [b for order in sorted(modulator) for b in modulator[order]]
    all_rows = mod_rows + pipeline
    return {
        "measured_at": datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ"),
        "blocks": all_rows[0]["bench_blocks"] if all_rows else 0,
        "placement": ", ".join(sorted({b.get("placement", "unknown") for b in all_rows})) or "n/a",
        "modulator": _summarize(mod_rows),
        "pipeline": _summarize(pipeline),
    }


def run_modulator(harness_bin, pin, blocks):
    results = {order: [] for order in ORDERS}
    print("\n=== Modulator Benchmark (single channel) ===")
    extra = (["--pin"] if pin else []) + (["--blocks", str(blocks)] if blocks else [])
    for order in ORDERS:
        print(f"\n>> Benchmark Order {order}:")
        for rate_str in DSD_RATES:
            for apod in BENCH_APODS:
                meta, _ = run_harness(
                    harness_bin,
                    f"bench_{rate_str}_{apod}_ord{order}",
                    ["--mode", "benchmark", "--rate", rate_str, "--order", str(order), "--apodization", apod] + extra,
                )
                results[order].append(meta)
                print(
                    f"  [Order {order}] {rate_str.upper():7s} [{apod:5s}]: "
                    f"Throughput = {meta['throughput_x_realtime']:5.2f}x real-time, "
                    f"Block duration = {meta['audio_block_duration_micros']:6.1f} us, "
                    f"p99.9 = {meta['p999_micros']:6.1f} us ({meta['p999_percent_of_budget']:4.1f}% of budget)"
                )
    return results


def run_pipeline(harness_bin, pin, blocks):
    rows = []
    print("\n=== Full Pipeline Benchmark (single channel) ===")
    extra = (["--pin"] if pin else []) + (["--blocks", str(blocks)] if blocks else [])
    for cfg in PIPELINE_CONFIGS:
        print(f"\n>> {cfg['title']}:")
        for in_key, in_label in PIPELINE_INPUTS:
            for out_key, out_label in PIPELINE_OUTPUTS:
                meta, _ = run_harness(
                    harness_bin,
                    f"pipe_{cfg['key']}_{in_key}_{out_key}",
                    [
                        "--mode", "benchmark-pipeline",
                        "--input", in_key,
                        "--output", out_key,
                        "--phase", cfg["phase"],
                        "--apodization", cfg["apodization"],
                        "--order", str(cfg["order"]),
                    ] + extra,
                )
                meta["config"] = cfg["key"]
                rows.append(meta)
                print(
                    f"  {in_label:12s} -> {out_label:11s}: "
                    f"Throughput = {meta['throughput_x_realtime']:6.2f}x real-time, "
                    f"p99.9 = {meta['p999_micros']:7.1f} us ({meta['p999_percent_of_budget']:4.1f}% of {meta['audio_block_duration_micros']:.0f} us budget)"
                )
    return rows


def build_comparison_lookup(compare_modulator):
    if not compare_modulator:
        return {}
    lookup = {}
    for order, rows in compare_modulator.items():
        for r in rows:
            apod = r.get("apodization", r.get("preset", ""))
            lookup[(int(order), r["rate"].lower(), str(apod))] = r
    return lookup


def build_pipeline_lookup(compare_pipeline):
    return {(r["config"], r["input"], r["output"]): r for r in compare_pipeline or []}


def render_markdown(tag, env_info, modulator, pipeline, bench_stats,
                    compare_tag=None, compare_modulator=None, compare_pipeline=None,
                    compare_stats=None, compare_env=None):
    bench_blocks = bench_stats.get("blocks", 20000)
    tag_suffix = "" if tag == "reference" else f" (`{tag}`)"
    tag_badge = "> **Tag:** `reference` (Official Reference Baseline)  \n" if tag == "reference" else f"> **Tag:** `{tag}`  \n"

    compare_header_line = ""
    has_comparison = bool(compare_tag and compare_modulator)
    mod_lookup = build_comparison_lookup(compare_modulator) if has_comparison else {}
    pipe_lookup = build_pipeline_lookup(compare_pipeline) if has_comparison else {}
    if compare_tag and compare_stats:
        base_cpu = (compare_env or {}).get("cpu", "unknown")
        compare_header_line = f"> **Comparison Baseline (`{compare_tag}`):** `{base_cpu}` (measured `{compare_stats.get('measured_at', 'unknown')}`)  \n"

    placement = bench_stats.get("placement", "unknown")
    space2 = "  "
    report = f"""# audigma — Benchmark Measurements{tag_suffix}

> **Generated:** `{bench_stats.get('measured_at', 'unknown')}`{space2}
> **Evaluated on:** `{env_info.get('cpu', 'unknown')}` running `{env_info.get('os', 'unknown')}`{space2}
> **Rust compiler:** `{env_info.get('rustc', 'unknown')}`{space2}
> **Hardware FTZ/DAZ:** `{env_info.get('ftz_daz_status', 'unknown')}`{space2}
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/benchmark/benchmark.py`{space2}
{tag_badge}> **Benchmark placement:** `{placement}` (measured `{bench_stats.get('measured_at', 'unknown')}`){space2}
{compare_header_line}> **DSP Quality Report:** [../measurements/README.md](../measurements/README.md)

## What the numbers show

- CPU throughput and real-time deadline budget.
- Pinned like a runtime DSP worker (`{placement}`, normal scheduling class).
- Each configuration runs {bench_blocks:,} timed blocks; p99.9 is the {bench_blocks // 1000}th-worst block.
- Every measurement is a single channel on a single core.

The benchmark excludes processing outside the DSP audio pipeline, such as:
- Worker hand-off between threads.
- DoP packing and sample format conversion.
- Audio I/O and transport overhead.

## The Target Headroom: Stereo on One Core

Most listening is stereo, and stereo on a single core takes twice the mono time. The target is defined for stereo on one core:

> **at least 2x real-time headroom** with a **p99.9 block time of at most 70% of the deadline budget**.

For mono measurements, the thresholds below determine the PASS/FAIL verdicts and marks:
- **Throughput:** at least **{GOAL_X_RT:.0f}x** real-time
- **p99.9 Budget Used:** at most **{GOAL_P999_PCT:.0f}%** of the block deadline

In runtime, audigma can utilize multiple DSP workers, where channels run in parallel on separate cores. As such, the single-core measurement shown below represents the conservative worst case.

---

## Modulator Benchmarks

This section shows the modulator-only benchmark: the half-band cascade (`HalfBandCascade`, driven directly), followed by the CRFB delta-sigma modulator, measured on one channel with a synthetic tone.

The modulator applies only to DSD output and constitutes the heaviest stage of the DSP chain; it excludes DSD input decimation and rational sample rate conversion.

- **Benchmark Parameters:**
  - *Tile Size:* Blocks of 256 baseband frames (5.805 ms audio duration at 44.1 kHz), matching the production tile.
  - *Subtiles:* Executed in the worker's cache-sized subtiles (256 frames at DSD64, 128 at DSD128, 64 at DSD256, 32 at DSD512).
  - *Block Budget:* Audio duration of the tile.
"""

    for order in ORDERS:
        report += f"\n### {order}th-Order\n\n"
        if has_comparison:
            report += (
                f"| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | "
                f"p99.9 Budget Used | Throughput (x Real-Time) | vs {compare_tag} | Goal (>= {GOAL_X_RT:.0f}x, <= {GOAL_P999_PCT:.0f}%) |\n"
                f"|---|---|---:|---:|---:|---:|---:|---:|:---:|\n"
            )
        else:
            report += (
                f"| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | "
                f"p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= {GOAL_X_RT:.0f}x, <= {GOAL_P999_PCT:.0f}%) |\n"
                f"|---|---|---:|---:|---:|---:|---:|:---:|\n"
            )

        for b in modulator[order]:
            verdict = "**PASS**" if _passes_goal(b) else "**FAIL**"
            row = (
                f"| `{b['rate'].upper()}` | {_filter_label(b)} | {b['audio_block_duration_micros']:.1f} | "
                f"{b['avg_micros']:.1f} | {b['p999_micros']:.1f} | **{b['p999_percent_of_budget']:.1f}%** | "
                f"**{b['throughput_x_realtime']:.2f}x** | "
            )
            if has_comparison:
                key = (order, b["rate"].lower(), str(b.get("apodization", b.get("preset", ""))))
                ref_row = mod_lookup.get(key)
                if ref_row and ref_row.get("throughput_x_realtime", 0) > 0:
                    ref_tp = ref_row["throughput_x_realtime"]
                    row += f"**{b['throughput_x_realtime'] / ref_tp:.2f}x** ({ref_tp:.2f}x) | "
                else:
                    row += "— | "
            report += row + f"{verdict} |\n"

    report += f"""
---

## Full Pipeline Benchmarks

This section shows the production worker benchmark as executed by `audigma run` on a single calling thread (one channel, no worker hand-off), using blocks of {pipeline[0]['baseband_block_frames'] if pipeline else 256} baseband frames matching the live loop.

Per audio block, the pipeline executes the following stages:
- **DSD Unpack & Decimation:** Raw DSD decoded to its intermediate PCM rate (176.4 kHz for DSD64). *Decimation is not applied for PCM input.*
- **Rational SRC:** Sample rate conversion applied when rate families differ or the intermediate rate differs from the target (e.g., 44.1 kHz or DSD64 to PCM 384 kHz, or 96 kHz to DSD256).
- **Half-Band Cascade:** Multi-stage upsampling cascade.
- **Modulator:** CRFB delta-sigma modulator for DSD output. *Modulator is not applied for PCM output.*

The two tables below evaluate the lightest and heaviest combinations of the pipeline:
- Each cell shows **p99.9 budget used / throughput (x real-time)**.
- A trailing **✗** marks a cell that misses the mono goal (>= {GOAL_X_RT:.0f}x, <= {GOAL_P999_PCT:.0f}%).
"""

    out_labels = {k: v for k, v in PIPELINE_OUTPUTS}
    in_labels = {k: v for k, v in PIPELINE_INPUTS}
    for cfg in PIPELINE_CONFIGS:
        report += f"\n### {cfg['title']}\n\n"
        report += "| Input | Block Budget (us) | " + " | ".join(out_labels[k] for k, _ in PIPELINE_OUTPUTS) + " |\n"
        report += "|---|---:|" + "---:|" * len(PIPELINE_OUTPUTS) + "\n"
        rows = {(r["input"], r["output"]): r for r in pipeline if r["config"] == cfg["key"]}
        for in_key, _ in PIPELINE_INPUTS:
            budget = next((rows[(in_key, o)]["audio_block_duration_micros"] for o, _ in PIPELINE_OUTPUTS if (in_key, o) in rows), None)
            line = f"| {in_labels[in_key]} | {budget:.1f} |" if budget is not None else f"| {in_labels[in_key]} | — |"
            for out_key, _ in PIPELINE_OUTPUTS:
                r = rows.get((in_key, out_key))
                if r is None:
                    line += " — |"
                    continue
                cell = f"{r['p999_percent_of_budget']:.1f}% / {r['throughput_x_realtime']:.2f}x"
                cell = f"**{cell}**" if _passes_goal(r) else f"**{cell} ✗**"
                ref = pipe_lookup.get((cfg["key"], in_key, out_key))
                if ref and ref.get("throughput_x_realtime", 0) > 0:
                    cell += f"<br>{r['throughput_x_realtime'] / ref['throughput_x_realtime']:.2f}x vs {compare_tag} ({ref['throughput_x_realtime']:.2f}x)"
                line += f" {cell} |"
            report += line + "\n"

    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--tag",
        default=None,
        help="tag for benchmark output files (default: 'current', or inferred from --source if set)",
    )
    parser.add_argument(
        "--compare",
        default="reference",
        help="tag or JSON path to compare against (default: 'reference'; empty disables)",
    )
    parser.add_argument(
        "--source",
        default=None,
        help="tag or JSON path to load results from instead of re-running the benchmark",
    )
    parser.add_argument(
        "--no-pin",
        action="store_true",
        help="do not pin benchmark worker thread to highest-performance core",
    )
    parser.add_argument(
        "--blocks",
        type=int,
        default=None,
        help="timed blocks per configuration (default: harness default, 20000); smaller values are for smoke runs",
    )
    args = parser.parse_args()

    BENCHMARKS_DIR.mkdir(parents=True, exist_ok=True)

    # 1. Resolve source data or run harness
    harness_bin = None
    if args.source:
        source_path = resolve_bench_path(args.source)
        if not source_path.is_file():
            sys.exit(f"Error: source benchmark file '{args.source}' not found at {source_path}")
        try:
            modulator, pipeline, bench_stats, env_info = load_benchmark(source_path)
        except Exception as e:
            sys.exit(f"Error loading source benchmark {source_path}: {e}")

        # Infer tag from source if not provided
        if args.tag:
            tag = args.tag
        elif source_path.name.startswith("benchmark.") and source_path.name.endswith(".json"):
            tag = source_path.name[len("benchmark.") : -len(".json")]
        else:
            tag = "current"
        print(f"Loaded benchmark data from {source_path} (tag: {tag})")
    else:
        tag = args.tag or "current"
        harness_bin = ensure_harness_binary()
        env_info = get_system_environment(harness_bin)
        print("\n=== Inspecting Host Runtime Environment ===")
        print(f"Host CPU:       {env_info['cpu']}")
        print(f"Host OS:        {env_info['os']}")
        print(f"Rust Compiler:  {env_info['rustc']}")
        print(f"HW FTZ/DAZ:     {env_info['ftz_daz_status']}")

        pin = not args.no_pin
        modulator = run_modulator(harness_bin, pin, args.blocks)
        pipeline = run_pipeline(harness_bin, pin, args.blocks)
        bench_stats = compute_stats(modulator, pipeline)

    # 2. Resolve comparison baseline if requested
    compare_modulator = None
    compare_pipeline = None
    compare_stats = None
    compare_env = None
    compare_tag = args.compare

    if compare_tag and compare_tag != tag:
        compare_path = resolve_bench_path(compare_tag)
        if compare_path.is_file():
            try:
                compare_modulator, compare_pipeline, compare_stats, compare_env = load_benchmark(compare_path)
                print(f"Loaded comparison baseline '{compare_tag}' from {compare_path}")
            except Exception as e:
                sys.exit(f"Error loading comparison baseline {compare_path}: {e}")
        else:
            if compare_tag == "reference":
                # Default reference baseline is optional if never created
                print(f"Notice: reference baseline '{compare_path}' not found; skipping comparison.")
                compare_tag = None
            else:
                sys.exit(f"Error: comparison baseline '{compare_tag}' not found at {compare_path}")
    else:
        # Same tag: comparison disabled
        compare_tag = None

    # 3. Save JSON results if newly run or writing to a different tag
    out_json = BENCHMARKS_DIR / f"benchmark.{tag}.json"
    if not args.source or out_json.resolve() != source_path.resolve():
        save_benchmark(out_json, modulator, pipeline, bench_stats, env_info)
        print(f"Wrote benchmark telemetry: {out_json}")

    # 4. Render and write Markdown report
    md_content = render_markdown(
        tag,
        env_info,
        modulator,
        pipeline,
        bench_stats,
        compare_tag=compare_tag,
        compare_modulator=compare_modulator,
        compare_pipeline=compare_pipeline,
        compare_stats=compare_stats,
        compare_env=compare_env,
    )
    out_md = BENCHMARKS_DIR / f"benchmark.{tag}.md"
    out_md.write_text(md_content)
    print(f"Wrote benchmark report:    {out_md}")

    # 5. Print summary
    for name in ("modulator", "pipeline"):
        s = bench_stats.get(name, {})
        print(
            f"Summary [{tag}, {name}]: throughput {s.get('min_tp', 0.0):.1f}x to {s.get('max_tp', 0.0):.1f}x real-time; "
            f"peak p99.9 budget: {s.get('max_p999_budget', 0.0):.1f}%"
        )


if __name__ == "__main__":
    main()
