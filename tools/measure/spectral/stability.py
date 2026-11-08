"""Section 4: stability and dynamic-envelope verification (silence, DC, multitone, overload)."""

from .common import DIRECT_RATES, DSD_RATES, N_SAMPLES, ORDERS, Section, run_harness


INPUT_LIMIT_SAMPLES = 1 << 21


def _base_args(mode, rate_str, order):
    return ["--mode", mode, "--rate", rate_str, "--order", str(order), "--samples", str(N_SAMPLES)]


def run(harness_bin):
    stability_results = {order: {"silence": [], "dc": [], "multitone": [], "overload": [], "input_limit": []} for order in ORDERS}
    print("\n=== 3. Stability & Envelope Verification ===")
    for order in ORDERS:
        print(f"\n>> Stability Verification Order {order}:")
        # Silence across all 8 rate/family cases (4 DSD rates x 44.1k/48k)
        for rate_str, _, _ in DIRECT_RATES:
            meta, _ = run_harness(
                harness_bin, f"silence_{rate_str}_ord{order}", _base_args("silence", rate_str, order), with_bin=True
            )
            stability_results[order]["silence"].append({
                "rate": rate_str.upper(),
                "order": order,
                "samples": meta["samples"],
                "ones_ratio": meta["ones_ratio"],
                "zero_bytes_count": meta.get("zero_bytes_count", 0),
                "full_bytes_count": meta.get("full_bytes_count", 0),
                "max_state": meta["max_state"],
                "overload_count": meta["overload_count"],
            })

        # DC offset (+0.5) across all 8 rate/family cases
        for rate_str, _, _ in DIRECT_RATES:
            meta, _ = run_harness(harness_bin, f"dc_{rate_str}_ord{order}", _base_args("dc", rate_str, order))
            stability_results[order]["dc"].append({
                "rate": rate_str.upper(),
                "order": order,
                "dc_val": meta["dc_val"],
                "ones_ratio": meta["ones_ratio"],
                "max_state": meta["max_state"],
                "peak_state": max(meta["max_state"]),
                "overload_count": meta["overload_count"],
            })

        # Multitone burst
        for rate_str in DSD_RATES:
            meta, _ = run_harness(
                harness_bin, f"multitone_{rate_str}_ord{order}", _base_args("multitone", rate_str, order)
            )
            stability_results[order]["multitone"].append({
                "rate": rate_str.upper(),
                "order": order,
                "max_state": meta["max_state"],
                "peak_state": max(meta["max_state"]),
                "overload_count": meta["overload_count"],
            })

        # Production modulator at its compiled input limit L (DC + tones, 2^21 bits each)
        for rate_str, _, _ in DIRECT_RATES:
            args = ["--mode", "input-limit", "--rate", rate_str, "--order", str(order), "--samples", str(INPUT_LIMIT_SAMPLES)]
            meta, _ = run_harness(harness_bin, f"limit_{rate_str}_ord{order}", args)
            stability_results[order]["input_limit"].append({"rate": rate_str.upper(), **meta})

        # Overload recovery
        for rate_str in DSD_RATES:
            meta, _ = run_harness(
                harness_bin, f"overload_{rate_str}_ord{order}", _base_args("overload", rate_str, order), with_bin=True
            )
            stability_results[order]["overload"].append({
                "rate": rate_str.upper(),
                "order": order,
                "overload_count": meta["overload_count"],
                "step_overloads": meta["step_overloads"],
                "tail_overloads": meta["tail_overloads"],
                "recovered_stable": meta["recovered_stable"],
                "idle_0x69_count": meta.get("overload_idle_0x69_count", 0),
                "zero_0x00_count": meta.get("overload_zero_0x00_count", 0),
                "tail_rail_bytes": meta["tail_rail_bytes"],
                "tail_error_rms": meta["tail_error_rms"],
                "reference_error_rms": meta["reference_error_rms"],
                "state_after_recovery": meta["state_after_recovery"],
            })
    return stability_results


def render(stability_results):
    failed_checks = []
    report = r"""
---

## 4. Stability & Dynamic Envelope Verification

Modulator stability was evaluated across four distinct dynamic edge cases per `Design.md §4.3` and `§8`:

### 4.1 Digital Silence (Zero Input)
- Evaluates balanced ones/zeros ratio and checks for absence of subnormal float decay, rail-lock sequences (0x00 / 0xFF), and idle tone spikes.
- **Criterion:** Ones ratio 0.50 +/- 0.05, 0 rail-lock bytes (0x00 / 0xFF), all states |x_i| < 50.0, 0 overloads.
"""

    for order in ORDERS:
        report += f"""
#### {order}th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_{order}] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
"""
        for s in stability_results[order]["silence"]:
            state_str = "[" + ", ".join(f"{x:.4f}" for x in s["max_state"]) + "]"
            zero_bytes = s.get("zero_bytes_count", 0)
            full_bytes = s.get("full_bytes_count", 0)
            is_pass = (
                abs(s["ones_ratio"] - 0.50) <= 0.05
                and zero_bytes == 0
                and full_bytes == 0
                and s["overload_count"] == 0
                and max(s["max_state"]) < 50.0
            )
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Silence Order {order} {s['rate']}")
            report += f"| `{s['rate']}` | {s['ones_ratio']:.4f} | {zero_bytes} / {full_bytes} | `{state_str}` | {s['overload_count']} | {verdict} |\n"

    report += r"""
### 4.2 Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak abs state | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
"""
    for order in ORDERS:
        for d in stability_results[order]["dc"]:
            is_pass = d["overload_count"] == 0 and d["peak_state"] < 50.0 and d["ones_ratio"] > 0.50
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"DC Offset Order {order} {d['rate']}")
            report += (
                f"| `{d['rate']}` | {order} | +{d['dc_val']:.2f} | {d['ones_ratio']:.4f} | "
                f"{d['peak_state']:.4f} | {d['overload_count']} | {verdict} |\n"
            )

    report += r"""
### 4.3 High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak abs state | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
"""
    for order in ORDERS:
        for m in stability_results[order]["multitone"]:
            is_pass = m["overload_count"] == 0 and m["peak_state"] < 50.0
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Multitone Order {order} {m['rate']}")
            report += (
                f"| `{m['rate']}` | {order} | 10-Tone Composite (100Hz–18kHz) | "
                f"{m['peak_state']:.4f} | {m['overload_count']} | {verdict} |\n"
            )

    report += r"""
### Input limit L (compiled, per order and rate)
- `L` is `max_stable_input_dbfs` from the coefficient tables (generated by `design_crfb.measure_msa_dbfs`: DC and 50 Hz-20 kHz sines, 0.25 dB
  level steps, 2^21 samples, Python reference loop without dither). Here the **production** modulator (`process_tile_bytes`, TPDF dither on) runs
  at exactly `L` for the same stimuli, 2^21 bits each from a reset state. DSD targets get headroom `L - M` (M = 0.5 dB), so this is 0.5 dB above
  the level a 0 dBFS PCM peak reaches.
- **Criterion:** 0 overloads for every stimulus.

| Target Rate | Order | L (dBFS) | Headroom L - M (dB) | Worst max abs state | Overloads (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Verdict |
|---|:---:|---:|---:|---:|---|:---:|
"""
    for order in ORDERS:
        for r in stability_results[order]["input_limit"]:
            ovl = [st["overloads"] for st in r["stimuli"]]
            is_pass = sum(ovl) == 0
            if not is_pass:
                failed_checks.append(f"Input limit Order {order} {r['rate']}")
            report += (
                f"| `{r['rate']}` | {order} | {r['limit_dbfs']:.2f} | {r['limit_dbfs'] - 0.5:.2f} | "
                f"{max(st['max_state'] for st in r['stimuli']):.3f} | {', '.join(map(str, ovl))} | {'**PASS**' if is_pass else '**FAIL**'} |\n"
            )

    report += r"""
### 4.4 Intentional Overload & Autonomous Recovery
- Injects a severe +5.0 step (2 tiles) into a 0.1-amplitude 1 kHz sine (timed at each target's own bit rate), triggering `OverloadPolicy::MuteRecover`.
- **Expected behavior:** the overload is recorded, integrator states are zeroed, the output falls back to the balanced DSD idle sequence (`0x69`) with no `0x00` bytes, and modulation resumes cleanly.
- **Clean resumption** is checked on the tail (after 4 settling tiles): no further overloads, no rail-lock bytes (`0x00` / `0xFF`), bounded states, and a low-passed reconstruction error
  (mean of the bits over 128-bit blocks against the mean input) of at most 1.5x that of a reference modulator that never overloaded (same seed, same sine).

| Target Rate | Order | Overloads (step / tail) | Idle 0x69 bytes in step | Tail rail bytes | Tail error rms | Reference error rms | Verdict |
|---|:---:|:---:|:---:|:---:|---:|---:|:---:|
"""
    for order in ORDERS:
        for o in stability_results[order]["overload"]:
            idle_0x69 = o.get("idle_0x69_count", 0)
            zero_0x00 = o.get("zero_0x00_count", 0)
            is_pass = (
                o["step_overloads"] > 0
                and o["tail_overloads"] == 0
                and o["recovered_stable"]
                and idle_0x69 > 0
                and zero_0x00 == 0
                and o["tail_rail_bytes"] == 0
                and o["tail_error_rms"] <= 1.5 * o["reference_error_rms"]
                and max(o["state_after_recovery"]) < 50.0
            )
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Overload Recovery Order {order} {o['rate']}")
            report += (
                f"| `{o['rate']}` | {order} | {o['step_overloads']} / {o['tail_overloads']} | {idle_0x69:,} | "
                f"{o['tail_rail_bytes']} | {o['tail_error_rms']:.2e} | {o['reference_error_rms']:.2e} | {verdict} |\n"
            )
    return Section(report, failed_checks)
