"""Modulator selection matrix (head of modulator.md)."""

from .analysis import floor_limited, fmt_snr
from .common import NTF_PRESET


def matrix(direct_results, oracle_results, stability_results=None):
    def get_direct(order, rate_name):
        for r in direct_results[order]:
            if r["rate"].lower() == rate_name.lower():
                return r
        return None

    d64_5 = get_direct(5, "dsd64")
    d128_5 = get_direct(5, "dsd128")
    d256_5 = get_direct(5, "dsd256")
    d512_5 = get_direct(5, "dsd512")
    d64_7 = get_direct(7, "dsd64")
    d128_7 = get_direct(7, "dsd128")
    d256_7 = get_direct(7, "dsd256")
    d512_7 = get_direct(7, "dsd512")

    def pick(r5, r7):
        """Data-driven order comparison; differences below 3 dB or at the analysis floor are not significant."""
        if not r5 or not r7:
            return "—"
        a, b = r5["snr_db"], r7["snr_db"]
        if (floor_limited(r5) and floor_limited(r7)) or abs(a - b) < 3.0:
            return "No significant difference"
        return f"{'7th' if b > a else '5th'}-order higher SNR by {abs(a - b):.0f} dB"

    def format_matrix_cell(direct_res):
        if not direct_res:
            return "N/A"
        theo = direct_res.get("theoretical_snr_db")
        if theo is not None:
            return f"**{fmt_snr(direct_res)}** (Oracle: {theo:.1f} dB)"
        return f"**{fmt_snr(direct_res)}**"

    def limit_range(order):
        vals = [r["limit_dbfs"] for r in (stability_results or {}).get(order, {}).get("input_limit", [])]
        if not vals:
            return "n/a"
        lo, hi = min(vals), max(vals)
        return f"{lo:.2f} dBFS" if lo == hi else f"{lo:.2f} to {hi:.2f} dBFS"

    msa_5_str, msa_7_str = limit_range(5), limit_range(7)

    def h_inf_range(order):
        vals = [r["h_inf"] for r in oracle_results[order].values()]
        return f"{min(vals):.2f}" if min(vals) == max(vals) else f"{max(vals):.2f} to {min(vals):.2f}"

    h_inf_5, h_inf_7 = h_inf_range(5), h_inf_range(7)

    matrix_table = f"""| Parameter / Metric | 5th-Order CRFB | 7th-Order CRFB | Comparison (this test only) |
|---|---|---|---|
| **Resonator Couplings** | 2 pairs (g1, g2) | 3 pairs (g1, g2, g3) | — |
| **Out-of-Band Gain H_inf** (preset `{NTF_PRESET}`, per rate) | {h_inf_5} | {h_inf_7} | Pinned designs (`crfb_designs.json`) |
| **Input limit L (compiled, all rates)** | {msa_5_str} | {msa_7_str} | — |
| **DSD64 (OSR ~70.6)** | {format_matrix_cell(d64_5)} | {format_matrix_cell(d64_7)} | {pick(d64_5, d64_7)} |
| **DSD128 (OSR ~141.1)** | {format_matrix_cell(d128_5)} | {format_matrix_cell(d128_7)} | {pick(d128_5, d128_7)} |
| **DSD256 (OSR ~282.2)** | {format_matrix_cell(d256_5)} | {format_matrix_cell(d256_7)} | {pick(d256_5, d256_7)} |
| **DSD512 (OSR ~564.5)** | {format_matrix_cell(d512_5)} | {format_matrix_cell(d512_7)} | {pick(d512_5, d512_7)} |

> [!NOTE]
> All measurements in this report evaluate the **compiled Rust DSP code** against the pinned CRFB designs (`tools/design/crfb_designs.json`, the source of the coefficient tables), preset `{NTF_PRESET}`.
"""
    return matrix_table
