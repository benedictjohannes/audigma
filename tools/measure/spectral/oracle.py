"""Design-side reference data: the synthesis oracle and the half-band design compliance."""

from design_crfb import synthesize_crfb

from .common import DIRECT_RATES, ORDERS

try:
    from design_halfband import design_all
except ImportError:
    design_all = None


def synthesize_oracle():
    """Live CRFB synthesis (design_crfb.py) for every order and rate/family case."""
    oracle_results = {order: {} for order in ORDERS}
    print("\n=== Evaluating Live Mathematical Synthesis Oracle (design_crfb.py) ===")
    for order in ORDERS:
        for rate_str, f_bit, _ in DIRECT_RATES:
            print(f"  Synthesizing theoretical Order {order} {rate_str.upper()}...")
            synth = synthesize_crfb(f_bit, order=order, H_inf=1.40, name=rate_str)
            oracle_results[order][rate_str] = {
                "synth": synth,
                "theoretical_snr_db": synth["measured_snr_db"],
                "osr": synth["osr"],
                "h_inf": synth.get("H_inf", 1.40),
            }
    return oracle_results


def load_halfband_design():
    """Design-side half-band results (design_halfband.py) for comparison with the implemented cascade."""
    if design_all is None:
        return None
    print("\n=== Evaluating half-band design (design_halfband.py) ===")
    return design_all()
