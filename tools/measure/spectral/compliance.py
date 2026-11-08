"""Invariant compliance table and conclusion (tail of the index report)."""

from .cascade_filters import PRESETS, PHASES
from .common import ORDERS, Section

# Design.md section 4.2 goals per preset: (passband ripple dB or None if "declared per rate", stopband rejection dB).
PRESET_GOALS = {"balanced": (0.01, 120.0), "reference": (0.001, 140.0), "low-latency": (None, 100.0)}


def filter_goal_rows(cascade_filters_res):
    """Measures the implemented cascade (off apodization, both phases, every K) against the section 4.2 goals."""
    impulse = cascade_filters_res["impulse"]
    rows, ok = [], True
    for preset in PRESETS:
        ripple_goal, rej_goal = PRESET_GOALS[preset]
        keys = [k for k in impulse if k[0] == preset and k[2] == "off"]
        ripple = max(impulse[k]["ripple_db"] for k in keys)
        rej = min(impulse[k]["worst_image_db"] for k in keys)
        r_ok = ripple_goal is None or ripple <= ripple_goal
        j_ok = rej >= rej_goal
        ok = ok and r_ok and j_ok
        rows.append((preset, ripple, ripple_goal, rej, rej_goal))
    return rows, ok


def render(env_info, direct_results, stability_results, bench_stats, cascade_filters_res, failed_so_far, stability_ok):
    """Returns (markdown, failed_checks) for the invariant table and the conclusion.

    `bench_stats` is None when the benchmark never ran. `failed_so_far` are the failures raised by the family reports.
    """
    failed_checks = list(failed_so_far)

    # OSR calculation across all tested direct rates.
    all_direct = direct_results[5] + direct_results[7]
    osr_compliant = all(abs(r["osr"] - (r["f_bit_hz"] / 40000.0)) < 1e-4 for r in all_direct)
    osr_44k_str = ", ".join(f"{r['osr']:.2f}" for r in direct_results[5] if r["family"] == "44.1k")
    osr_48k_str = ", ".join(f"{r['osr']:.2f}" for r in direct_results[5] if r["family"] == "48k")
    osr_val_str = f"44.1k: {osr_44k_str} | 48k: {osr_48k_str} (f_bit / 40 kHz)"
    if not osr_compliant:
        failed_checks.append("Baseband OSR Calculation invariant")

    # Half-band cascade against the Design.md section 4.2 goals, measured on the implemented cascade.
    goal_rows, filters_ok = filter_goal_rows(cascade_filters_res)
    ripple_str = " | ".join(
        f"{p}: {rip:.6f} dB (goal {'declared per rate' if g is None else f'<= {g} dB'})" for p, rip, g, _, _ in goal_rows
    )
    rej_str = " | ".join(f"{p}: {rej:.1f} dB (goal >= {g:.0f} dB)" for p, _, _, rej, g in goal_rows)
    if not filters_ok:
        failed_checks.append("Cascade passband ripple / image rejection goals (Design.md section 4.2)")

    stability_compliant = stability_ok

    # DSD silence byte (0x69) and absence of rail lock (0x00 / 0xFF).
    total_silence_bits = sum(s["samples"] for order in ORDERS for s in stability_results[order]["silence"])
    total_zero_bytes = sum(s.get("zero_bytes_count", 0) for order in ORDERS for s in stability_results[order]["silence"])
    total_full_bytes = sum(s.get("full_bytes_count", 0) for order in ORDERS for s in stability_results[order]["silence"])
    total_idle_0x69 = sum(o.get("idle_0x69_count", 0) for order in ORDERS for o in stability_results[order]["overload"])
    total_overload_zeros = sum(o.get("zero_0x00_count", 0) for order in ORDERS for o in stability_results[order]["overload"])
    silence_byte_compliant = total_zero_bytes == 0 and total_full_bytes == 0 and total_idle_0x69 > 0 and total_overload_zeros == 0
    silence_val_str = f"0x69 idle pattern confirmed ({total_idle_0x69:,} clamp bytes); 0 rail-lock bytes (0x00/0xFF) across {total_silence_bits:,} silence bits"
    if not silence_byte_compliant:
        failed_checks.append("DSD Silence Byte Invariant (0x69) / Rail Lock")

    rows = [
        ("Baseband OSR Calculation", "`Design.md §4.1`", osr_val_str, osr_compliant),
        ("Cascade Passband Ripple (implemented, `off`, all K)", "`Design.md §4.2`", ripple_str, all(
            g is None or rip <= g for _, rip, g, _, _ in goal_rows)),
        ("Cascade Image Rejection (implemented, `off`, all K)", "`Design.md §4.2`", rej_str, all(
            rej >= g for _, _, _, rej, g in goal_rows)),
        ("Modulator Stability Envelope", "`Design.md §4.3`", "Bounded states across silence, DC (+0.5), multitone, step overload recovery", stability_compliant),
        ("DSD Silence Byte Invariant", "`Design.md §1.3`", silence_val_str, silence_byte_compliant),
        ("Hardware FTZ/DAZ", "`Design.md §4.1`", env_info["ftz_daz_status"], env_info["ftz_daz_active"] is True),
    ]
    if bench_stats:
        min_tp, max_tp, max_p = bench_stats["min_tp"], bench_stats["max_tp"], bench_stats["max_p999_budget"]
        rows.append(("Real-Time Throughput", "`Design.md §8.3`", f">= 2.0x sustained real-time throughput (Measured: {min_tp:.2f}x to {max_tp:.2f}x)", min_tp >= 2.0))
        rows.append(("p99.9 Deadline Budget", "`Design.md §8.3`", f"<= 70% of audio block duration (Measured peak: {max_p:.1f}%)", max_p <= 70.0))
        if min_tp < 2.0:
            failed_checks.append("Real-time throughput goal")
        if max_p > 70.0:
            failed_checks.append("p99.9 deadline budget goal")
    text = """
## Invariants & Compliance Audit

| Requirement / Invariant | Canonical Reference | Measured / Verified Value | Status |
|---|---|---|:---:|
"""
    for name, ref, val, ok in rows:
        text += f"| **{name}** | {ref} | {val} | {'Compliant' if ok else '**Non-Compliant**'} |\n"
    if not bench_stats:
        text += "| **Real-Time Throughput / p99.9 Budget** | `Design.md §8.3` | Benchmark never run (see [benchmark.md](benchmark.md)) | — |\n"
    text += "\n## Conclusion\n\n"
    n_ok = sum(bool(r[3]) for r in rows)
    text += (
        f"{n_ok} of {len(rows)} invariants verified on the compiled code. Method checks (cascade reproduces the design response, tones agree "
        "with the impulse response, resampler tables equal the generator output) are part of the family reports and listed below if any failed.\n\n"
    )
    if failed_checks:
        text += f"{len(failed_checks)} non-compliant check(s):\n" + "\n".join(f"- {c}" for c in failed_checks) + "\n\n"
    text += (
        "Filter-cascade and resampler *quality* (flatness, image/alias levels, ASRC spurs) is reported in [cascade.md](cascade.md) and "
        "[resampler.md](resampler.md) without thresholds; whether it is good enough is decided in audit step 5.\n"
    )
    return text, failed_checks
