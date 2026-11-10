# audigma — Signal Measurements

> **Generated:** `2026-11-10 02:27:42Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [report.md](report.md)

Production DSD chain (gain L - M, DC blocker, cascade, dithered modulator): quality tiers, level sweep, dither, DC, IMD, ultrasonic noise, rate families.

---

## Signal-dependent measurements

Production chain with the default settings, harness mode `chain`: baseband PCM stimulus -> profile gain `L - M`
(`M` = 0.25 dB, `[gain] dsd_safety_margin_db` default) -> `DcBlocker` (first-order high-pass, 1 Hz; settled for 30 time
constants before the record) -> `HalfBandCascade` (linear phase, apodization `off`) -> `Modulator`
(TPDF dither 0.005, seed 12345). Every record is checked bit for bit against the per-sample modulator path (`PeakTracker`).

**Levels.** dBFS is relative to the PCM input's full scale: a 0 dBFS PCM sine reaches the modulator at `L - M`
(-4.25 to -5.0 dB re the DSD bitstream's full scale; add the "Gain" column to convert). Noise is in-band (20 Hz-20 kHz),
excluding the tone, harmonic and intermodulation lobes. "DR (A)" follows AES17: full scale minus the A-weighted THD+N
of a -60 dBFS 1 kHz tone. Levels within 10 dB of the analysis floor (Session 1 method) print as upper bounds,
ratios against them as lower bounds.

**Spur prominence.** The 9-bin group (one tone lobe) of noise bins with the highest power over its local noise floor (running
median of 81 noise bins x 9). White Gaussian noise of the same length gives 8.3 dB (median of 8 seeds, max
9.1 dB), so values near that mean "no tone above the noise"; an idle tone or residual image stands out above it.

Records: N = 2^20 bits (2^20 x 2/4/8 at DSD128/256/512 for the two-tone tests, so the bin width stays ~2.7-2.9 Hz).
Invariant checks: no overload at any stimulus (including 0 dBFS at `M = 0`, i.e. a sine peak exactly at `L`), and every
measured tone level (tones up to 10 kHz) within 0.01 dB of the requested PCM level (plus 3 sigma of the
noise inside the tone lobe at the lowest levels; worst error over the checked records:
0.0250 dB).
Tones above 10 kHz (CCIF 19/20 kHz) are held to the chain flatness requirement instead (§8.6).

### Quality requirements (`Design.md` §8.6)

Two anchors. **Audibility:** with 0 dBFS at ~120 dB SPL peak, noise and every discrete product stay below the threshold of
hearing: DR (A) >= 120 dB and every in-band spur <= -120 dBFS. **24-bit transparency:** the chain is not the
bottleneck for 24-bit sources (TPDF-dithered 24-bit noise is -141.5 dBFS): in-band noise, harmonic and IMD levels and every spur
<= -141 dBFS at every stimulus. Tier A (DSD64, both families) must meet audibility, tier B (DSD128 and above) both. Tones
above 10 kHz must be within 0.02 dB (A) / 0.01 dB (B) of the requested level. Spurs are gated in absolute
dBFS (a spur's audibility does not depend on the test tone, and silence and DC have no tone to refer to); THD and IMD are
ratios and stay in dBc in the tables below. Worst values are over every production record (dither on, DC blocker on: the
level sweep, silence, DC, SMPTE, CCIF); levels within 10 dB of the analysis floor count as that bound.

| Order | Rate | Tier | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Max level error > 10 kHz (dB) | Verdict |
|---:|---|:---:|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | A | 118.4 | -130.2 | -116.3 | -117.2 | 0.0110 | **EXCEPTION** (DR (A) 118.4 dB < 120; documented) |
| 5 | `DSD128` | B | 147.5 | -163.6 | -149.7 | -149.3 | 0.0027 | **PASS** |
| 5 | `DSD256` | B | 175.6 | -192.8 | -182.4 | -176.1 | 0.0007 | **PASS** |
| 5 | `DSD512` | B | 210.5 | -227.0 | -215.8 | -208.5 | 0.0002 | **PASS** |
| 5 | `DSD64_48` | A | 122.4 | -134.7 | -120.2 | -120.2 | 0.0092 | **PASS** |
| 5 | `DSD128_48` | B | 150.7 | -171.2 | -153.6 | -152.2 | 0.0023 | **PASS** |
| 5 | `DSD256_48` | B | 179.9 | -197.5 | -186.9 | -179.9 | 0.0006 | **PASS** |
| 5 | `DSD512_48` | B | 214.0 | -225.9 | -219.6 | -211.4 | 0.0001 | **PASS** |
| 7 | `DSD64` | A | 137.3 | -156.6 | -134.7 | -139.0 | 0.0150 | **PASS** |
| 7 | `DSD128` | B | 179.9 | -193.3 | -179.6 | -182.7 | 0.0038 | **PASS** |
| 7 | `DSD256` | B | 222.1 | -241.9 | -224.9 | -222.8 | 0.0009 | **PASS** |
| 7 | `DSD512` | B | 268.0 | -262.6 | -243.8 | -265.4 | 0.0002 | **PASS** |
| 7 | `DSD64_48` | A | 142.5 | -161.5 | -140.2 | -144.1 | 0.0127 | **PASS** |
| 7 | `DSD128_48` | B | 184.0 | -203.1 | -185.2 | -186.1 | 0.0032 | **PASS** |
| 7 | `DSD256_48` | B | 225.8 | -239.1 | -230.2 | -226.4 | 0.0008 | **PASS** |
| 7 | `DSD512_48` | B | 273.1 | -262.8 | -243.4 | -267.9 | 0.0002 | **PASS** |

### Summary per rate and order (dither on)

| Order | Rate | Family | L (dBFS) | Gain L - M (dB) | Noise, silence (dBFS) | Noise A-wtd, silence (dBFS) | SNR @ 0 dBFS (dB) | THD @ 0 dBFS (dBc) | THD @ 0 dBFS, M = 0 (dBc) | DR (A) (dB) | Ultrasonic 20-100 kHz, silence (dBFS) | Ultrasonic 20-100 kHz @ 0 dBFS (dBFS) |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | 44.1k | -4.00 | -4.25 | <= -124 | <= -124 | 117.1 | -123.0 | -122.9 | 118.4 | -46.0 | -26.2 |
| 5 | `DSD128` | 44.1k | -4.00 | -4.25 | -156.9 | -159.9 | 150.1 | -153.4 | -152.7 | 147.5 | -61.0 | -55.2 |
| 5 | `DSD256` | 44.1k | -4.25 | -4.50 | <= -192 | <= -192 | 183.2 | -183.5 | -183.4 | 175.6 | -101.0 | -88.6 |
| 5 | `DSD512` | 44.1k | -4.00 | -4.25 | <= -227 | <= -227 | 217.0 | -213.2 | -212.7 | 210.5 | -134.1 | -121.6 |
| 5 | `DSD64_48` | 48k | -4.25 | -4.50 | <= -145 | <= -145 | 120.9 | -127.0 | -126.8 | 122.4 | -41.5 | -28.3 |
| 5 | `DSD128_48` | 48k | -4.00 | -4.25 | <= -162 | <= -162 | 154.0 | -157.0 | -156.6 | 150.7 | -71.7 | -59.3 |
| 5 | `DSD256_48` | 48k | -4.00 | -4.25 | -197.2 | -198.9 | 187.4 | -186.5 | -186.2 | 179.9 | -104.5 | -92.7 |
| 5 | `DSD512_48` | 48k | -4.25 | -4.50 | -229.6 | -230.5 | 221.0 | -217.3 | -216.8 | 214.0 | -137.7 | -125.3 |
| 7 | `DSD64` | 44.1k | -4.50 | -4.75 | <= -141 | <= -141 | 135.4 | -141.4 | -141.7 | 137.3 | -34.7 | -21.1 |
| 7 | `DSD128` | 44.1k | -4.75 | -5.00 | -188.8 | <= -192 | 180.4 | -184.0 | -183.6 | 179.9 | -53.7 | -46.8 |
| 7 | `DSD256` | 44.1k | -4.75 | -5.00 | <= -233 | <= -233 | 225.7 | -225.9 | -225.8 | 222.1 | -100.8 | -92.4 |
| 7 | `DSD512` | 44.1k | -4.75 | -5.00 | <= -272 | <= -272 | 271.9 | -267.9 | -267.7 | 268.0 | -145.8 | -137.5 |
| 7 | `DSD64_48` | 48k | -4.50 | -4.75 | -148.7 | -151.7 | 140.9 | -146.6 | -146.0 | 142.5 | -35.9 | -21.8 |
| 7 | `DSD128_48` | 48k | -4.75 | -5.00 | <= -192 | <= -192 | 185.6 | -189.3 | -188.4 | 184.0 | -59.9 | -52.8 |
| 7 | `DSD256_48` | 48k | -4.75 | -5.00 | -239.2 | -242.2 | 231.5 | -231.0 | -230.5 | 225.8 | -106.8 | -98.1 |
| 7 | `DSD512_48` | 48k | -4.75 | -5.00 | <= -273 | <= -273 | >= 273 | -273.6 | -272.9 | 273.1 | -151.2 | -142.4 |

### Level sweep, 5th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 116.0 | 148.5 | 180.1 | 211.5 | 119.9 | 152.4 | 183.8 | 215.4 |
| 0 | 116.1 | 148.4 | 180.4 | 211.7 | 119.9 | 152.3 | 183.9 | 215.8 |
| -1 | 115.1 | 147.8 | 179.9 | 211.9 | 118.7 | 152.1 | 183.9 | 215.7 |
| -3 | 113.4 | 146.1 | 178.2 | 210.4 | 117.1 | 149.9 | 182.8 | 214.3 |
| -6 | 110.4 | 143.1 | 175.6 | 208.8 | 114.3 | 146.7 | 179.9 | 212.0 |
| -10 | 106.2 | 139.2 | 171.4 | 204.9 | 109.8 | 143.4 | 175.9 | 208.0 |
| -20 | 97.6 | 130.4 | 163.5 | 195.7 | 100.9 | 134.3 | 167.4 | 199.5 |
| -40 | 74.7 | 108.7 | 141.6 | 175.2 | 78.2 | 112.3 | 145.6 | 178.0 |
| -60 | 56.6 | 87.8 | 115.7 | 148.2 | 60.7 | 91.1 | 119.5 | 151.1 |
| -80 | 44.0 | 76.7 | 109.7 | 133.1 | 48.2 | 81.1 | 113.6 | 136.3 |
| -100 | >= 24 | 57.1 | 90.5 | 123.4 | 28.4 | 61.5 | 94.5 | 127.8 |
| -120 | >= 4 | 37.4 | 69.8 | 104.0 | 8.7 | 41.2 | 74.3 | 106.6 |

Worst spur per level (5th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 9.9 / -130.2 | 8.8 / -170.3 | 13.0 / -195.0 | 9.6 / -232.8 | 7.9 / -143.7 | 9.0 / -200.1 | 8.4 / -212.1 | 7.8 / -252.8 |
| 0 | 9.0 / -139.2 | 9.4 / -172.8 | 9.3 / -196.2 | 13.1 / -227.0 | 7.9 / -144.9 | 8.0 / -178.7 | 14.7 / -197.5 | 7.5 / -232.0 |
| -1 | 8.3 / -170.1 | 9.0 / -210.9 | 9.2 / -218.1 | 6.7 / -244.5 | 8.9 / -139.9 | 7.9 / -182.4 | 7.8 / -204.0 | 6.3 / -234.5 |
| -3 | 7.7 / -140.7 | 9.1 / -184.2 | 7.8 / -192.8 | 9.2 / -255.8 | 8.2 / -145.8 | 7.4 / -180.5 | 7.4 / -210.5 | 9.7 / -255.6 |
| -6 | 7.9 / -141.9 | 10.0 / -163.6 | 14.1 / -239.2 | 7.1 / -230.6 | 8.3 / -140.1 | 7.7 / -178.8 | 7.8 / -220.6 | 8.7 / -231.0 |
| -10 | 13.0 / -159.5 | 8.1 / -199.4 | 8.0 / -217.3 | 7.4 / -232.6 | 15.3 / -134.7 | 7.9 / -172.7 | 7.4 / -219.8 | 11.9 / -225.9 |
| -20 | 8.8 / -167.6 | 9.1 / -208.1 | 9.5 / -209.9 | 10.9 / -261.1 | 10.8 / -141.0 | 7.9 / -171.2 | 7.6 / -212.9 | 6.7 / -231.7 |
| -40 | 11.0 / -139.8 | 13.9 / -166.2 | 7.2 / -221.3 | 9.5 / -254.0 | 8.9 / -154.0 | 8.3 / -203.2 | 7.8 / -213.0 | 7.7 / -259.8 |
| -60 | 12.3 / -135.5 | 7.3 / -191.6 | 9.6 / -236.7 | 6.8 / -241.2 | 11.7 / -154.9 | 8.5 / -177.2 | 8.1 / -205.1 | 9.3 / -247.4 |
| -80 | 8.0 / -156.2 | 8.7 / -194.4 | 8.3 / -206.1 | 14.7 / -231.2 | 10.0 / -150.9 | 6.9 / -218.9 | 8.9 / -207.1 | 7.6 / -243.6 |
| -100 | 11.8 / -150.4 | 11.8 / -167.9 | 9.6 / -238.1 | 7.5 / -240.1 | 11.7 / -147.4 | 8.8 / -186.7 | 7.1 / -244.8 | 7.2 / -242.3 |
| -120 | 16.4 / -152.1 | 11.8 / -182.9 | 8.8 / -211.9 | 11.1 / -271.2 | 13.3 / -155.1 | 13.9 / -197.2 | 9.4 / -217.3 | 10.1 / -250.5 |
| silence | 3.1 / -325.3 | 7.5 / -177.9 | 8.3 / -227.8 | 11.5 / -239.4 | 2.5 / -330.6 | 7.6 / -198.4 | 8.7 / -214.2 | 15.4 / -240.8 |
| DC -40 | 3.1 / -325.3 | 9.0 / -209.2 | 11.9 / -240.4 | 9.7 / -236.8 | 2.5 / -330.6 | 8.0 / -198.5 | 17.2 / -344.1 | 8.1 / -271.6 |
| DC -80 | 3.1 / -325.3 | 7.5 / -177.9 | 7.9 / -237.4 | 9.3 / -244.0 | 2.5 / -330.6 | 7.6 / -198.4 | 11.1 / -229.8 | 13.1 / -256.4 |

### Level sweep, 7th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 134.7 | 178.4 | 222.8 | 266.2 | 139.9 | 183.9 | 227.9 | 271.3 |
| 0 | 134.4 | 178.8 | 222.8 | 266.4 | 139.8 | 184.0 | 228.2 | 271.9 |
| -1 | 133.5 | 178.0 | 222.3 | 265.8 | 139.1 | 183.6 | 227.6 | 271.1 |
| -3 | 131.7 | 175.9 | 220.7 | 264.9 | 137.0 | 181.5 | 225.8 | 269.6 |
| -6 | 128.6 | 173.4 | 218.1 | 262.7 | 134.1 | 178.8 | 223.2 | >= 267 |
| -10 | 124.5 | 169.1 | 214.1 | 258.6 | 129.9 | 174.7 | 219.5 | >= 263 |
| -20 | 115.7 | 160.2 | 205.1 | 250.5 | 121.2 | 165.3 | 211.2 | >= 253 |
| -40 | 94.7 | 139.9 | 184.1 | 228.0 | 99.7 | 145.5 | 190.4 | >= 233 |
| -60 | 75.6 | 119.3 | 161.3 | 205.4 | 81.0 | 123.8 | 165.5 | 210.0 |
| -80 | >= 61 | 106.6 | 150.8 | 187.0 | 68.2 | >= 112 | 153.9 | 192.2 |
| -100 | >= 41 | 87.5 | 132.0 | >= 172 | 48.2 | >= 92 | 138.2 | >= 173 |
| -120 | >= 21 | 67.9 | >= 113 | >= 152 | 28.6 | >= 72 | 118.2 | >= 153 |

Worst spur per level (7th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 8.2 / -174.2 | 9.6 / -219.1 | 9.1 / -253.7 | 7.2 / -288.4 | 7.9 / -161.6 | 7.8 / -210.7 | 8.1 / -247.3 | 10.9 / -284.7 |
| 0 | 7.7 / -185.9 | 7.2 / -234.9 | 9.1 / -256.2 | 8.3 / -295.0 | 9.7 / -170.7 | 7.0 / -204.4 | 8.2 / -255.7 | 9.6 / -287.7 |
| -1 | 9.0 / -160.4 | 8.6 / -193.3 | 7.5 / -243.1 | 6.7 / -285.0 | 8.0 / -185.9 | 10.1 / -222.2 | 9.8 / -245.3 | 7.1 / -298.8 |
| -3 | 9.7 / -163.8 | 10.6 / -224.8 | 12.6 / -272.8 | 7.4 / -285.7 | 9.8 / -189.8 | 7.9 / -222.2 | 9.9 / -239.1 | 7.2 / -289.0 |
| -6 | 9.1 / -156.6 | 8.5 / -222.4 | 8.9 / -250.4 | 7.2 / -282.7 | 7.8 / -170.0 | 8.0 / -205.1 | 10.8 / -284.1 | 9.2 / -287.3 |
| -10 | 8.2 / -193.1 | 15.4 / -215.1 | 7.7 / -241.9 | 6.9 / -285.4 | 8.1 / -183.7 | 9.2 / -240.5 | 10.5 / -241.3 | 9.1 / -295.1 |
| -20 | 7.8 / -157.6 | 9.1 / -235.5 | 6.7 / -259.4 | 8.5 / -314.2 | 9.8 / -203.9 | 10.0 / -220.1 | 7.5 / -248.4 | 6.8 / -289.9 |
| -40 | 8.2 / -160.2 | 8.2 / -204.1 | 8.2 / -263.0 | 7.9 / -298.7 | 9.1 / -170.8 | 8.9 / -207.7 | 7.3 / -262.2 | 6.6 / -291.8 |
| -60 | 8.3 / -175.5 | 7.7 / -227.5 | 7.8 / -262.4 | 8.7 / -311.5 | 10.4 / -199.3 | 12.3 / -242.3 | 8.0 / -250.3 | 7.7 / -289.4 |
| -80 | 8.0 / -183.1 | 8.3 / -216.9 | 7.9 / -256.5 | 7.0 / -301.1 | 8.7 / -166.2 | 7.5 / -235.1 | 6.3 / -254.7 | 8.8 / -297.7 |
| -100 | 8.2 / -161.2 | 7.5 / -226.2 | 8.5 / -255.5 | 6.7 / -299.9 | 10.2 / -208.2 | 12.4 / -205.0 | 7.9 / -261.9 | 10.9 / -319.0 |
| -120 | 9.1 / -180.7 | 10.2 / -223.0 | 8.7 / -253.4 | 8.0 / -290.7 | 8.2 / -175.7 | 9.3 / -205.4 | 7.5 / -256.3 | 6.8 / -306.1 |
| silence | 9.2 / -158.8 | 7.8 / -207.2 | 8.4 / -274.2 | 7.4 / -312.7 | 8.5 / -197.7 | 8.1 / -210.7 | 11.1 / -286.1 | 7.4 / -297.7 |
| DC -40 | 8.9 / -188.6 | 7.8 / -235.9 | 9.9 / -276.1 | 7.4 / -293.8 | 7.7 / -200.5 | 7.2 / -220.2 | 8.9 / -273.2 | 7.9 / -295.3 |
| DC -80 | 9.2 / -158.8 | 9.1 / -200.0 | 11.4 / -277.9 | 9.3 / -293.1 | 8.5 / -197.7 | 8.1 / -210.7 | 10.1 / -259.9 | 7.4 / -300.6 |

### Dither (TPDF 0.005) on vs off

Low-level stimuli with and without the production dither. "Noise delta" is the in-band noise with dither minus without at
-6 dBFS (the dither is added at the quantizer, so the loop shapes it like quantization noise). The spur columns give the
highest prominence over the stimuli -120/-100/-60 dBFS 1 kHz, silence and DC -40/-80 dBFS, with the stimulus and frequency.

| Order | Rate | Noise delta @ -6 dBFS (dB) | Worst spur, no dither | Worst spur, TPDF |
|---:|---|---:|---|---|
| 5 | `DSD64` | +0.07 | 15.7 dB (1 kHz -120, 0.78 kHz, -150.4 dBFS) | 16.4 dB (1 kHz -120, 0.77 kHz, -152.1 dBFS) |
| 5 | `DSD128` | +0.52 | 12.7 dB (1 kHz -120, 2.19 kHz, -177.3 dBFS) | 11.8 dB (1 kHz -100, 19.93 kHz, -167.9 dBFS) |
| 5 | `DSD256` | +0.22 | 17.2 dB (silence, 5.49 kHz, -343.8 dBFS) | 11.9 dB (DC -40, 0.09 kHz, -240.4 dBFS) |
| 5 | `DSD512` | -0.72 | 23.0 dB (DC -80, 0.15 kHz, -251.6 dBFS) | 11.5 dB (silence, 1.74 kHz, -239.4 dBFS) |
| 5 | `DSD64_48` | +0.14 | 18.3 dB (silence, 7.50 kHz, -343.3 dBFS) | 13.3 dB (1 kHz -120, 0.78 kHz, -155.1 dBFS) |
| 5 | `DSD128_48` | +0.44 | 18.0 dB (DC -40, 5.99 kHz, -341.3 dBFS) | 13.9 dB (1 kHz -120, 0.32 kHz, -197.2 dBFS) |
| 5 | `DSD256_48` | -0.09 | 17.2 dB (silence, 5.98 kHz, -344.1 dBFS) | 17.2 dB (DC -40, 5.98 kHz, -344.1 dBFS) |
| 5 | `DSD512_48` | -1.70 | 15.9 dB (DC -80, 2.23 kHz, -239.8 dBFS) | 15.4 dB (silence, 2.06 kHz, -240.8 dBFS) |
| 7 | `DSD64` | +0.32 | 9.2 dB (1 kHz -60, 0.12 kHz, -184.5 dBFS) | 9.2 dB (silence, 19.75 kHz, -158.8 dBFS) |
| 7 | `DSD128` | -0.03 | 11.9 dB (1 kHz -120, 0.05 kHz, -247.3 dBFS) | 10.2 dB (1 kHz -120, 15.08 kHz, -223.0 dBFS) |
| 7 | `DSD256` | +0.31 | 10.1 dB (DC -40, 19.94 kHz, -243.2 dBFS) | 11.4 dB (DC -80, 0.12 kHz, -277.9 dBFS) |
| 7 | `DSD512` | -0.83 | 15.7 dB (silence, 19.66 kHz, -292.5 dBFS) | 9.3 dB (DC -80, 16.17 kHz, -293.1 dBFS) |
| 7 | `DSD64_48` | -0.08 | 11.3 dB (1 kHz -120, 0.04 kHz, -218.2 dBFS) | 10.4 dB (1 kHz -60, 0.06 kHz, -199.3 dBFS) |
| 7 | `DSD128_48` | -0.26 | 10.0 dB (silence, 14.61 kHz, -231.5 dBFS) | 12.4 dB (1 kHz -100, 19.96 kHz, -205.0 dBFS) |
| 7 | `DSD256_48` | -0.22 | 10.4 dB (silence, 0.18 kHz, -279.7 dBFS) | 11.1 dB (silence, 0.12 kHz, -286.1 dBFS) |
| 7 | `DSD512_48` | at floor | 16.0 dB (DC -80, 19.80 kHz, -293.7 dBFS) | 10.9 dB (1 kHz -100, 0.23 kHz, -319.0 dBFS) |

### DC offsets: DC blocker off vs on (dither on)

A DC offset `u` at the modulator input gives an idle tone near `u * f_bit` that the dither does not remove (F9). The
production chain removes source DC with a first-order 1 Hz high-pass ahead of the cascade (`Design.md` §4.1). "Off" is the
chain without it (before O7). Residual DC is the record's windowed mean (dBFS). Prominence near 8.3 dB (white-noise
median, max 9.1 dB) means no tone above the noise.

| Order | Rate | DC (dBFS) | Off: worst spur (prominence, kHz, dBFS) | Off: residual DC (dBFS) | On: worst spur (prominence, kHz, dBFS) | On: residual DC (dBFS) |
|---:|---|---:|---|---:|---|---:|
| 5 | `DSD64` | -40 | 32.6 dB, 17.31 kHz, -118.9 | -40.0 | 3.1 dB, 16.03 kHz, -325.3 | -345.1 |
| 5 | `DSD64` | -80 | 35.7 dB, 0.16 kHz, -141.3 | -80.0 | 3.1 dB, 16.03 kHz, -325.3 | -345.1 |
| 5 | `DSD128` | -40 | 9.5 dB, 10.30 kHz, -187.6 | -40.0 | 9.0 dB, 0.16 kHz, -209.2 | -263.4 |
| 5 | `DSD128` | -80 | 36.1 dB, 0.33 kHz, -165.7 | -80.0 | 7.5 dB, 18.97 kHz, -177.9 | -262.3 |
| 5 | `DSD256` | -40 | 8.0 dB, 11.04 kHz, -223.2 | -40.0 | 11.9 dB, 0.09 kHz, -240.4 | -283.9 |
| 5 | `DSD256` | -80 | 34.9 dB, 0.65 kHz, -189.8 | -80.0 | 7.9 dB, 0.23 kHz, -237.4 | -271.3 |
| 5 | `DSD512` | -40 | 7.1 dB, 8.70 kHz, -236.4 | -40.0 | 9.7 dB, 19.83 kHz, -236.8 | -303.4 |
| 5 | `DSD512` | -80 | 34.1 dB, 1.36 kHz, -213.9 | -80.0 | 9.3 dB, 7.19 kHz, -244.0 | -305.9 |
| 5 | `DSD64_48` | -40 | 33.2 dB, 18.29 kHz, -133.6 | -40.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD64_48` | -80 | 33.7 dB, 0.18 kHz, -144.1 | -80.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD128_48` | -40 | 7.4 dB, 14.53 kHz, -172.1 | -40.0 | 8.0 dB, 10.39 kHz, -198.5 | -231.6 |
| 5 | `DSD128_48` | -80 | 34.1 dB, 0.37 kHz, -168.6 | -80.0 | 7.6 dB, 18.34 kHz, -198.4 | -232.4 |
| 5 | `DSD256_48` | -40 | 9.8 dB, 7.90 kHz, -205.5 | -40.0 | 17.2 dB, 5.98 kHz, -344.1 | -375.0 |
| 5 | `DSD256_48` | -80 | 34.2 dB, 0.71 kHz, -193.0 | -80.0 | 11.1 dB, 0.32 kHz, -229.8 | -290.9 |
| 5 | `DSD512_48` | -40 | 11.4 dB, 0.14 kHz, -266.7 | -40.0 | 8.1 dB, 0.23 kHz, -271.6 | -316.0 |
| 5 | `DSD512_48` | -80 | 35.7 dB, 1.41 kHz, -217.0 | -80.0 | 13.1 dB, 0.16 kHz, -256.4 | -303.6 |
| 7 | `DSD64` | -40 | 25.4 dB, 16.33 kHz, -138.1 | -40.0 | 8.9 dB, 0.20 kHz, -188.6 | -256.7 |
| 7 | `DSD64` | -80 | 27.2 dB, 0.16 kHz, -160.9 | -80.0 | 9.2 dB, 19.75 kHz, -158.8 | -244.0 |
| 7 | `DSD128` | -40 | 9.2 dB, 19.15 kHz, -208.3 | -40.0 | 7.8 dB, 7.99 kHz, -235.9 | -280.8 |
| 7 | `DSD128` | -80 | 28.0 dB, 0.31 kHz, -198.3 | -80.0 | 9.1 dB, 19.92 kHz, -200.0 | -284.4 |
| 7 | `DSD256` | -40 | 7.4 dB, 7.81 kHz, -261.2 | -40.0 | 9.9 dB, 0.17 kHz, -276.1 | -318.2 |
| 7 | `DSD256` | -80 | 27.1 dB, 0.61 kHz, -234.7 | -80.0 | 11.4 dB, 0.12 kHz, -277.9 | -324.5 |
| 7 | `DSD512` | -40 | 7.2 dB, 18.13 kHz, -285.7 | -40.0 | 7.4 dB, 16.32 kHz, -293.8 | -327.7 |
| 7 | `DSD512` | -80 | 27.3 dB, 1.21 kHz, -271.7 | -80.0 | 9.3 dB, 16.17 kHz, -293.1 | -356.5 |
| 7 | `DSD64_48` | -40 | 25.2 dB, 17.78 kHz, -141.5 | -40.0 | 7.7 dB, 14.78 kHz, -200.5 | -251.7 |
| 7 | `DSD64_48` | -80 | 28.0 dB, 0.17 kHz, -165.5 | -80.0 | 8.5 dB, 14.75 kHz, -197.7 | -251.3 |
| 7 | `DSD128_48` | -40 | 7.9 dB, 16.76 kHz, -205.1 | -40.0 | 7.2 dB, 18.69 kHz, -220.2 | -332.3 |
| 7 | `DSD128_48` | -80 | 26.8 dB, 0.33 kHz, -202.8 | -80.0 | 8.1 dB, 19.97 kHz, -210.7 | -294.3 |
| 7 | `DSD256_48` | -40 | 8.4 dB, 2.80 kHz, -247.1 | -40.0 | 8.9 dB, 8.51 kHz, -273.2 | -328.7 |
| 7 | `DSD256_48` | -80 | 27.5 dB, 0.66 kHz, -239.9 | -80.0 | 10.1 dB, 18.61 kHz, -259.9 | -327.7 |
| 7 | `DSD512_48` | -40 | 7.4 dB, 8.74 kHz, -303.7 | -40.0 | 7.9 dB, 17.77 kHz, -295.3 | -328.8 |
| 7 | `DSD512_48` | -80 | 28.8 dB, 1.34 kHz, -275.9 | -80.0 | 7.4 dB, 6.38 kHz, -300.6 | -338.0 |

### Two-tone intermodulation (0 dBFS peak, dither on)

IMD = power of the products `m*f1 + n*f2` (both non-zero, `|m| + |n| <= 7`) inside 20 Hz-20 kHz relative to the total tone
power; THD counts the harmonics of either tone. CCIF products: 1 kHz (d2), 18 kHz (d3); 21 kHz and above are out of band.

Tone level error is measured minus requested level of each tone (the cascade is flat to 1e-6 dB, so at 19/20 kHz this
is the modulator's signal transfer function).

| Order | Rate | Stimulus | IMD (dBc) | IMD level (dBFS) | THD (dBc) | Noise (dBFS) | Worst spur (prominence / dBFS) | Tone level error f1 / f2 (dB) | Peak abs state |
|---:|---|---|---:|---:|---:|---:|---|---:|---:|
| 5 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -124.6 | -126.3 | -118.7 | -119.0 | 9.7 / -176.8 | -0.0012 / +0.0013 | 1.070 |
| 5 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -130.2 | -133.2 | — | -116.3 | 9.5 / -162.4 | +0.0098 / +0.0110 | 1.036 |
| 5 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -154.6 | -156.2 | -151.7 | -151.9 | 9.7 / -165.9 | -0.0012 / +0.0003 | 1.075 |
| 5 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -162.5 | -165.5 | — | -149.9 | 10.2 / -169.3 | +0.0024 / +0.0027 | 1.077 |
| 5 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -185.4 | -187.1 | -184.2 | -185.0 | 9.5 / -234.8 | -0.0012 / +0.0001 | 1.087 |
| 5 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -193.0 | -196.0 | — | -182.6 | 10.1 / -201.8 | +0.0006 / +0.0007 | 1.058 |
| 5 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -215.2 | -216.8 | -216.8 | -218.0 | 10.9 / -235.0 | -0.0012 / +0.0000 | 1.087 |
| 5 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -223.2 | -226.2 | — | -215.8 | 15.8 / -237.5 | +0.0002 / +0.0002 | 1.062 |
| 5 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -127.6 | -129.3 | -122.6 | -123.0 | 8.4 / -182.9 | -0.0013 / +0.0011 | 1.015 |
| 5 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -133.7 | -136.7 | — | -120.4 | 9.3 / -141.0 | +0.0082 / +0.0092 | 1.013 |
| 5 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -158.0 | -159.7 | -155.4 | -156.9 | 9.5 / -172.0 | -0.0013 / +0.0003 | 1.090 |
| 5 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -165.5 | -168.5 | — | -153.6 | 8.5 / -174.7 | +0.0020 / +0.0023 | 1.048 |
| 5 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -188.8 | -190.5 | -188.3 | -189.7 | 13.5 / -247.8 | -0.0013 / +0.0001 | 1.076 |
| 5 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -196.2 | -199.2 | — | -186.9 | 10.7 / -204.8 | +0.0005 / +0.0006 | 1.070 |
| 5 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -219.6 | -221.2 | -221.1 | -222.6 | 9.8 / -238.9 | -0.0013 / +0.0000 | 1.092 |
| 5 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -227.1 | -230.1 | — | -219.6 | 11.7 / -259.5 | +0.0001 / +0.0001 | 1.063 |
| 7 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -150.1 | -151.8 | -137.5 | -137.5 | 7.5 / -157.9 | -0.0012 / +0.0018 | 1.035 |
| 7 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -142.6 | -145.6 | — | -135.3 | 11.7 / -159.7 | +0.0136 / +0.0150 | 1.013 |
| 7 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -191.9 | -193.5 | -182.2 | -182.6 | 10.3 / -236.7 | -0.0012 / +0.0005 | 1.014 |
| 7 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -184.3 | -187.3 | — | -179.8 | 7.4 / -215.3 | +0.0034 / +0.0038 | 1.026 |
| 7 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -235.0 | -236.7 | -227.4 | -227.4 | 15.5 / -284.2 | -0.0012 / +0.0001 | 1.073 |
| 7 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -226.5 | -229.5 | — | -225.1 | 17.4 / -262.0 | +0.0009 / +0.0009 | 1.073 |
| 7 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -274.0 | -275.7 | -263.7 | -266.9 | 24.6 / -276.9 | -0.0012 / +0.0000 | 1.055 |
| 7 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -266.6 | -269.6 | — | -243.8 | 29.7 / -262.6 | +0.0002 / +0.0002 | 1.051 |
| 7 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -154.6 | -156.3 | -142.4 | -143.5 | 9.0 / -183.5 | -0.0013 / +0.0016 | 1.059 |
| 7 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -146.7 | -149.7 | — | -140.2 | 9.6 / -161.5 | +0.0115 / +0.0127 | 1.029 |
| 7 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -197.4 | -199.1 | -186.9 | -187.7 | 9.0 / -203.1 | -0.0013 / +0.0004 | 1.017 |
| 7 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -189.0 | -192.1 | — | -185.2 | 8.7 / -245.6 | +0.0029 / +0.0032 | 1.021 |
| 7 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -239.7 | -241.4 | -232.2 | -233.1 | 8.7 / -275.3 | -0.0013 / +0.0001 | 1.061 |
| 7 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -231.6 | -234.6 | — | -230.2 | 20.0 / -260.1 | +0.0007 / +0.0008 | 1.032 |
| 7 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -276.7 | -278.4 | -266.2 | -264.3 | 24.1 / -270.7 | -0.0013 / +0.0000 | 1.048 |
| 7 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -273.5 | -276.5 | — | -243.4 | 30.4 / -262.8 | +0.0002 / +0.0002 | 1.077 |

### Ultrasonic noise (reported, not gated)

Out-of-band power per band, dBFS (PCM full scale). Neither the K17 nor the JA11 documents an ultrasonic input limit
(`docs/hardware/`), so this is information for the DAC and amplifier chain, not a check. "dBc @ -6" is relative to a
-6 dBFS tone, for comparison with earlier reports.

| Order | Rate | Silence 20-50 kHz | Silence 50-100 kHz | Silence 20-100 kHz | 0 dBFS 20-50 kHz | 0 dBFS 50-100 kHz | 0 dBFS 20-100 kHz | 20-100 kHz, dBc @ -6 dBFS |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | -278.1 | -46.0 | -46.0 | -57.1 | -26.2 | -26.2 | -20.6 |
| 5 | `DSD128` | -97.4 | -61.0 | -61.0 | -89.7 | -55.2 | -55.2 | -49.1 |
| 5 | `DSD256` | -135.9 | -101.0 | -101.0 | -122.7 | -88.6 | -88.6 | -82.0 |
| 5 | `DSD512` | -168.6 | -134.1 | -134.1 | -151.9 | -121.6 | -121.6 | -115.5 |
| 5 | `DSD64_48` | -97.4 | -41.5 | -41.5 | -60.5 | -28.3 | -28.3 | -22.4 |
| 5 | `DSD128_48` | -105.8 | -71.7 | -71.7 | -93.8 | -59.4 | -59.3 | -53.2 |
| 5 | `DSD256_48` | -139.9 | -104.5 | -104.5 | -127.5 | -92.7 | -92.7 | -86.5 |
| 5 | `DSD512_48` | -171.5 | -137.7 | -137.7 | -150.1 | -125.3 | -125.3 | -119.1 |
| 7 | `DSD64` | -55.3 | -34.7 | -34.7 | -49.4 | -21.1 | -21.1 | -15.0 |
| 7 | `DSD128` | -102.7 | -53.7 | -53.7 | -94.6 | -46.8 | -46.8 | -40.8 |
| 7 | `DSD256` | -148.3 | -100.8 | -100.8 | -139.2 | -92.4 | -92.4 | -86.1 |
| 7 | `DSD512` | -193.1 | -145.8 | -145.8 | -153.8 | -137.6 | -137.5 | -131.0 |
| 7 | `DSD64_48` | -62.3 | -35.9 | -35.9 | -55.0 | -21.8 | -21.8 | -15.6 |
| 7 | `DSD128_48` | -108.4 | -59.9 | -59.9 | -100.0 | -52.8 | -52.8 | -46.5 |
| 7 | `DSD256_48` | -153.3 | -106.8 | -106.8 | -143.5 | -98.1 | -98.1 | -91.3 |
| 7 | `DSD512_48` | -198.6 | -151.2 | -151.2 | -150.2 | -143.2 | -142.4 | -136.3 |

### Rate family: 44.1 kHz vs 48 kHz

Same DSD multiple, 48 kHz family minus 44.1 kHz family (the 48k family runs at an 8.8% higher bit rate, so a higher OSR).
Positive SNR/DR deltas favour 48k; "at floor" means one side is within 10 dB of the analysis floor.

| Order | DSD multiple | SNR @ 0 dBFS delta (dB) | DR (A) delta (dB) | THD @ 0 dBFS delta (dB) | Ultrasonic 20-100 kHz @ 0 dBFS delta (dB) |
|---:|---|---:|---:|---:|---:|
| 5 | DSD64 | +3.8 | +4.0 | -4.0 | -2.1 |
| 5 | DSD128 | +3.9 | +3.1 | -3.6 | -4.2 |
| 5 | DSD256 | +4.2 | +4.3 | -3.0 | -4.1 |
| 5 | DSD512 | +3.9 | +3.5 | -4.2 | -3.7 |
| 7 | DSD64 | +5.5 | +5.3 | -5.2 | -0.7 |
| 7 | DSD128 | +5.2 | +4.1 | -5.3 | -6.0 |
| 7 | DSD256 | +5.8 | +3.7 | -5.1 | -5.7 |
| 7 | DSD512 | at floor | +5.1 | -5.8 | -4.9 |
