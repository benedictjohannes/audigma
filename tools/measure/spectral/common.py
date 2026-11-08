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
INDEX_PATH = REPORT_DIR / "report.md"

# Add tools/design to sys.path so design scripts and dsm_analysis import directly.
sys.path.insert(0, str(REPO_ROOT / "tools" / "design"))

# Pass thresholds (in-band 20 Hz-20 kHz, -6 dBFS 1 kHz tone, no dither).
MIN_INBAND_SNR_DB = 100.0
MAX_SPUR_DBC = -100.0

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
