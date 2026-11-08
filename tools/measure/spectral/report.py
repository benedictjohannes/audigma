"""Report files: one markdown file per measurement family plus the index (report.md)."""

from datetime import datetime, timezone

from .common import INDEX_PATH, REPORT_DIR, unnumber

FAMILIES = {
    "modulator": ("Modulator", "CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope."),
    "cascade": ("Cascade", "Half-band interpolation cascade as implemented (impulse response, tones) and cascade + modulator end to end."),
    "resampler": ("Resampler", "Polyphase rational SRC (44.1k <-> 48k families) and ASRC (clock-drift tracking)."),
    "benchmark": ("Benchmark", "CPU throughput and real-time deadline budget (pinned like a runtime DSP worker)."),
}


def now_str():
    return datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ")


def family_header(key, env_info, extra=""):
    title = FAMILIES[key][0]
    return f"""# audigma — {title} Measurements

> **Generated:** `{now_str()}`  
> **Evaluated on:** `{env_info['cpu']}` running `{env_info['os']}`  
> **Rust compiler:** `{env_info['rustc']}`  
> **Hardware FTZ/DAZ:** `{env_info['ftz_daz_status']}`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
{extra}> **Index:** [report.md](report.md)

{FAMILIES[key][1]}
"""


def write_family(key, env_info, sections, extra="", preamble=""):
    """Writes docs/measurements/<key>.md from rendered Sections; returns the path."""
    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    path = REPORT_DIR / f"{key}.md"
    text = family_header(key, env_info, extra) + preamble + "".join(unnumber(s.text) for s in sections)
    path.write_text(text)
    return path


def write_index(env_info, headlines, compliance_text, bench_stats):
    bench_line = (
        f"last run `{bench_stats['measured_at']}`, placement `{bench_stats['placement']}`"
        if bench_stats
        else "never run"
    )
    rows = "".join(
        f"| [{FAMILIES[k][0]}]({k}.md) | {FAMILIES[k][1]} | {headlines[k]} |\n" for k in FAMILIES
    )
    text = f"""# audigma — DSP Quality Report

> **Authoritative specification:** `Design.md §4` and `§8`.  
> **Generated:** `{now_str()}` on `{env_info['cpu']}` running `{env_info['os']}`  
> **Rust compiler:** `{env_info['rustc']}`  
> **Hardware FTZ/DAZ:** `{env_info['ftz_daz_status']}`  
> **Benchmark:** {bench_line}  
> **Harness:** `crates/audigma-measurements` (`measure_harness`) analysed by `tools/measure/measure_spectral.py`.

This index lists the per-family reports and the invariant check. Filter and resampler quality figures are reported
**without pass/fail thresholds**; thresholds are derived from stated requirements in the DSP quality audit's threshold step.

## Reports

| Report | Contents | Headline |
|---|---|---|
{rows}{compliance_text}"""
    INDEX_PATH.write_text(text)
    return INDEX_PATH
