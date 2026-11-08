"""Section 2: direct modulator (no cascade) coherent-FFT verification against the oracle."""

from dsm_analysis import analysis_floor_snr_db

from .analysis import analyze_bitstream, bitexact_mismatches, fmt_snr, floor_limited
from .common import (
    DIRECT_RATES,
    N_SAMPLES,
    ORDERS,
    Section,
    run_harness,
)


def run(harness_bin, oracle_results):
    direct_results = {order: [] for order in ORDERS}
    print("\n=== 1. Direct Modulator Coherent FFT (-6 dBFS 1 kHz sine) ===")
    for order in ORDERS:
        print(f"\n>> Modulator Order {order}:")
        for rate_str, f_bit, family in DIRECT_RATES:
            meta, bin_out = run_harness(
                harness_bin,
                f"direct_{rate_str}_ord{order}",
                [
                    "--mode", "direct-sine",
                    "--rate", rate_str,
                    "--order", str(order),
                    "--samples", str(N_SAMPLES),
                    "--amp-db", "-6.0",
                    "--dither", "none",
                ],
                with_bin=True,
            )

            metrics = analyze_bitstream(bin_out, f_bit, meta["k_bin"], N_SAMPLES)
            theo = oracle_results[order][rate_str]
            mismatches = bitexact_mismatches(bin_out, theo["synth"], N_SAMPLES, meta["k_bin"], -6.0)
            floor_db = analysis_floor_snr_db(theo["synth"], f_bit, N_SAMPLES, meta["k_bin"], -6.0)
            res = {
                "rate": rate_str.upper(),
                "order": order,
                "family": family,
                "f_bit_hz": f_bit,
                "osr": theo.get("osr", f_bit / (2.0 * 20000.0)),
                "theoretical_snr_db": theo.get("theoretical_snr_db"),
                "k_bin": meta["k_bin"],
                "f_tone_hz": meta["f_tone_hz"],
                "max_state": meta["max_state"],
                "peak_state": max(meta["max_state"]),
                "overload_count": meta["overload_count"],
                "bitexact_mismatches": mismatches,
                "analysis_floor_db": floor_db,
                **metrics,
            }
            direct_results[order].append(res)
            theo_str = f" (Oracle: {res['theoretical_snr_db']:.1f} dB)" if res["theoretical_snr_db"] is not None else ""
            print(
                f"  [Order {order}] {res['rate']:10s} ({family}): Measured SNR = {res['snr_db']:6.2f} dB{theo_str}, "
                f"SINAD = {res['sinad_db']:6.2f} dB, THD = {res['thd_db']:6.2f} dB, "
                f"Ultra1 = {res['ultra1_db']:6.2f} dB, Spur = {res['spur_dbc']:7.2f} dBc, "
                f"Peak |x| = {res['peak_state']:.4f}, "
                f"bit mismatches vs reference = {mismatches}, analysis floor = {floor_db:.1f} dB"
            )
    return direct_results


def render(direct_results):
    failed_checks = []
    report = """
---

## 2. Direct Modulator Verification (Theoretical Oracle vs. Compiled Bitstream)

Test conditions: Direct coherent 1 kHz sine at -6.0 dBFS (f_tone = k * f_bit / N), N = 1,048,576 samples (2^20), evaluating peak modulator dynamic range and noise-shaping NTF depth against the live mathematical oracle.
Verdict = stable and bit-exact against the reference loop. Quality figures here are noise-shaper limits without dither or gain staging; the quality requirements (`Design.md` §8.6) are checked on the production chain (`signal.md`).
"""

    for order in ORDERS:
        report += f"""
### 2.{order // 3} {order}th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
"""
        for r in direct_results[order]:
            is_stable = r["overload_count"] == 0 and r["peak_state"] < 50.0
            if not is_stable:
                verdict = "**FAIL (Unstable)**"
                failed_checks.append(f"Direct Order {order} {r['rate']} stability")
            elif r["bitexact_mismatches"] != 0:
                verdict = "**FAIL (Not bit-exact)**"
                failed_checks.append(f"Direct Order {order} {r['rate']} differs from the reference loop")
            else:
                verdict = "**PASS**"

            theo_snr_str = f"{r['theoretical_snr_db']:.1f} dB" if r["theoretical_snr_db"] is not None else "Unknown"
            if r["theoretical_snr_db"] is not None and not floor_limited(r):
                delta_str = f"{r['snr_db'] - r['theoretical_snr_db']:+.1f} dB"
            else:
                delta_str = "—"
            bitexact_str = "yes" if r["bitexact_mismatches"] == 0 else f"no ({r['bitexact_mismatches']} bits)"

            report += (
                f"| `{r['rate']}` | {r['family']} | {r['f_bit_hz']:,} | {r['osr']:.1f} | "
                f"{r['tone_dbfs']:.2f} | {theo_snr_str} | **{fmt_snr(r)}** | {delta_str} | {r['analysis_floor_db']:.0f} dB | "
                f"{r['sinad_db']:.2f} | {r['thd_db']:.2f} | {r['ultra1_db']:.2f} dB | {r['spur_dbc']:.1f} | {r['peak_state']:.4f} | {bitexact_str} | {verdict} |\n"
            )

    report += r"""
> [!NOTE]
> **Method** (`tools/design/dsm_analysis.py`). Each bitstream is analysed with one full-record periodic Nuttall window (4-term, continuous first derivative: -93 dB sidelobes, 18 dB/octave decay); the tone is coherent (on a bin). Noise excludes the tone lobe and harmonic lobes (bins +-4); "Worst spur" is the strongest 9-bin group in the remaining 20 Hz-20 kHz bins relative to the tone. The modulator runs without dither here, so these are noise-shaper limits.
> **Analysis floor** is the highest SNR the method resolves for that modulator's NTF: a synthetic non-periodic record with the same out-of-band noise shape and an in-band level 300 dB below the tone is analysed the same way. SNRs within 10 dB of it are printed as lower bounds and are not compared.
> **Bit-exact vs reference**: the Rust record equals, bit for bit, a Python run of the designed CRFB loop (`simulate_crfb`, coefficients from `design_crfb.py`) on the same input. The oracle SNR is that loop with a 2^18 record and small TPDF dither (0.005), analysed the same way.
> Earlier windows were invalid. A rectangular window treats the non-periodic record as periodic, so every integrator state jumps at the wrap and the out-of-band noise spreads over all bins (58-115 dB "SNR"). The 4-term Blackman-Harris window has a small edge step, decays only 6 dB/octave and floored results at ~145-150 dB.
"""
    return Section(report, failed_checks)
