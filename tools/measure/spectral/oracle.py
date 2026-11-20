"""Design-side reference data: the pinned CRFB designs (the oracle) and the half-band design compliance."""

from design_crfb import load_designs, oracle_snr_db, preset_design

from .common import DIRECT_RATES, NTF_PRESET, ORDERS

try:
    from design_halfband import design_all
except ImportError:
    design_all = None


def synthesize_oracle():
    """The committed designs (`crfb_designs.json`) of `NTF_PRESET` for every order and rate/family case.

    These are the coefficients the Rust tables were generated from; nothing is re-synthesized, so the oracle cannot
    drift from the tables with a different numpy/scipy.
    """
    designs = load_designs()
    oracle_results = {order: {} for order in ORDERS}
    print(f"\n=== Loading pinned CRFB designs (crfb_designs.json, preset {NTF_PRESET}) ===")
    for order in ORDERS:
        for rate_str, _, _ in DIRECT_RATES:
            print(f"  Order {order} {rate_str.upper()}...")
            synth = preset_design(designs, order, rate_str.upper(), NTF_PRESET)
            oracle_results[order][rate_str] = {
                "synth": synth,
                "theoretical_snr_db": oracle_snr_db(synth),
                "osr": synth["osr"],
                "h_inf": synth["H_inf"],
                "limit_dbfs": synth["max_stable_input_dbfs"],
            }
    return oracle_results


def load_halfband_design():
    """Design-side half-band results (design_halfband.py) for comparison with the implemented cascade."""
    if design_all is None:
        return None
    print("\n=== Evaluating half-band design (design_halfband.py) ===")
    return design_all()
