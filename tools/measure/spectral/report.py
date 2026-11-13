"""Report files: one markdown file per measurement family plus the index (README.md)."""

from .common import INDEX_PATH, REPORT_DIR, unnumber

FAMILIES = {
    "modulator": ("Modulator", "CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope."),
    "cascade": ("Cascade", "Half-band interpolation cascade as implemented (impulse response, tones) and cascade + modulator end to end."),
    "resampler": ("Resampler", "Polyphase rational SRC (44.1k <-> 48k families) and ASRC (clock-drift tracking)."),
    "signal": ("Signal", "Production DSD chain (gain L - M, DC blocker, cascade, dithered modulator): quality tiers, level sweep, dither, DC, IMD, ultrasonic noise, rate families."),
}


def family_header(key, extra=""):
    title = FAMILIES[key][0]
    return f"""# audigma — {title} Measurements

> **Specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
{extra}> **Index:** [README.md](README.md)

{FAMILIES[key][1]}
"""


def write_family(key, sections, extra="", preamble=""):
    """Writes docs/measurements/<key>.md from rendered Sections; returns the path."""
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    path = REPORT_DIR / f"{key}.md"
    text = family_header(key, extra) + preamble + "".join(unnumber(s.text) for s in sections)
    path.write_text(text)
    return path


def write_index(headlines, compliance_text):
    rows = "".join(
        f"| [{FAMILIES[k][0]}]({k}.md) | {FAMILIES[k][1]} | {headlines[k]} |\n" for k in FAMILIES
    )
    text = f"""# audigma — DSP Quality Report

> **Authoritative specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`) analysed by `tools/measure/measure_spectral.py`.

This index lists the per-family reports and the invariant check. Filter and resampler quality figures are reported
**without pass/fail thresholds**; thresholds are derived from stated requirements in the DSP quality audit's threshold step.

## Reports

| Report | Contents | Headline |
|---|---|---|
{rows}{compliance_text}"""
    INDEX_PATH.write_text(text)
    return INDEX_PATH
