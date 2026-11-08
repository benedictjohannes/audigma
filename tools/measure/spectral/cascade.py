"""Section 3: end-to-end half-band cascade + modulator verification across presets."""

from dsm_analysis import analysis_floor_snr_db

from .analysis import analyze_bitstream, fmt_snr
from .common import (
    DSD_RATES,
    MAX_SPUR_DBC,
    MIN_INBAND_SNR_DB,
    N_SAMPLES,
    ORDERS,
    Section,
    run_harness,
)

PRESETS = ["balanced", "reference", "low-latency"]


def run(harness_bin, oracle_results):
    cascade_results = {order: [] for order in ORDERS}
    print("\n=== 2. End-to-End Cascade + Modulator (-6 dBFS 1 kHz sine) ===")
    for order in ORDERS:
        print(f"\n>> Cascade + Modulator Order {order}:")
        for rate_str in DSD_RATES:
            for preset in PRESETS:
                meta, bin_out = run_harness(
                    harness_bin,
                    f"casc_{rate_str}_{preset}_ord{order}",
                    [
                        "--mode", "cascade-sine",
                        "--rate", rate_str,
                        "--order", str(order),
                        "--preset", preset,
                        "--samples", str(N_SAMPLES),
                        "--amp-db", "-6.0",
                        "--dither", "none",
                    ],
                    with_bin=True,
                )

                metrics = analyze_bitstream(bin_out, meta["f_bit_hz"], meta["k_bin"], N_SAMPLES)
                floor_db = analysis_floor_snr_db(
                    oracle_results[order][rate_str]["synth"], meta["f_bit_hz"], N_SAMPLES, meta["k_bin"], -6.0
                )
                last_idx = len(meta["max_state"]) - 1
                res = {
                    "rate": rate_str.upper(),
                    "order": order,
                    "preset": preset,
                    "f_bit_hz": meta["f_bit_hz"],
                    "stages": meta["cascade_stages"],
                    "max_state": meta["max_state"],
                    "last_state": meta["max_state"][last_idx],
                    "overload_count": meta["overload_count"],
                    "analysis_floor_db": floor_db,
                    **metrics,
                }
                cascade_results[order].append(res)
                print(
                    f"  [Order {order}] {res['rate']:7s} [{preset:11s}]: SNR = {res['snr_db']:6.2f} dB, "
                    f"SINAD = {res['sinad_db']:6.2f} dB, THD = {res['thd_db']:6.2f} dB, "
                    f"Ultra1 = {res['ultra1_db']:6.2f} dB, Spur = {res['spur_dbc']:7.2f} dBc, "
                    f"Stages = {res['stages']}"
                )
    return cascade_results


def render(cascade_results):
    failed_checks = []
    report = """
---

## 3. End-to-End Cascade + Modulator Verification

Test conditions: Baseband PCM 1 kHz coherent tone at -6.0 dBFS upsampled via `HalfBandCascade` (2^K linear-phase interpolation) directly into `Modulator`.
"""

    for order in ORDERS:
        report += f"""
### 3.{order // 3} {order}th-Order Modulator Pipeline

| Target Rate | Preset | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x{order} | Verdict |
|---|---|:---:|---:|---:|---:|---:|---:|---:|---:|:---:|
"""
        for r in cascade_results[order]:
            is_pass = r["overload_count"] == 0 and r["last_state"] < 50.0 and r["snr_db"] >= MIN_INBAND_SNR_DB and r["spur_dbc"] <= MAX_SPUR_DBC
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Cascade Order {order} {r['rate']} [{r['preset']}]")

            report += (
                f"| `{r['rate']}` | `{r['preset']}` | {r['stages']} | "
                f"**{fmt_snr(r)}** | {r['analysis_floor_db']:.0f} dB | {r['sinad_db']:.2f} | {r['thd_db']:.2f} | "
                f"{r['ultra1_db']:.2f} dB | {r['spur_dbc']:.1f} | {r['last_state']:.4f} | {verdict} |\n"
            )
    return Section(report, failed_checks)
