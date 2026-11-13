"""Measurement sections for tools/measure/measure_spectral.py (one module per report section)."""

import sys
from pathlib import Path

_DESIGN_DIR = Path(__file__).resolve().parent.parent.parent / "design"
if str(_DESIGN_DIR) not in sys.path:
    sys.path.insert(0, str(_DESIGN_DIR))
