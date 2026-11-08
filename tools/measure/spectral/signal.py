"""Signal-dependent measurements of the production DSD chain (DSP quality audit, plan step 4).

Every record runs the chain as `audigma run` builds it with the default settings (`Design.md §2`, §4.3-§4.4):
baseband PCM -> profile gain L - M (M = `[gain] dsd_safety_margin_db`, default 0.25) -> `DcBlocker` (1 Hz) -> `HalfBandCascade`
(linear phase, apodization off) -> `Modulator` with TPDF dither (0.005, the production default), harness mode `chain`.
Levels are relative to the PCM input's full scale (a 0 dBFS PCM sine reaches the modulator at L - M).

Stimuli: a 1 kHz level sweep (0 dBFS at M = 0 and M = 0.25 down to -120 dBFS), digital silence and small DC offsets,
the same low-level set without dither, two-tone IMD (SMPTE 60 Hz + 7 kHz 4:1, CCIF 19 + 20 kHz 1:1) at 0 dBFS peak, and the
DC offsets once more with the DC blocker off (what the chain did before O7).
Quality requirements (`Design.md` §8.6) are checked per tier: A (DSD64) audibility, B (DSD128+) audibility and 24-bit
transparency; DSD64 5th order (44.1k) is a documented exception for DR (A). Method/invariant checks also fail the run.
"""

import os
from concurrent.futures import ThreadPoolExecutor

import numpy as np

from dsm_analysis import FLOOR_MARGIN_DB, analysis_floor_snr_db

from .analysis import SPUR_LOCAL_BINS, analyze_components, spur_prominence_noise_db
from .common import (
    AUDIBILITY_DR_A_DB,
    AUDIBILITY_SPUR_DBFS,
    CHAIN_FLATNESS_DB,
    DIRECT_RATES,
    N_SAMPLES,
    ORDERS,
    TIER_EXCEPTIONS,
    TMP_DIR,
    TRANSPARENCY_NOISE_DBFS,
    TRANSPARENCY_SPUR_DBFS,
    Section,
    quality_tier,
    run_harness,
)

PRODUCTION_MARGIN_DB = 0.25
SWEEP_DBFS = [0.0, -1.0, -3.0, -6.0, -10.0, -20.0, -40.0, -60.0, -80.0, -100.0, -120.0]
NO_DITHER_DBFS = [-6.0, -60.0, -100.0, -120.0]
DC_DBFS = [-40.0, -80.0]
# The DC blocker must leave at most this fraction of a DC offset (method check for O7; the blocker is settled for 30 time
# constants, -261 dB, so anything left is the record's own low-frequency noise).
DC_REMOVED_DB = 60.0
# Measured tone level must match the requested one, within a tolerance plus the reading error the noise inside the tone
# lobe allows (3 sigma, amplitude). Tones up to LEVEL_CHECK_MAX_HZ: LEVEL_TOL_DB (method check: gain staging, cascade
# DC gain, DC blocker). Above it: the chain flatness requirement CHAIN_FLATNESS_DB of the rate's tier (the modulator's
# signal transfer function lifts 19-20 kHz by up to +0.015 dB at DSD64).
LEVEL_TOL_DB = 0.01
LEVEL_CHECK_MAX_HZ = 10_000.0


def _level_tolerance_db(lobe_snr_db, tol_db=LEVEL_TOL_DB):
    return tol_db + 20 * np.log10(1 + 3 * 10 ** (-lobe_snr_db / 20))
# Two-tone records are longer so the SMPTE 60 Hz sidebands are resolved at every rate (bin ~2.7-2.9 Hz).
RATE_MULT = {"dsd64": 1, "dsd128": 2, "dsd256": 4, "dsd512": 8}

IMD_STIMULI = {
    # Peak sum = 0 dBFS: SMPTE 4:1 (0.8 + 0.2), CCIF 1:1 (0.5 + 0.5).
    "smpte": ("SMPTE 60 Hz + 7 kHz (4:1)", [(60.0, 20 * np.log10(0.8)), (7000.0, 20 * np.log10(0.2))]),
    "ccif": ("CCIF 19 kHz + 20 kHz (1:1)", [(19000.0, 20 * np.log10(0.5)), (20000.0, 20 * np.log10(0.5))]),
}


def _stimuli():
    """(id, tones, margin, dither, long record, DC blocker) of every record per rate/order."""
    return [(*s, True) for s in _production_stimuli()] + [
        (f"dc_{db:g}_unblocked", [(0.0, db)], PRODUCTION_MARGIN_DB, "tpdf", False, False) for db in DC_DBFS
    ]


def _production_stimuli():
    out = [("1k_L", [(1000.0, 0.0)], 0.0, "tpdf", False)]
    out += [(f"1k_{db:g}", [(1000.0, db)], PRODUCTION_MARGIN_DB, "tpdf", False) for db in SWEEP_DBFS]
    out += [("silence", [], PRODUCTION_MARGIN_DB, "tpdf", False)]
    out += [(f"dc_{db:g}", [(0.0, db)], PRODUCTION_MARGIN_DB, "tpdf", False) for db in DC_DBFS]
    out += [(f"1k_{db:g}", [(1000.0, db)], PRODUCTION_MARGIN_DB, "none", False) for db in NO_DITHER_DBFS]
    out += [("silence", [], PRODUCTION_MARGIN_DB, "none", False)]
    out += [(f"dc_{db:g}", [(0.0, db)], PRODUCTION_MARGIN_DB, "none", False) for db in DC_DBFS]
    out += [(sid, tones, PRODUCTION_MARGIN_DB, "tpdf", True) for sid, (_, tones) in IMD_STIMULI.items()]
    return out


def _tones_arg(tones):
    return ",".join(f"{f:g}:{db:.6f}" for f, db in tones) or "none"


def _run_one(harness_bin, order, rate_str, sid, tones, margin, dither, long, dc_blocker):
    base = rate_str.split("_")[0]
    n = N_SAMPLES * (RATE_MULT[base] if long else 1)
    stem = f"sig_{rate_str}_ord{order}_{sid}_{dither}_m{margin:g}"
    meta, bin_out = run_harness(
        harness_bin,
        stem,
        ["--mode", "chain", "--rate", rate_str, "--order", str(order), "--samples", str(n),
         "--tones", _tones_arg(tones), "--margin-db", f"{margin:g}", "--dither", dither,
         "--dc-blocker", "on" if dc_blocker else "off"],
        with_bin=True,
    )
    bits = np.unpackbits(np.fromfile(bin_out, dtype=np.uint8))[:n]
    bin_out.unlink()
    m = analyze_components(np.where(bits == 1, 1.0, -1.0), meta["f_bit_hz"],
                           [t["k_bin"] for t in meta["tones"] if t["f_hz"] > 0], ref_db=meta["gain_db"])
    expected = [t["level_dbfs"] for t in meta["tones"] if t["f_hz"] > 0]
    tone_hz = [t["f_hz"] for t in meta["tones"] if t["f_hz"] > 0]
    flat_tol = CHAIN_FLATNESS_DB[quality_tier(rate_str)]
    tols = [_level_tolerance_db(x, LEVEL_TOL_DB if f <= LEVEL_CHECK_MAX_HZ else flat_tol)
            for f, x in zip(tone_hz, m["tone_lobe_snr_db"])]
    devs = [a - b for a, b in zip(m["tone_levels_db"], expected)]
    return {
        "order": order, "rate": rate_str, "id": sid, "dither": dither, "margin": margin, "samples": n,
        "dc_blocker": meta["dc_blocker"],
        "gain_db": meta["gain_db"], "limit_dbfs": meta["limit_dbfs"],
        "overloads": meta["overload_count"] + meta["warmup_overloads"],
        "peak_state": max(meta["max_state"]),
        "level_error_db": max((abs(d) for d in devs), default=0.0),
        "level_dev_db": devs,
        "level_tol_db": tols,
        "tone_hz": tone_hz,
        **m,
    }


def run(harness_bin, oracle_results):
    print("\n=== Signal-dependent measurements (production chain, harness mode `chain`) ===")
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    jobs = [(order, rate_str, *s) for order in ORDERS for rate_str, _, _ in DIRECT_RATES for s in _stimuli()]
    workers = max(1, (os.cpu_count() or 2) // 2)
    with ThreadPoolExecutor(workers) as ex:
        rows = list(ex.map(lambda j: _run_one(harness_bin, *j), jobs))
    res = {(r["order"], r["rate"], r["id"], r["dither"], r["margin"]): r for r in rows}

    # Analysis floor (Session 1 rule): the absolute in-band level the method resolves for each NTF, from the
    # synthetic record at -6 dB modulator level and N = 2^20 (longer records resolve at least as far).
    floors = {}
    for order in ORDERS:
        for rate_str, f_bit, _ in DIRECT_RATES:
            k = round(1000.0 * N_SAMPLES / f_bit)
            snr = analysis_floor_snr_db(oracle_results[order][rate_str]["synth"], f_bit, N_SAMPLES, k, -6.0)
            gain = res[(order, rate_str, "silence", "tpdf", PRODUCTION_MARGIN_DB)]["gain_db"]
            floors[(order, rate_str)] = 20 * np.log10(10 ** (-6.0 / 20)) - snr - gain
    prom_med, prom_max = spur_prominence_noise_db(N_SAMPLES)
    for order in ORDERS:
        for rate_str, _, _ in DIRECT_RATES:
            r0 = res[(order, rate_str, "1k_0", "tpdf", PRODUCTION_MARGIN_DB)]
            r60 = res[(order, rate_str, "1k_-60", "tpdf", PRODUCTION_MARGIN_DB)]
            print(f"  [Order {order}] {rate_str.upper():10s}: SNR @0 dBFS {r0['snr_db']:6.1f} dB, "
                  f"THD {r0['thd_db']:7.1f} dB, DR(A) {-r60['thdn_a_db']:6.1f} dB, "
                  f"ultrasonic {r0['ultrasonic_db']['20-100k']:6.1f} dB")
    return {"rows": res, "floors": floors, "prominence_noise": (prom_med, prom_max)}


# --- rendering -------------------------------------------------------------------------------------------------


def _fmt_level(x, floor):
    """An absolute in-band level, or an upper bound when within FLOOR_MARGIN_DB of the analysis floor."""
    if x < floor + FLOOR_MARGIN_DB:
        return f"<= {floor + FLOOR_MARGIN_DB:.0f}"
    return f"{x:.1f}"


def _fmt_ratio(signal_db, noise_db, floor):
    """signal - noise in dB, or a lower bound when the noise is at the analysis floor."""
    if noise_db < floor + FLOOR_MARGIN_DB:
        return f">= {signal_db - floor - FLOOR_MARGIN_DB:.0f}"
    return f"{signal_db - noise_db:.1f}"


def _fmt_opt(x):
    return "—" if x is None else f"{x:.1f}"


def _family(rate_str):
    return "48k" if rate_str.endswith("_48") else "44.1k"


def render(sig):
    rows, floors = sig["rows"], sig["floors"]
    prom_med, prom_max = sig["prominence_noise"]
    failures = []
    M = PRODUCTION_MARGIN_DB

    def get(order, rate, sid, dither="tpdf", margin=M):
        return rows[(order, rate, sid, dither, margin)]

    # Invariants: no overload at any production level (including 0 dBFS at M = 0), level staging exact.
    for r in rows.values():
        if r["overloads"]:
            failures.append(f"Signal: overload at order {r['order']} {r['rate']} {r['id']} ({r['dither']}, M={r['margin']:g})")
        for f, d, tol in zip(r["tone_hz"], r["level_dev_db"], r["level_tol_db"]):
            if abs(d) > tol:
                failures.append(f"Signal: level error {d:+.4f} dB at {f / 1e3:.2f} kHz (tolerance {tol:.4f}) at order {r['order']} {r['rate']} {r['id']}")

    text = f"""
---

## Signal-dependent measurements

Production chain with the default settings, harness mode `chain`: baseband PCM stimulus -> profile gain `L - M`
(`M` = {M:g} dB, `[gain] dsd_safety_margin_db` default) -> `DcBlocker` (first-order high-pass, 1 Hz; settled for 30 time
constants before the record) -> `HalfBandCascade` (linear phase, apodization `off`) -> `Modulator`
(TPDF dither 0.005, seed 12345). Every record is checked bit for bit against the per-sample modulator path (`PeakTracker`).

**Levels.** dBFS is relative to the PCM input's full scale: a 0 dBFS PCM sine reaches the modulator at `L - M`
(-4.25 to -5.0 dB re the DSD bitstream's full scale; add the "Gain" column to convert). Noise is in-band (20 Hz-20 kHz),
excluding the tone, harmonic and intermodulation lobes. "DR (A)" follows AES17: full scale minus the A-weighted THD+N
of a -60 dBFS 1 kHz tone. Levels within {FLOOR_MARGIN_DB:g} dB of the analysis floor (Session 1 method) print as upper bounds,
ratios against them as lower bounds.

**Spur prominence.** The 9-bin group (one tone lobe) of noise bins with the highest power over its local noise floor (running
median of {SPUR_LOCAL_BINS} noise bins x 9). White Gaussian noise of the same length gives {prom_med:.1f} dB (median of 8 seeds, max
{prom_max:.1f} dB), so values near that mean "no tone above the noise"; an idle tone or residual image stands out above it.

Records: N = 2^20 bits (2^20 x 2/4/8 at DSD128/256/512 for the two-tone tests, so the bin width stays ~2.7-2.9 Hz).
Invariant checks: no overload at any stimulus (including 0 dBFS at `M = 0`, i.e. a sine peak exactly at `L`), and every
measured tone level (tones up to {LEVEL_CHECK_MAX_HZ / 1e3:g} kHz) within {LEVEL_TOL_DB} dB of the requested PCM level (plus 3 sigma of the
noise inside the tone lobe at the lowest levels; worst error over the checked records:
{max(abs(d) for r in rows.values() for f, d in zip(r['tone_hz'], r['level_dev_db']) if f <= LEVEL_CHECK_MAX_HZ):.4f} dB).
Tones above {LEVEL_CHECK_MAX_HZ / 1e3:g} kHz (CCIF 19/20 kHz) are held to the chain flatness requirement instead (§8.6).
"""

    def production(order, rate):
        """The production records (dither on, DC blocker on) of one rate and order."""
        return [r for r in rows.values() if r["order"] == order and r["rate"] == rate and r["dither"] == "tpdf" and r["dc_blocker"]]

    def bounded(x, fl):
        """A level as an upper bound: values within FLOOR_MARGIN_DB of the analysis floor count as the bound."""
        return max(x, fl + FLOOR_MARGIN_DB)

    text += f"""
### Quality requirements (`Design.md` §8.6)

Two anchors. **Audibility:** with 0 dBFS at ~120 dB SPL peak, noise and every discrete product stay below the threshold of
hearing: DR (A) >= {AUDIBILITY_DR_A_DB:.0f} dB and every in-band spur <= {AUDIBILITY_SPUR_DBFS:.0f} dBFS. **24-bit transparency:** the chain is not the
bottleneck for 24-bit sources (TPDF-dithered 24-bit noise is -141.5 dBFS): in-band noise, harmonic and IMD levels and every spur
<= {TRANSPARENCY_NOISE_DBFS:.0f} dBFS at every stimulus. Tier A (DSD64, both families) must meet audibility, tier B (DSD128 and above) both. Tones
above {LEVEL_CHECK_MAX_HZ / 1e3:g} kHz must be within {CHAIN_FLATNESS_DB['A']} dB (A) / {CHAIN_FLATNESS_DB['B']} dB (B) of the requested level. Spurs are gated in absolute
dBFS (a spur's audibility does not depend on the test tone, and silence and DC have no tone to refer to); THD and IMD are
ratios and stay in dBc in the tables below. Worst values are over every production record (dither on, DC blocker on: the
level sweep, silence, DC, SMPTE, CCIF); levels within {FLOOR_MARGIN_DB:g} dB of the analysis floor count as that bound.

| Order | Rate | Tier | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Max level error > {LEVEL_CHECK_MAX_HZ / 1e3:g} kHz (dB) | Verdict |
|---:|---|:---:|---:|---:|---:|---:|---:|---|
"""
    for order in ORDERS:
        for rate, _, _ in DIRECT_RATES:
            tier, fl = quality_tier(rate), floors[(order, rate)]
            recs = production(order, rate)
            dr = -get(order, rate, "1k_-60")["thdn_a_db"]
            dr_bounded = min(dr, -(fl + FLOOR_MARGIN_DB))
            spur = max(r["spur_db"] for r in recs)
            noise = max(bounded(r["noise_db"], fl) for r in recs)
            dist = max(bounded(x, fl) for r in recs for x in (r.get("harm_db"), r.get("imd_level_db")) if x is not None)
            hf = [abs(d) for r in recs for f, d in zip(r["tone_hz"], r["level_dev_db"]) if f > LEVEL_CHECK_MAX_HZ]
            misses = []
            if dr_bounded < AUDIBILITY_DR_A_DB:
                misses.append(f"DR (A) {dr_bounded:.1f} dB < {AUDIBILITY_DR_A_DB:.0f}")
            spur_limit = AUDIBILITY_SPUR_DBFS if tier == "A" else TRANSPARENCY_SPUR_DBFS
            if spur > spur_limit:
                misses.append(f"spur {spur:.1f} dBFS > {spur_limit:.0f}")
            if tier == "B":
                if noise > TRANSPARENCY_NOISE_DBFS:
                    misses.append(f"noise {noise:.1f} dBFS > {TRANSPARENCY_NOISE_DBFS:.0f}")
                if dist > TRANSPARENCY_NOISE_DBFS:
                    misses.append(f"distortion {dist:.1f} dBFS > {TRANSPARENCY_NOISE_DBFS:.0f}")
            exception = TIER_EXCEPTIONS.get((order, rate))
            if not misses:
                verdict = "**PASS**"
            elif exception and all(x.startswith("DR (A)") for x in misses):
                verdict = f"**EXCEPTION** ({misses[0]}; documented)"
            else:
                verdict = "**FAIL** (" + "; ".join(misses) + ")"
                failures.append(f"Signal: quality requirement at order {order} {rate}: " + "; ".join(misses))
            if exception and not (misses and verdict.startswith("**EXCEPTION")):
                failures.append(f"Signal: exception {(order, rate)} no longer needed or does not apply; update TIER_EXCEPTIONS")
            text += (
                f"| {order} | `{rate.upper()}` | {tier} | {_fmt_ratio(0.0, -dr, fl)} | {spur:.1f} | {_fmt_level(noise, fl)} | "
                f"{_fmt_level(dist, fl)} | {max(hf):.4f} | {verdict} |\n"
            )

    text += """
### Summary per rate and order (dither on)

| Order | Rate | Family | L (dBFS) | Gain L - M (dB) | Noise, silence (dBFS) | Noise A-wtd, silence (dBFS) | SNR @ 0 dBFS (dB) | THD @ 0 dBFS (dBc) | THD @ 0 dBFS, M = 0 (dBc) | DR (A) (dB) | Ultrasonic 20-100 kHz, silence (dBFS) | Ultrasonic 20-100 kHz @ 0 dBFS (dBFS) |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
"""
    for order in ORDERS:
        for rate, _, fam in DIRECT_RATES:
            fl = floors[(order, rate)]
            s, r0, rl, r60 = get(order, rate, "silence"), get(order, rate, "1k_0"), get(order, rate, "1k_L", margin=0.0), get(order, rate, "1k_-60")
            text += (
                f"| {order} | `{rate.upper()}` | {fam} | {r0['limit_dbfs']:.2f} | {r0['gain_db']:.2f} | "
                f"{_fmt_level(s['noise_db'], fl)} | {_fmt_level(s['noise_a_db'], fl)} | "
                f"{_fmt_ratio(r0['tone_levels_db'][0], r0['noise_db'], fl)} | {r0['thd_db']:.1f} | {rl['thd_db']:.1f} | "
                f"{_fmt_ratio(0.0, r60['thdn_a_db'], fl)} | {s['ultrasonic_db']['20-100k']:.1f} | {r0['ultrasonic_db']['20-100k']:.1f} |\n"
            )

    for order in ORDERS:
        text += f"""
### Level sweep, {order}th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | """ + " | ".join(f"`{r.upper()}`" for r, _, _ in DIRECT_RATES) + " |\n|---:|" + "---:|" * len(DIRECT_RATES) + "\n"
        for sid, label in [("1k_L", "0 (M = 0)")] + [(f"1k_{db:g}", f"{db:g}") for db in SWEEP_DBFS]:
            cells = []
            for rate, _, _ in DIRECT_RATES:
                r = get(order, rate, sid, margin=0.0 if sid == "1k_L" else M)
                fl = floors[(order, rate)]
                nd = 10 * np.log10(10 ** (r["noise_db"] / 10) + 10 ** ((r["harm_db"] or -400) / 10))
                cells.append(_fmt_ratio(r["tone_levels_db"][0], nd, fl))
            text += f"| {label} | " + " | ".join(cells) + " |\n"

        text += f"""
Worst spur per level ({order}th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | """ + " | ".join(f"`{r.upper()}`" for r, _, _ in DIRECT_RATES) + " |\n|---:|" + "---:|" * len(DIRECT_RATES) + "\n"
        sids = [("1k_L", "0 (M = 0)", 0.0)] + [(f"1k_{db:g}", f"{db:g}", M) for db in SWEEP_DBFS]
        sids += [("silence", "silence", M)] + [(f"dc_{db:g}", f"DC {db:g}", M) for db in DC_DBFS]
        for sid, label, margin in sids:
            cells = []
            for rate, _, _ in DIRECT_RATES:
                r = get(order, rate, sid, margin=margin)
                cells.append(f"{r['spur_prominence_db']:.1f} / {r['spur_db']:.1f}")
            text += f"| {label} | " + " | ".join(cells) + " |\n"

    text += """
### Dither (TPDF 0.005) on vs off

Low-level stimuli with and without the production dither. "Noise delta" is the in-band noise with dither minus without at
-6 dBFS (the dither is added at the quantizer, so the loop shapes it like quantization noise). The spur columns give the
highest prominence over the stimuli -120/-100/-60 dBFS 1 kHz, silence and DC -40/-80 dBFS, with the stimulus and frequency.

| Order | Rate | Noise delta @ -6 dBFS (dB) | Worst spur, no dither | Worst spur, TPDF |
|---:|---|---:|---|---|
"""
    low = [f"1k_{db:g}" for db in NO_DITHER_DBFS if db < -6] + ["silence"] + [f"dc_{db:g}" for db in DC_DBFS]
    for order in ORDERS:
        for rate, _, _ in DIRECT_RATES:
            fl = floors[(order, rate)]
            on, off = get(order, rate, "1k_-6"), get(order, rate, "1k_-6", "none")
            delta = (on["noise_db"] - off["noise_db"]) if min(on["noise_db"], off["noise_db"]) >= fl + FLOOR_MARGIN_DB else None

            def worst(dither):
                r = max((get(order, rate, s, dither) for s in low), key=lambda x: x["spur_prominence_db"])
                return f"{r['spur_prominence_db']:.1f} dB ({r['id'].replace('1k_', '1 kHz ').replace('dc_', 'DC ')}, {r['spur_hz'] / 1e3:.2f} kHz, {r['spur_db']:.1f} dBFS)"

            text += f"| {order} | `{rate.upper()}` | {'at floor' if delta is None else f'{delta:+.2f}'} | {worst('none')} | {worst('tpdf')} |\n"

    text += f"""
### DC offsets: DC blocker off vs on (dither on)

A DC offset `u` at the modulator input gives an idle tone near `u * f_bit` that the dither does not remove (F9). The
production chain removes source DC with a first-order 1 Hz high-pass ahead of the cascade (`Design.md` §4.1). "Off" is the
chain without it (before O7). Residual DC is the record's windowed mean (dBFS). Prominence near {prom_med:.1f} dB (white-noise
median, max {prom_max:.1f} dB) means no tone above the noise.

| Order | Rate | DC (dBFS) | Off: worst spur (prominence, kHz, dBFS) | Off: residual DC (dBFS) | On: worst spur (prominence, kHz, dBFS) | On: residual DC (dBFS) |
|---:|---|---:|---|---:|---|---:|
"""

    def spur(r):
        return f"{r['spur_prominence_db']:.1f} dB, {r['spur_hz'] / 1e3:.2f} kHz, {r['spur_db']:.1f}"

    for order in ORDERS:
        for rate, _, _ in DIRECT_RATES:
            for db in DC_DBFS:
                off, on = get(order, rate, f"dc_{db:g}_unblocked"), get(order, rate, f"dc_{db:g}")
                text += (
                    f"| {order} | `{rate.upper()}` | {db:g} | {spur(off)} | {off['dc_db']:.1f} | "
                    f"{spur(on)} | {on['dc_db']:.1f} |\n"
                )
                # Invariant: the blocker removes the DC (residual at least DC_REMOVED_DB below the input offset).
                if on["dc_db"] > db - DC_REMOVED_DB:
                    failures.append(f"Signal: DC blocker left {on['dc_db']:.1f} dBFS of a {db:g} dBFS offset at order {order} {rate}")

    text += """
### Two-tone intermodulation (0 dBFS peak, dither on)

IMD = power of the products `m*f1 + n*f2` (both non-zero, `|m| + |n| <= 7`) inside 20 Hz-20 kHz relative to the total tone
power; THD counts the harmonics of either tone. CCIF products: 1 kHz (d2), 18 kHz (d3); 21 kHz and above are out of band.

Tone level error is measured minus requested level of each tone (the cascade is flat to 1e-6 dB, so at 19/20 kHz this
is the modulator's signal transfer function).

| Order | Rate | Stimulus | IMD (dBc) | IMD level (dBFS) | THD (dBc) | Noise (dBFS) | Worst spur (prominence / dBFS) | Tone level error f1 / f2 (dB) | Peak abs state |
|---:|---|---|---:|---:|---:|---:|---|---:|---:|
"""
    for order in ORDERS:
        for rate, _, _ in DIRECT_RATES:
            fl = floors[(order, rate)]
            for sid, (label, _) in IMD_STIMULI.items():
                r = get(order, rate, sid)
                text += (
                    f"| {order} | `{rate.upper()}` | {label} | {r['imd_db']:.1f} | {r['imd_level_db']:.1f} | "
                    f"{_fmt_opt(r['thd_db'])} | {_fmt_level(r['noise_db'], fl)} | {r['spur_prominence_db']:.1f} / {r['spur_db']:.1f} | "
                    f"{r['level_dev_db'][0]:+.4f} / {r['level_dev_db'][1]:+.4f} | {r['peak_state']:.3f} |\n"
                )

    text += """
### Ultrasonic noise (reported, not gated)

Out-of-band power per band, dBFS (PCM full scale). Neither the K17 nor the JA11 documents an ultrasonic input limit
(`docs/hardware/`), so this is information for the DAC and amplifier chain, not a check. "dBc @ -6" is relative to a
-6 dBFS tone, for comparison with earlier reports.

| Order | Rate | Silence 20-50 kHz | Silence 50-100 kHz | Silence 20-100 kHz | 0 dBFS 20-50 kHz | 0 dBFS 50-100 kHz | 0 dBFS 20-100 kHz | 20-100 kHz, dBc @ -6 dBFS |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
"""
    for order in ORDERS:
        for rate, _, _ in DIRECT_RATES:
            s, r0, r6 = get(order, rate, "silence"), get(order, rate, "1k_0"), get(order, rate, "1k_-6")
            u = lambda r, b: f"{r['ultrasonic_db'][b]:.1f}"  # noqa: E731
            text += (
                f"| {order} | `{rate.upper()}` | {u(s, '20-50k')} | {u(s, '50-100k')} | {u(s, '20-100k')} | "
                f"{u(r0, '20-50k')} | {u(r0, '50-100k')} | {u(r0, '20-100k')} | "
                f"{r6['ultrasonic_db']['20-100k'] - r6['tone_levels_db'][0]:.1f} |\n"
            )

    text += """
### Rate family: 44.1 kHz vs 48 kHz

Same DSD multiple, 48 kHz family minus 44.1 kHz family (the 48k family runs at an 8.8% higher bit rate, so a higher OSR).
Positive SNR/DR deltas favour 48k; "at floor" means one side is within 10 dB of the analysis floor.

| Order | DSD multiple | SNR @ 0 dBFS delta (dB) | DR (A) delta (dB) | THD @ 0 dBFS delta (dB) | Ultrasonic 20-100 kHz @ 0 dBFS delta (dB) |
|---:|---|---:|---:|---:|---:|
"""
    for order in ORDERS:
        for base in ["dsd64", "dsd128", "dsd256", "dsd512"]:
            a0, b0 = get(order, base, "1k_0"), get(order, f"{base}_48", "1k_0")
            a6, b6 = get(order, base, "1k_-60"), get(order, f"{base}_48", "1k_-60")
            fa, fb = floors[(order, base)], floors[(order, f"{base}_48")]
            ok = min(a0["noise_db"] - fa, b0["noise_db"] - fb) >= FLOOR_MARGIN_DB
            ok60 = min(a6["thdn_a_db"] - fa, b6["thdn_a_db"] - fb) >= FLOOR_MARGIN_DB
            snr = f"{(b0['tone_levels_db'][0] - b0['noise_db']) - (a0['tone_levels_db'][0] - a0['noise_db']):+.1f}" if ok else "at floor"
            dr = f"{a6['thdn_a_db'] - b6['thdn_a_db']:+.1f}" if ok60 else "at floor"
            text += (
                f"| {order} | {base.upper()} | {snr} | {dr} | {b0['thd_db'] - a0['thd_db']:+.1f} | "
                f"{b0['ultrasonic_db']['20-100k'] - a0['ultrasonic_db']['20-100k']:+.1f} |\n"
            )
    return Section(text, failures)
