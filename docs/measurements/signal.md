# audigma — Signal Measurements

> **Specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [README.md](README.md)

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
measured tone level (tones up to 10 kHz) within 0.01 dB of the requested PCM level (plus the
reading error the tone bin allows: 3 sigma of the noise inside the tone lobe, and a coherent in-bin component up to
-141 dBFS, the transparency spur limit; both matter only at the lowest levels; worst error over the checked records:
0.0496 dB).
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
| 5 | `DSD64` | A | 118.1 | -135.9 | -116.1 | -116.4 | 0.0110 | **EXCEPTION** (DR (A) 118.1 dB < 120; documented) |
| 5 | `DSD128` | B | 141.8 | -162.3 | -146.1 | -143.2 | 0.0027 | **PASS** |
| 5 | `DSD256` | B | 152.3 | -173.5 | -163.0 | -152.0 | 0.0007 | **PASS** |
| 5 | `DSD512` | B | 175.1 | -193.0 | -184.3 | -170.9 | 0.0002 | **PASS** |
| 5 | `DSD64_48` | A | 122.4 | -134.7 | -120.2 | -120.2 | 0.0092 | **PASS** |
| 5 | `DSD128_48` | B | 142.6 | -161.5 | -148.6 | -143.7 | 0.0023 | **PASS** |
| 5 | `DSD256_48` | B | 157.7 | -177.8 | -167.7 | -157.1 | 0.0006 | **PASS** |
| 5 | `DSD512_48` | B | 185.5 | -203.0 | -194.3 | -180.9 | 0.0001 | **PASS** |
| 7 | `DSD64` | A | 137.3 | -156.6 | -134.8 | -139.0 | 0.0150 | **PASS** |
| 7 | `DSD128` | B | 153.6 | -173.5 | -160.6 | -154.8 | 0.0038 | **PASS** |
| 7 | `DSD256` | B | 171.4 | -187.5 | -174.4 | -171.0 | 0.0009 | **PASS** |
| 7 | `DSD512` | B | 202.0 | -216.9 | -202.6 | -199.9 | 0.0002 | **PASS** |
| 7 | `DSD64_48` | A | 141.9 | -152.7 | -139.9 | -143.8 | 0.0127 | **PASS** |
| 7 | `DSD128_48` | B | 144.9 | -165.5 | -151.0 | -145.9 | 0.0032 | **PASS** |
| 7 | `DSD256_48` | B | 163.8 | -182.0 | -166.2 | -161.8 | 0.0008 | **PASS** |
| 7 | `DSD512_48` | B | 215.2 | -234.4 | -216.7 | -214.3 | 0.0002 | **PASS** |

### Summary per rate and order (dither on)

| Order | Rate | Family | L (dBFS) | Gain L - M (dB) | Noise, silence (dBFS) | Noise A-wtd, silence (dBFS) | SNR @ 0 dBFS (dB) | THD @ 0 dBFS (dBc) | THD @ 0 dBFS, M = 0 (dBc) | DR (A) (dB) | Ultrasonic 20-100 kHz, silence (dBFS) | Ultrasonic 20-100 kHz @ 0 dBFS (dBFS) |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | 44.1k | -4.25 | -4.50 | <= -124 | <= -124 | 116.4 | -123.2 | -123.0 | 118.1 | -45.8 | -26.2 |
| 5 | `DSD128` | 44.1k | -3.50 | -3.75 | -153.6 | -156.5 | 147.0 | -148.6 | -148.7 | 141.8 | -57.6 | -52.0 |
| 5 | `DSD256` | 44.1k | -2.00 | -2.25 | <= -169 | <= -169 | 167.3 | -161.8 | -161.0 | 152.3 | -75.1 | -72.2 |
| 5 | `DSD512` | 44.1k | -1.75 | -2.00 | <= -193 | <= -193 | 193.3 | -183.6 | -183.2 | 175.1 | -109.6 | -97.5 |
| 5 | `DSD64_48` | 48k | -4.25 | -4.50 | <= -145 | <= -145 | 120.9 | -127.0 | -126.8 | 122.4 | -41.5 | -28.3 |
| 5 | `DSD128_48` | 48k | -3.25 | -3.50 | <= -155 | <= -155 | 149.3 | -151.1 | -150.3 | 142.6 | -59.2 | -54.7 |
| 5 | `DSD256_48` | 48k | -2.00 | -2.25 | -175.1 | <= -179 | 171.8 | -165.2 | -164.8 | 157.7 | -80.4 | -76.5 |
| 5 | `DSD512_48` | 48k | -1.75 | -2.00 | <= -210 | <= -210 | 200.7 | -190.4 | -189.9 | 185.5 | -125.7 | -104.8 |
| 7 | `DSD64` | 44.1k | -4.50 | -4.75 | <= -141 | <= -141 | 135.0 | -141.9 | -141.7 | 137.3 | -34.7 | -21.1 |
| 7 | `DSD128` | 44.1k | -2.50 | -2.75 | <= -169 | <= -169 | 162.3 | -160.4 | -160.0 | 153.6 | -49.4 | -32.6 |
| 7 | `DSD256` | 44.1k | -2.00 | -2.25 | <= -178 | <= -178 | >= 178 | -176.3 | -175.6 | 171.4 | -80.5 | -46.2 |
| 7 | `DSD512` | 44.1k | -2.00 | -2.25 | <= -211 | <= -211 | >= 211 | -207.8 | -207.6 | 202.0 | -99.9 | -80.2 |
| 7 | `DSD64_48` | 48k | -5.00 | -5.25 | -148.2 | -151.2 | 140.2 | -146.9 | -146.7 | 141.9 | -35.4 | -21.3 |
| 7 | `DSD128_48` | 48k | -2.25 | -2.50 | <= -155 | <= -155 | >= 155 | -152.9 | -152.4 | 144.9 | -53.7 | -35.1 |
| 7 | `DSD256_48` | 48k | -2.00 | -2.25 | <= -175 | <= -175 | >= 175 | -169.9 | -170.2 | 163.8 | -64.1 | -44.3 |
| 7 | `DSD512_48` | 48k | -2.00 | -2.25 | <= -234 | <= -234 | 228.8 | -220.1 | -219.5 | 215.2 | -114.5 | -94.7 |

### Level sweep, 5th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 116.1 | 144.8 | 160.2 | 182.8 | 119.9 | 147.2 | 164.1 | 189.6 |
| 0 | 115.6 | 144.7 | 160.7 | 183.2 | 119.9 | 147.1 | 164.4 | 190.0 |
| -1 | 115.1 | 144.3 | 160.9 | 183.4 | 118.7 | 146.7 | 164.8 | 190.6 |
| -3 | 113.2 | 142.1 | 159.3 | 181.9 | 117.1 | 144.6 | 163.0 | 188.7 |
| -6 | 110.1 | 139.4 | 153.3 | 174.5 | 114.3 | 141.3 | 156.9 | 182.2 |
| -10 | 105.8 | 134.8 | 148.8 | 170.0 | 109.8 | 137.0 | 152.8 | 177.7 |
| -20 | 97.1 | 126.5 | 140.3 | 159.3 | 100.9 | 128.6 | 143.5 | 168.2 |
| -40 | 74.4 | 105.4 | 119.2 | 143.3 | 78.2 | 106.4 | 122.7 | 154.9 |
| -60 | 56.4 | 82.3 | 91.9 | 110.8 | 60.7 | 83.2 | 96.9 | 120.9 |
| -80 | 43.7 | 73.9 | >= 89 | 96.7 | 48.2 | >= 75 | >= 99 | 104.0 |
| -100 | 24.0 | 55.2 | >= 69 | >= 93 | 28.4 | >= 55 | 77.9 | 106.3 |
| -120 | >= 4 | 33.9 | >= 49 | >= 73 | 8.7 | >= 35 | 54.6 | 83.8 |

Worst spur per level (5th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 9.0 / -139.2 | 7.4 / -189.2 | 6.4 / -206.1 | 8.2 / -216.2 | 7.9 / -143.7 | 8.0 / -181.9 | 7.1 / -188.6 | 7.4 / -217.0 |
| 0 | 7.8 / -166.6 | 9.2 / -199.5 | 9.2 / -216.9 | 9.4 / -212.6 | 7.9 / -144.9 | 7.9 / -170.0 | 7.6 / -189.1 | 8.7 / -222.7 |
| -1 | 8.5 / -162.7 | 8.3 / -177.0 | 8.9 / -179.1 | 19.1 / -241.6 | 8.9 / -139.9 | 8.7 / -183.3 | 13.7 / -224.6 | 9.2 / -248.3 |
| -3 | 10.9 / -170.4 | 8.5 / -197.4 | 9.5 / -182.7 | 8.0 / -241.1 | 8.2 / -145.8 | 8.0 / -177.2 | 12.7 / -186.7 | 7.0 / -213.6 |
| -6 | 6.8 / -144.1 | 8.9 / -185.0 | 8.8 / -177.2 | 7.0 / -235.6 | 8.3 / -140.1 | 7.6 / -176.6 | 7.5 / -188.9 | 8.1 / -228.1 |
| -10 | 8.4 / -135.9 | 7.9 / -186.8 | 8.5 / -183.5 | 7.3 / -228.4 | 15.3 / -134.7 | 8.7 / -196.0 | 8.2 / -208.0 | 6.6 / -214.2 |
| -20 | 9.3 / -144.1 | 10.9 / -182.0 | 7.9 / -194.9 | 7.8 / -207.2 | 10.8 / -141.0 | 8.8 / -167.0 | 7.4 / -217.5 | 8.0 / -234.7 |
| -40 | 10.5 / -163.4 | 9.7 / -202.0 | 9.6 / -202.9 | 9.7 / -214.2 | 8.9 / -154.0 | 10.6 / -161.5 | 13.2 / -213.6 | 8.5 / -235.1 |
| -60 | 10.7 / -142.4 | 7.1 / -181.9 | 9.9 / -181.5 | 8.9 / -228.7 | 11.7 / -154.9 | 8.6 / -189.6 | 10.2 / -182.2 | 6.7 / -219.2 |
| -80 | 7.9 / -157.8 | 8.9 / -177.7 | 14.7 / -199.8 | 12.2 / -222.1 | 10.0 / -150.9 | 12.1 / -175.0 | 14.8 / -222.5 | 7.7 / -234.6 |
| -100 | 9.6 / -150.7 | 12.6 / -171.1 | 18.4 / -183.8 | 15.7 / -221.7 | 11.7 / -147.4 | 15.2 / -164.7 | 16.0 / -189.2 | 14.4 / -229.8 |
| -120 | 16.4 / -151.6 | 20.3 / -172.1 | 23.8 / -182.2 | 20.0 / -222.2 | 13.3 / -155.1 | 8.2 / -181.9 | 24.1 / -184.3 | 10.7 / -228.5 |
| silence | 3.1 / -325.1 | 9.1 / -212.2 | 6.9 / -221.1 | 7.6 / -225.0 | 2.5 / -330.6 | 8.5 / -177.3 | 8.4 / -203.8 | 6.7 / -229.2 |
| DC -40 | 3.1 / -325.1 | 8.4 / -173.4 | 8.6 / -216.3 | 10.1 / -212.1 | 2.5 / -330.6 | 10.9 / -170.5 | 8.5 / -193.2 | 7.1 / -252.2 |
| DC -80 | 3.1 / -325.1 | 9.1 / -212.2 | 6.9 / -221.1 | 7.0 / -227.4 | 2.5 / -330.6 | 8.5 / -177.3 | 8.1 / -202.2 | 10.6 / -227.7 |

### Level sweep, 7th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 134.7 | 158.1 | 174.8 | 207.0 | 139.7 | 150.9 | 169.2 | 219.1 |
| 0 | 134.2 | 158.2 | 175.2 | 207.3 | 139.4 | 151.1 | 168.9 | 219.5 |
| -1 | 133.7 | 157.9 | 175.1 | 207.3 | 138.3 | 150.9 | 169.7 | 220.0 |
| -3 | 131.7 | 156.7 | 170.6 | 201.8 | 136.9 | 148.6 | 163.8 | 214.6 |
| -6 | 128.6 | 152.4 | 165.5 | 195.9 | 133.5 | 143.2 | 158.6 | 209.3 |
| -10 | 124.8 | 147.4 | 162.6 | 193.0 | 129.6 | 139.2 | 155.6 | 206.1 |
| -20 | 115.7 | 139.2 | 150.4 | 179.6 | 120.4 | 129.0 | 141.6 | 193.9 |
| -40 | 94.7 | 118.7 | 133.3 | 160.8 | 99.7 | 109.3 | 122.6 | 176.7 |
| -60 | 75.6 | 94.3 | 110.5 | 139.8 | 80.4 | 85.4 | 102.3 | 152.9 |
| -80 | >= 61 | >= 89 | >= 98 | 124.3 | 67.5 | >= 75 | >= 95 | 135.0 |
| -100 | >= 41 | >= 69 | >= 78 | >= 111 | 48.1 | >= 55 | >= 75 | >= 134 |
| -120 | >= 21 | >= 49 | >= 58 | >= 91 | 27.8 | >= 35 | >= 55 | >= 114 |

Worst spur per level (7th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 8.2 / -174.2 | 11.1 / -219.6 | 9.9 / -224.8 | 8.2 / -242.9 | 8.5 / -184.9 | 7.7 / -198.7 | 7.7 / -193.2 | 14.5 / -237.3 |
| 0 | 8.4 / -169.4 | 7.6 / -220.5 | 8.2 / -226.2 | 8.0 / -228.4 | 8.8 / -178.6 | 7.2 / -197.5 | 6.9 / -211.5 | 9.2 / -248.3 |
| -1 | 8.3 / -157.8 | 9.8 / -202.6 | 8.4 / -211.5 | 7.9 / -237.2 | 7.8 / -191.0 | 8.7 / -197.8 | 7.7 / -224.6 | 7.0 / -254.0 |
| -3 | 9.7 / -163.8 | 9.1 / -176.6 | 8.5 / -217.3 | 9.6 / -238.6 | 8.3 / -162.5 | 10.3 / -193.7 | 7.3 / -204.3 | 8.1 / -239.3 |
| -6 | 9.1 / -156.6 | 8.4 / -181.8 | 8.7 / -212.9 | 7.8 / -232.8 | 8.0 / -190.6 | 8.4 / -182.6 | 9.1 / -225.1 | 9.4 / -247.6 |
| -10 | 8.3 / -158.4 | 9.1 / -173.5 | 8.5 / -214.3 | 8.8 / -240.1 | 8.1 / -164.7 | 7.6 / -173.0 | 8.1 / -202.3 | 8.6 / -248.0 |
| -20 | 7.8 / -157.6 | 8.0 / -201.4 | 8.9 / -227.6 | 9.3 / -250.1 | 9.6 / -172.2 | 9.4 / -165.5 | 8.4 / -195.5 | 8.1 / -244.8 |
| -40 | 8.2 / -160.2 | 8.4 / -187.0 | 8.1 / -187.5 | 8.4 / -221.3 | 8.1 / -168.5 | 10.2 / -172.0 | 7.7 / -188.5 | 7.7 / -234.4 |
| -60 | 8.3 / -175.5 | 7.1 / -205.3 | 8.1 / -189.0 | 10.9 / -216.9 | 8.5 / -160.2 | 8.1 / -189.3 | 8.0 / -182.0 | 6.9 / -235.3 |
| -80 | 8.0 / -183.1 | 11.7 / -213.3 | 8.4 / -218.4 | 7.0 / -226.5 | 9.7 / -187.1 | 12.8 / -188.3 | 10.2 / -221.9 | 8.0 / -240.1 |
| -100 | 8.2 / -161.2 | 15.1 / -196.7 | 13.8 / -201.9 | 14.6 / -266.8 | 8.5 / -172.6 | 16.9 / -177.2 | 11.2 / -200.7 | 11.0 / -270.3 |
| -120 | 9.1 / -180.7 | 18.6 / -190.2 | 17.2 / -202.7 | 11.4 / -242.7 | 8.3 / -183.0 | 25.3 / -181.1 | 18.1 / -199.6 | 16.3 / -247.3 |
| silence | 9.2 / -158.8 | 8.7 / -195.9 | 10.8 / -213.2 | 9.6 / -255.5 | 8.5 / -197.2 | 8.1 / -186.9 | 9.2 / -219.2 | 9.4 / -268.8 |
| DC -40 | 8.9 / -188.6 | 9.2 / -205.7 | 9.5 / -237.4 | 7.5 / -249.4 | 7.7 / -200.0 | 8.6 / -204.4 | 11.7 / -202.4 | 6.4 / -264.0 |
| DC -80 | 9.2 / -158.8 | 8.7 / -195.9 | 7.2 / -228.9 | 8.0 / -260.5 | 8.5 / -197.2 | 8.1 / -186.9 | 8.3 / -210.8 | 7.5 / -265.0 |

### Dither (TPDF 0.005) on vs off

Low-level stimuli with and without the production dither. "Noise delta" is the in-band noise with dither minus without at
-6 dBFS (the dither is added at the quantizer, so the loop shapes it like quantization noise). The spur columns give the
highest prominence over the stimuli -120/-100/-60 dBFS 1 kHz, silence and DC -40/-80 dBFS, with the stimulus and frequency.

| Order | Rate | Noise delta @ -6 dBFS (dB) | Worst spur, no dither | Worst spur, TPDF |
|---:|---|---:|---|---|
| 5 | `DSD64` | -0.05 | 14.2 dB (1 kHz -120, 0.75 kHz, -151.3 dBFS) | 16.4 dB (1 kHz -120, 0.76 kHz, -151.6 dBFS) |
| 5 | `DSD128` | +0.04 | 18.9 dB (1 kHz -120, 5.30 kHz, -163.0 dBFS) | 20.3 dB (1 kHz -120, 1.28 kHz, -172.1 dBFS) |
| 5 | `DSD256` | +0.57 | 23.6 dB (1 kHz -120, 6.62 kHz, -179.4 dBFS) | 23.8 dB (1 kHz -120, 2.56 kHz, -182.2 dBFS) |
| 5 | `DSD512` | -0.17 | 19.5 dB (1 kHz -120, 9.56 kHz, -213.3 dBFS) | 20.0 dB (1 kHz -120, 0.52 kHz, -222.2 dBFS) |
| 5 | `DSD64_48` | +0.14 | 18.3 dB (silence, 7.50 kHz, -343.3 dBFS) | 13.3 dB (1 kHz -120, 0.78 kHz, -155.1 dBFS) |
| 5 | `DSD128_48` | -0.10 | 21.4 dB (1 kHz -100, 19.97 kHz, -165.9 dBFS) | 15.2 dB (1 kHz -100, 19.97 kHz, -164.7 dBFS) |
| 5 | `DSD256_48` | -0.00 | 25.1 dB (1 kHz -120, 2.67 kHz, -185.8 dBFS) | 24.1 dB (1 kHz -120, 6.67 kHz, -184.3 dBFS) |
| 5 | `DSD512_48` | -0.44 | 17.2 dB (1 kHz -100, 0.21 kHz, -242.7 dBFS) | 14.4 dB (1 kHz -100, 9.87 kHz, -229.8 dBFS) |
| 7 | `DSD64` | +0.23 | 9.2 dB (1 kHz -60, 0.12 kHz, -184.5 dBFS) | 9.2 dB (silence, 19.75 kHz, -158.8 dBFS) |
| 7 | `DSD128` | -0.45 | 20.5 dB (1 kHz -120, 3.68 kHz, -179.0 dBFS) | 18.6 dB (1 kHz -120, 0.67 kHz, -190.2 dBFS) |
| 7 | `DSD256` | at floor | 21.6 dB (1 kHz -120, 19.60 kHz, -191.3 dBFS) | 17.2 dB (1 kHz -120, 1.57 kHz, -202.7 dBFS) |
| 7 | `DSD512` | at floor | 18.6 dB (1 kHz -120, 0.22 kHz, -253.4 dBFS) | 14.6 dB (1 kHz -100, 0.17 kHz, -266.8 dBFS) |
| 7 | `DSD64_48` | +0.07 | 11.7 dB (1 kHz -100, 19.95 kHz, -165.1 dBFS) | 8.5 dB (1 kHz -100, 15.79 kHz, -172.6 dBFS) |
| 7 | `DSD128_48` | at floor | 25.4 dB (1 kHz -120, 7.50 kHz, -180.5 dBFS) | 25.3 dB (1 kHz -120, 7.51 kHz, -181.1 dBFS) |
| 7 | `DSD256_48` | +0.06 | 23.4 dB (1 kHz -120, 0.29 kHz, -211.0 dBFS) | 18.1 dB (1 kHz -120, 6.67 kHz, -199.6 dBFS) |
| 7 | `DSD512_48` | -0.10 | 23.2 dB (silence, 5.67 kHz, -247.5 dBFS) | 16.3 dB (1 kHz -120, 19.88 kHz, -247.3 dBFS) |

### DC offsets: DC blocker off vs on (dither on)

A DC offset `u` at the modulator input gives an idle tone near `u * f_bit` that the dither does not remove (F9). The
production chain removes source DC with a first-order 1 Hz high-pass ahead of the cascade (`Design.md` §4.1). "Off" is the
chain without it (before O7). Residual DC is the record's windowed mean (dBFS). Prominence near 8.3 dB (white-noise
median, max 9.1 dB) means no tone above the noise.

| Order | Rate | DC (dBFS) | Off: worst spur (prominence, kHz, dBFS) | Off: residual DC (dBFS) | On: worst spur (prominence, kHz, dBFS) | On: residual DC (dBFS) |
|---:|---|---:|---|---:|---|---:|
| 5 | `DSD64` | -40 | 33.1 dB, 16.80 kHz, -115.8 | -40.0 | 3.1 dB, 16.03 kHz, -325.1 | -344.9 |
| 5 | `DSD64` | -80 | 32.1 dB, 0.16 kHz, -141.3 | -80.0 | 3.1 dB, 16.03 kHz, -325.1 | -344.9 |
| 5 | `DSD128` | -40 | 9.7 dB, 10.27 kHz, -183.3 | -40.0 | 8.4 dB, 16.51 kHz, -173.4 | -271.3 |
| 5 | `DSD128` | -80 | 35.9 dB, 0.36 kHz, -159.0 | -80.0 | 9.1 dB, 0.10 kHz, -212.2 | -275.2 |
| 5 | `DSD256` | -40 | 7.5 dB, 8.12 kHz, -185.4 | -40.0 | 8.6 dB, 0.34 kHz, -216.3 | -287.6 |
| 5 | `DSD256` | -80 | 39.5 dB, 0.85 kHz, -162.6 | -80.0 | 6.9 dB, 0.15 kHz, -221.1 | -262.2 |
| 5 | `DSD512` | -40 | 8.7 dB, 0.13 kHz, -238.5 | -40.0 | 10.1 dB, 19.60 kHz, -212.1 | -292.7 |
| 5 | `DSD512` | -80 | 38.5 dB, 1.72 kHz, -177.3 | -80.0 | 7.0 dB, 1.49 kHz, -227.4 | -297.4 |
| 5 | `DSD64_48` | -40 | 33.2 dB, 18.29 kHz, -133.6 | -40.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD64_48` | -80 | 33.7 dB, 0.18 kHz, -144.1 | -80.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD128_48` | -40 | 7.4 dB, 14.78 kHz, -170.0 | -40.0 | 10.9 dB, 19.97 kHz, -170.5 | -279.1 |
| 5 | `DSD128_48` | -80 | 38.1 dB, 0.39 kHz, -159.5 | -80.0 | 8.5 dB, 7.72 kHz, -177.3 | -275.8 |
| 5 | `DSD256_48` | -40 | 9.5 dB, 5.07 kHz, -186.8 | -40.0 | 8.5 dB, 9.77 kHz, -193.2 | -315.5 |
| 5 | `DSD256_48` | -80 | 38.7 dB, 0.91 kHz, -165.7 | -80.0 | 8.1 dB, 9.36 kHz, -202.2 | -285.1 |
| 5 | `DSD512_48` | -40 | 10.5 dB, 0.68 kHz, -227.0 | -40.0 | 7.1 dB, 0.61 kHz, -252.2 | -265.6 |
| 5 | `DSD512_48` | -80 | 39.2 dB, 1.88 kHz, -184.6 | -80.0 | 10.6 dB, 19.83 kHz, -227.7 | -281.6 |
| 7 | `DSD64` | -40 | 25.4 dB, 16.33 kHz, -138.1 | -40.0 | 8.9 dB, 0.20 kHz, -188.6 | -256.7 |
| 7 | `DSD64` | -80 | 27.2 dB, 0.16 kHz, -160.9 | -80.0 | 9.2 dB, 19.75 kHz, -158.8 | -244.0 |
| 7 | `DSD128` | -40 | 10.3 dB, 0.05 kHz, -217.1 | -40.0 | 9.2 dB, 8.52 kHz, -205.7 | -261.4 |
| 7 | `DSD128` | -80 | 28.7 dB, 0.40 kHz, -170.2 | -80.0 | 8.7 dB, 7.03 kHz, -195.9 | -269.5 |
| 7 | `DSD256` | -40 | 9.0 dB, 18.53 kHz, -204.8 | -40.0 | 9.5 dB, 0.22 kHz, -237.4 | -247.6 |
| 7 | `DSD256` | -80 | 32.0 dB, 0.84 kHz, -176.6 | -80.0 | 7.2 dB, 8.93 kHz, -228.9 | -260.1 |
| 7 | `DSD512` | -40 | 10.0 dB, 0.15 kHz, -254.8 | -40.0 | 7.5 dB, 18.15 kHz, -249.4 | -302.3 |
| 7 | `DSD512` | -80 | 14.5 dB, 8.66 kHz, -227.2 | -80.0 | 8.0 dB, 8.85 kHz, -260.5 | -318.7 |
| 7 | `DSD64_48` | -40 | 23.5 dB, 16.79 kHz, -142.0 | -40.0 | 7.7 dB, 14.78 kHz, -200.0 | -251.2 |
| 7 | `DSD64_48` | -80 | 28.6 dB, 0.16 kHz, -165.7 | -80.0 | 8.5 dB, 14.75 kHz, -197.2 | -250.8 |
| 7 | `DSD128_48` | -40 | 9.6 dB, 0.09 kHz, -200.2 | -40.0 | 8.6 dB, 14.66 kHz, -204.4 | -264.4 |
| 7 | `DSD128_48` | -80 | 31.6 dB, 0.46 kHz, -159.7 | -80.0 | 8.1 dB, 1.24 kHz, -186.9 | -278.8 |
| 7 | `DSD256_48` | -40 | 7.2 dB, 12.13 kHz, -187.1 | -40.0 | 11.7 dB, 19.93 kHz, -202.4 | -259.4 |
| 7 | `DSD256_48` | -80 | 27.1 dB, 0.91 kHz, -169.0 | -80.0 | 8.3 dB, 15.59 kHz, -210.8 | -272.4 |
| 7 | `DSD512_48` | -40 | 8.2 dB, 0.63 kHz, -251.0 | -40.0 | 6.4 dB, 2.98 kHz, -264.0 | -314.7 |
| 7 | `DSD512_48` | -80 | 23.3 dB, 5.74 kHz, -220.8 | -80.0 | 7.5 dB, 10.20 kHz, -265.0 | -322.0 |

### Two-tone intermodulation (0 dBFS peak, dither on)

IMD = power of the products `m*f1 + n*f2` (both non-zero, `|m| + |n| <= 7`) inside 20 Hz-20 kHz relative to the total tone
power; THD counts the harmonics of either tone. CCIF products: 1 kHz (d2), 18 kHz (d3); 21 kHz and above are out of band.

Tone level error is measured minus requested level of each tone (the cascade is flat to 1e-6 dB, so at 19/20 kHz this
is the modulator's signal transfer function).

| Order | Rate | Stimulus | IMD (dBc) | IMD level (dBFS) | THD (dBc) | Noise (dBFS) | Worst spur (prominence / dBFS) | Tone level error f1 / f2 (dB) | Peak abs state |
|---:|---|---|---:|---:|---:|---:|---|---:|---:|
| 5 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -124.1 | -125.8 | -118.7 | -119.1 | 7.3 / -158.9 | -0.0012 / +0.0013 | 1.013 |
| 5 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -130.9 | -133.9 | — | -116.3 | 10.1 / -160.9 | +0.0098 / +0.0110 | 1.028 |
| 5 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -150.4 | -152.1 | -147.8 | -148.0 | 12.7 / -163.0 | -0.0012 / +0.0003 | 0.949 |
| 5 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -156.5 | -159.5 | — | -146.4 | 11.3 / -162.3 | +0.0024 / +0.0027 | 0.929 |
| 5 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -165.7 | -167.4 | -166.0 | -163.0 | 22.1 / -175.8 | -0.0012 / +0.0001 | 0.632 |
| 5 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -170.4 | -173.4 | — | -163.6 | 20.2 / -173.5 | +0.0006 / +0.0007 | 0.633 |
| 5 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -188.5 | -190.2 | -189.5 | -184.5 | 25.6 / -193.0 | -0.0012 / +0.0000 | 0.505 |
| 5 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -192.1 | -195.1 | — | -186.8 | 26.7 / -197.5 | +0.0002 / +0.0002 | 0.505 |
| 5 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -127.6 | -129.3 | -122.6 | -123.0 | 8.4 / -182.9 | -0.0013 / +0.0011 | 1.015 |
| 5 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -133.7 | -136.7 | — | -120.4 | 9.3 / -141.0 | +0.0082 / +0.0092 | 1.013 |
| 5 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -152.3 | -154.0 | -149.8 | -151.2 | 16.1 / -164.1 | -0.0013 / +0.0003 | 0.908 |
| 5 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -157.0 | -160.0 | — | -148.6 | 14.2 / -163.2 | +0.0021 / +0.0023 | 0.883 |
| 5 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -169.3 | -171.0 | -166.7 | -168.5 | 21.7 / -178.9 | -0.0013 / +0.0001 | 0.633 |
| 5 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -173.7 | -176.7 | — | -167.7 | 20.5 / -177.8 | +0.0005 / +0.0006 | 0.631 |
| 5 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -195.1 | -196.8 | -192.3 | -194.3 | 25.1 / -203.0 | -0.0013 / +0.0000 | 0.559 |
| 5 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -199.1 | -202.2 | — | -194.4 | 24.5 / -205.6 | +0.0001 / +0.0001 | 0.560 |
| 7 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -149.7 | -151.4 | -137.4 | -137.3 | 7.7 / -171.1 | -0.0012 / +0.0018 | 1.042 |
| 7 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -141.9 | -144.9 | — | -134.8 | 8.4 / -184.6 | +0.0136 / +0.0150 | 1.023 |
| 7 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -170.7 | -172.4 | -163.5 | -162.7 | 18.3 / -188.0 | -0.0012 / +0.0005 | 0.720 |
| 7 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -162.1 | -165.1 | — | -160.8 | 17.6 / -180.0 | +0.0034 / +0.0038 | 0.722 |
| 7 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -188.5 | -190.2 | -181.3 | -177.6 | 22.4 / -188.3 | -0.0012 / +0.0001 | 0.442 |
| 7 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -179.0 | -182.0 | — | -177.4 | 22.9 / -206.1 | +0.0009 / +0.0009 | 0.445 |
| 7 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -220.7 | -222.4 | -213.7 | -208.1 | 26.2 / -219.9 | -0.0012 / +0.0000 | 0.357 |
| 7 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -210.1 | -213.1 | — | -209.0 | 25.2 / -218.8 | +0.0002 / +0.0002 | 0.359 |
| 7 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -154.8 | -156.5 | -142.2 | -142.6 | 8.3 / -166.6 | -0.0013 / +0.0016 | 1.006 |
| 7 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -147.7 | -150.7 | — | -139.9 | 8.5 / -152.7 | +0.0115 / +0.0127 | 0.991 |
| 7 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -164.6 | -166.3 | -155.0 | <= -155 | 20.7 / -175.3 | -0.0013 / +0.0004 | 0.573 |
| 7 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -155.5 | -158.5 | — | -153.4 | 18.7 / -185.1 | +0.0029 / +0.0032 | 0.572 |
| 7 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -184.0 | -185.6 | -172.8 | -171.9 | 22.6 / -186.4 | -0.0013 / +0.0001 | 0.357 |
| 7 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -172.7 | -175.7 | — | -170.6 | 23.2 / -186.5 | +0.0007 / +0.0008 | 0.356 |
| 7 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -232.9 | -234.6 | -224.1 | -223.1 | 24.4 / -244.9 | -0.0013 / +0.0000 | 0.417 |
| 7 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -222.8 | -225.8 | — | -223.0 | 25.6 / -237.0 | +0.0002 / +0.0002 | 0.417 |

### Ultrasonic noise (digital domain noise analysis)

Out-of-band power per band, dBFS (PCM full scale). This is mathematical noise analysis of audigma's output per band that would enter the receiving device, not a DAC analog output noise measurement. "dBc @ -6" is relative to a
-6 dBFS tone, for comparison with earlier reports.

| Order | Rate | Silence 20-50 kHz | Silence 50-100 kHz | Silence 20-100 kHz | 0 dBFS 20-50 kHz | 0 dBFS 50-100 kHz | 0 dBFS 20-100 kHz | 20-100 kHz, dBc @ -6 dBFS |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | -277.8 | -45.8 | -45.8 | -56.8 | -26.2 | -26.2 | -20.1 |
| 5 | `DSD128` | -93.6 | -57.6 | -57.6 | -87.3 | -52.0 | -52.0 | -46.0 |
| 5 | `DSD256` | -115.2 | -75.1 | -75.1 | -106.7 | -72.2 | -72.2 | -65.6 |
| 5 | `DSD512` | -145.3 | -109.6 | -109.6 | -129.8 | -97.5 | -97.5 | -90.5 |
| 5 | `DSD64_48` | -97.4 | -41.5 | -41.5 | -60.5 | -28.3 | -28.3 | -22.4 |
| 5 | `DSD128_48` | -96.4 | -59.2 | -59.2 | -89.7 | -54.7 | -54.7 | -48.7 |
| 5 | `DSD256_48` | -118.8 | -80.4 | -80.4 | -110.9 | -76.5 | -76.5 | -70.3 |
| 5 | `DSD512_48` | -160.7 | -125.7 | -125.7 | -137.6 | -104.8 | -104.8 | -98.8 |
| 7 | `DSD64` | -55.3 | -34.7 | -34.7 | -49.7 | -21.1 | -21.1 | -15.0 |
| 7 | `DSD128` | -85.3 | -49.4 | -49.4 | -76.3 | -32.6 | -32.6 | -25.7 |
| 7 | `DSD256` | -115.7 | -80.5 | -80.5 | -94.6 | -46.2 | -46.2 | -39.7 |
| 7 | `DSD512` | -148.1 | -99.9 | -99.9 | -125.5 | -80.2 | -80.2 | -74.4 |
| 7 | `DSD64_48` | -61.8 | -35.4 | -35.4 | -54.9 | -21.3 | -21.3 | -15.1 |
| 7 | `DSD128_48` | -76.9 | -53.7 | -53.7 | -69.7 | -35.1 | -35.1 | -28.0 |
| 7 | `DSD256_48` | -103.4 | -64.1 | -64.1 | -87.1 | -44.3 | -44.3 | -38.3 |
| 7 | `DSD512_48` | -162.8 | -114.5 | -114.5 | -139.1 | -94.7 | -94.7 | -88.5 |

### Rate family: 44.1 kHz vs 48 kHz

Same DSD multiple, 48 kHz family minus 44.1 kHz family (the 48k family runs at an 8.8% higher bit rate, so a higher OSR).
Positive SNR/DR deltas favour 48k; "at floor" means one side is within 10 dB of the analysis floor.

| Order | DSD multiple | SNR @ 0 dBFS delta (dB) | DR (A) delta (dB) | THD @ 0 dBFS delta (dB) | Ultrasonic 20-100 kHz @ 0 dBFS delta (dB) |
|---:|---|---:|---:|---:|---:|
| 5 | DSD64 | +4.5 | +4.3 | -3.7 | -2.1 |
| 5 | DSD128 | +2.3 | +0.8 | -2.5 | -2.7 |
| 5 | DSD256 | +4.5 | +5.4 | -3.5 | -4.3 |
| 5 | DSD512 | +7.4 | +10.4 | -6.8 | -7.3 |
| 7 | DSD64 | +5.2 | +4.6 | -5.0 | -0.1 |
| 7 | DSD128 | at floor | -8.6 | +7.5 | -2.5 |
| 7 | DSD256 | at floor | -7.6 | +6.4 | +2.0 |
| 7 | DSD512 | at floor | +13.1 | -12.2 | -14.6 |
