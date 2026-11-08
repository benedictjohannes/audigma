"""Section 5: CPU throughput and real-time deadline budget (pinned like a runtime DSP worker)."""

import json
from datetime import datetime, timezone

from .common import DSD_RATES, ORDERS, REPORT_DIR, Section, run_harness

BENCH_JSON = REPORT_DIR / "benchmark.json"
# `off` (199-tap half-band stage 0) and the longest apodizing stage 0 (pass edge 20 kHz).
BENCH_APODS = ["off", "20000"]


def _filter_label(b):
    """Filter column of a benchmark row; results saved before the presets were removed carry `preset`."""
    if "apodization" in b:
        return f"apodization {b['apodization']}"
    return f"preset {b.get('preset', '?')} (old run)"


def run(harness_bin):
    bench_results = {order: [] for order in ORDERS}
    print("\n=== 4. Real-Time CPU Throughput & Deadline Budget ===")
    for order in ORDERS:
        print(f"\n>> Benchmark Order {order}:")
        for rate_str in DSD_RATES:
            for apod in BENCH_APODS:
                meta, _ = run_harness(
                    harness_bin,
                    f"bench_{rate_str}_{apod}_ord{order}",
                    [
                        "--mode", "benchmark",
                        "--rate", rate_str,
                        "--order", str(order),
                        "--apodization", apod,
                        "--pin",
                    ],
                )
                bench_results[order].append(meta)
                print(
                    f"  [Order {order}] {rate_str.upper():7s} [{apod:5s}]: "
                    f"Throughput = {meta['throughput_x_realtime']:5.2f}x real-time, "
                    f"Block duration = {meta['audio_block_duration_micros']:6.1f} us, "
                    f"p99.9 = {meta['p999_micros']:6.1f} us ({meta['p999_percent_of_budget']:4.1f}% of budget)"
                )
    return bench_results


def save(bench_results, bench_stats, path=BENCH_JSON):
    """Persists the results so the other reports can be regenerated without re-running the benchmark."""
    payload = {"results": {str(o): v for o, v in bench_results.items()}, "stats": bench_stats}
    path.write_text(json.dumps(payload, indent=1) + "\n")


def load(path=BENCH_JSON):
    """Returns (results, stats) from the last benchmark run, or (None, None) when never run."""
    if not path.exists():
        return None, None
    payload = json.loads(path.read_text())
    return {int(o): v for o, v in payload["results"].items()}, payload["stats"]


def stats(bench_results):
    """Aggregates used by the summary and the compliance table."""
    all_bench = bench_results[5] + bench_results[7]
    return {
        "measured_at": datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ"),
        "blocks": bench_results[5][0]["bench_blocks"] if bench_results[5] else 0,
        "placement": ", ".join(sorted({b.get("placement", "unknown") for b in all_bench})) or "n/a",
        "min_tp": min(b["throughput_x_realtime"] for b in all_bench) if all_bench else 0.0,
        "max_tp": max(b["throughput_x_realtime"] for b in all_bench) if all_bench else 0.0,
        "max_p999_budget": max(b["p999_percent_of_budget"] for b in all_bench) if all_bench else 0.0,
    }


def render(bench_results, bench_stats):
    failed_checks = []
    bench_blocks = bench_stats["blocks"]
    report = f"""
---

## 5. Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: {bench_blocks:,} blocks per configuration (p99.9 = {bench_blocks // 1000}th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`{bench_stats['placement']}`), normal scheduling class.
"""

    for order in ORDERS:
        report += f"""
### 5.{order // 3} {order}th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
"""
        for b in bench_results[order]:
            is_pass = b["throughput_x_realtime"] >= 2.0 and b["p999_percent_of_budget"] <= 70.0
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Benchmark Order {order} {b['rate']} [{_filter_label(b)}]")
            report += (
                f"| `{b['rate'].upper()}` | {_filter_label(b)} | {b['audio_block_duration_micros']:.1f} | "
                f"{b['avg_micros']:.1f} | {b['p999_micros']:.1f} | **{b['p999_percent_of_budget']:.1f}%** | "
                f"**{b['throughput_x_realtime']:.2f}x** | {verdict} |\n"
            )
    return Section(report, failed_checks)
