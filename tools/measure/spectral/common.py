"""Shared paths, constants, harness invocation and the Section result type."""

import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent.parent
TMP_DIR = REPO_ROOT / "tools" / "measure" / ".tmp_data"
REPORT_DIR = REPO_ROOT / "docs" / "measurements"
INDEX_PATH = REPORT_DIR / "README.md"

# Add tools/design to sys.path so design scripts and dsm_analysis import directly.
sys.path.insert(0, str(REPO_ROOT / "tools" / "design"))

# Quality requirements (Design.md §8.6, DSP quality audit P5). Levels are in-band (20 Hz-20 kHz) dBFS re a full-scale sine.
# Anchor 1, audibility: 0 dBFS at ~120 dB SPL peak, so noise and every discrete product stay below ~0 dB SPL.
AUDIBILITY_DR_A_DB = 120.0
AUDIBILITY_SPUR_DBFS = -120.0
# Anchor 2, 24-bit transparency: a TPDF-dithered 24-bit quantizer's in-band noise is -141.5 dBFS.
TRANSPARENCY_NOISE_DBFS = -141.0
TRANSPARENCY_SPUR_DBFS = -141.0
# Linear stages (cascade with apodization off, resampler): passband deviation 0-20 kHz.
FILTER_PASSBAND_DEV_DB = 0.01
# Production chain, tones above 10 kHz: the modulator's STF lifts 19-20 kHz at DSD64 (+0.011 / +0.015 dB, 5th / 7th).
CHAIN_FLATNESS_DB = {"A": 0.02, "B": 0.01}
# Interpolation images lie above 24.1 kHz (never in band): gated as if they were audible (anchor 1). The filters are
# designed for 140 dB (FILTER_REJECTION_TARGET_DB); that is a design goal, not the requirement.
IMAGE_REJECTION_MIN_DB = 120.0
# Documented exception: DSD64 (44.1k family) with the 5th-order modulator misses DR (A) >= 120 dB by ~1.6 dB.
TIER_EXCEPTIONS = {(5, "dsd64"): "DR (A) below the audibility anchor; documented exception (Design.md §8.6)"}


def quality_tier(rate_str):
    """'A' (DSD64, both families: audibility anchor) or 'B' (DSD128 and above: audibility + 24-bit transparency)."""
    return "A" if rate_str.split("_")[0] == "dsd64" else "B"

N_SAMPLES = 1_048_576
ORDERS = [5, 7]

# (harness rate name, f_bit in Hz, rate family): the 8 production rate/family cases.
DIRECT_RATES = [
    ("dsd64", 2822400, "44.1k"),
    ("dsd128", 5644800, "44.1k"),
    ("dsd256", 11289600, "44.1k"),
    ("dsd512", 22579200, "44.1k"),
    ("dsd64_48", 3072000, "48k"),
    ("dsd128_48", 6144000, "48k"),
    ("dsd256_48", 12288000, "48k"),
    ("dsd512_48", 24576000, "48k"),
]
DSD_RATES = ["dsd64", "dsd128", "dsd256", "dsd512"]


def unnumber(text):
    """Strips outline numbers ("## 3. ", "### 2.1 ") from markdown headings; each family file has its own outline."""
    return re.sub(r"^(#{2,4}) \d+(?:\.\d+)*\.? ", r"\1 ", text, flags=re.M)


@dataclass
class Section:
    """A rendered report section and the names of the checks that failed in it."""

    text: str
    failures: list = field(default_factory=list)


def run_cmd(cmd, cwd=REPO_ROOT):
    """Executes a command and exits on error."""
    res = subprocess.run(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if res.returncode != 0:
        print(f"Error running {' '.join(cmd)}:\n{res.stderr}", file=sys.stderr)
        sys.exit(1)
    return res.stdout


def ensure_harness_binary():
    """Builds the release measure_harness binary (crate audigma-measurements)."""
    print("Building audigma-measurements release harness...")
    run_cmd(["cargo", "build", "-p", "audigma-measurements", "--release"])
    bin_path = REPO_ROOT / "target" / "release" / "measure_harness"
    if not bin_path.exists():
        raise FileNotFoundError(f"Binary not found at {bin_path}")
    return bin_path


def run_harness(harness_bin, stem, args, *, with_bin=False):
    """Runs the harness; returns (parsed JSON metadata, path of the bitstream or None).

    Outputs land in TMP_DIR as <stem>.json (and <stem>.bin when with_bin).
    """
    json_out = TMP_DIR / f"{stem}.json"
    cmd = [str(harness_bin), *args]
    bin_out = None
    if with_bin:
        bin_out = TMP_DIR / f"{stem}.bin"
        cmd += ["--out-bin", str(bin_out)]
    cmd += ["--out-json", str(json_out)]
    run_cmd(cmd)
    with open(json_out, "r") as jf:
        meta = json.load(jf)
    return meta, bin_out
