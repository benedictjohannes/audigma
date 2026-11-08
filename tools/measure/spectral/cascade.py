"""Section 3: end-to-end half-band cascade + modulator verification (apodization off and the longest apodizing stage 0)."""

from dsm_analysis import analysis_floor_snr_db

from .analysis import analyze_bitstream, fmt_snr
from .common import (
    DSD_RATES,
    N_SAMPLES,
    ORDERS,
    Section,
    run_harness,
)

FILTERS = [("linear", "off"), ("linear", "20000"), ("minimum", "off"), ("minimum", "20000")]


def run(harness_bin, oracle_results):
    cascade_results = {order: [] for order in ORDERS}
    print("\n=== 2. End-to-End Cascade + Modulator (-6 dBFS 1 kHz sine) ===")
    for order in ORDERS:
        print(f"\n>> Cascade + Modulator Order {order}:")
        for rate_str in DSD_RATES:
            for phase, apod in FILTERS:
                meta, bin_out = run_harness(
                    harness_bin,
                    f"casc_{rate_str}_{phase}_{apod}_ord{order}",
                    [
                        "--mode", "cascade-sine",
                        "--rate", rate_str,
                        "--order", str(order),
                        "--phase", phase,
                        "--apodization", apod,
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
                res = {
                    "rate": rate_str.upper(),
                    "order": order,
                    "phase": phase,
                    "apodization": apod,
                    "f_bit_hz": meta["f_bit_hz"],
                    "stages": meta["cascade_stages"],
                    "max_state": meta["max_state"],
                    "peak_state": max(meta["max_state"]),
                    "overload_count": meta["overload_count"],
                    "analysis_floor_db": floor_db,
                    **metrics,
                }
                cascade_results[order].append(res)
                print(
                    f"  [Order {order}] {res['rate']:7s} [{phase:7s} {apod:5s}]: SNR = {res['snr_db']:6.2f} dB, "
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

Test conditions: Baseband PCM 1 kHz coherent tone at -6.0 dBFS through the production `DcBlocker` and `HalfBandCascade` (2^K interpolation; linear or minimum phase, apodization `off` or pass edge 20 kHz) directly into `Modulator`.
Verdict = no overload and bounded states. The quality requirements (`Design.md` §8.6) are checked on the production chain (`signal.md`), which adds the gain staging, DC blocker and dither.
"""

    for order in ORDERS:
        report += f"""
### 3.{order // 3} {order}th-Order Modulator Pipeline

| Target Rate | Phase | Apodization | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Verdict |
|---|---|---|:---:|---:|---:|---:|---:|---:|---:|:---:|
"""
        for r in cascade_results[order]:
            is_pass = r["overload_count"] == 0 and r["peak_state"] < 50.0
            verdict = "**PASS**" if is_pass else "**FAIL**"
            if not is_pass:
                failed_checks.append(f"Cascade Order {order} {r['rate']} [{r['phase']}, {r['apodization']}]")

            report += (
                f"| `{r['rate']}` | `{r['phase']}` | `{r['apodization']}` | {r['stages']} | "
                f"**{fmt_snr(r)}** | {r['analysis_floor_db']:.0f} dB | {r['sinad_db']:.2f} | {r['thd_db']:.2f} | "
                f"{r['ultra1_db']:.2f} dB | {r['spur_dbc']:.1f} | {r['peak_state']:.4f} | {verdict} |\n"
            )
    return Section(report, failed_checks)
