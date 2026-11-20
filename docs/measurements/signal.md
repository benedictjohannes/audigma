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
| 5 | `DSD128` | B | 140.4 | -159.0 | -145.2 | -141.8 | 0.0027 | **PASS** |
| 5 | `DSD256` | B | 153.0 | -173.3 | -164.0 | -153.1 | 0.0007 | **PASS** |
| 5 | `DSD512` | B | 172.7 | -190.9 | -182.1 | -168.5 | 0.0002 | **PASS** |
| 5 | `DSD64_48` | A | 122.4 | -134.7 | -120.2 | -120.2 | 0.0092 | **PASS** |
| 5 | `DSD128_48` | B | 141.3 | -161.3 | -147.8 | -142.4 | 0.0023 | **PASS** |
| 5 | `DSD256_48` | B | 158.9 | -178.3 | -168.8 | -158.3 | 0.0006 | **PASS** |
| 5 | `DSD512_48` | B | 183.4 | -200.0 | -192.4 | -178.8 | 0.0001 | **PASS** |
| 7 | `DSD64` | A | 137.3 | -156.6 | -134.8 | -139.0 | 0.0150 | **PASS** |
| 7 | `DSD128` | B | 151.5 | -174.4 | -158.9 | -152.7 | 0.0038 | **PASS** |
| 7 | `DSD256` | B | 167.9 | -190.6 | -170.4 | -166.5 | 0.0009 | **PASS** |
| 7 | `DSD512` | B | 197.8 | -215.9 | -197.8 | -194.8 | 0.0002 | **PASS** |
| 7 | `DSD64_48` | A | 142.5 | -166.2 | -140.4 | -144.1 | 0.0127 | **PASS** |
| 7 | `DSD128_48` | B | 142.2 | -158.4 | -147.6 | -143.2 | 0.0031 | **PASS** |
| 7 | `DSD256_48` | B | 158.8 | -181.1 | -161.7 | -156.9 | 0.0008 | **PASS** |
| 7 | `DSD512_48` | B | 212.2 | -232.0 | -212.0 | -209.7 | 0.0002 | **PASS** |

### Summary per rate and order (dither on)

| Order | Rate | Family | L (dBFS) | Gain L - M (dB) | Noise, silence (dBFS) | Noise A-wtd, silence (dBFS) | SNR @ 0 dBFS (dB) | THD @ 0 dBFS (dBc) | THD @ 0 dBFS, M = 0 (dBc) | DR (A) (dB) | Ultrasonic 20-100 kHz, silence (dBFS) | Ultrasonic 20-100 kHz @ 0 dBFS (dBFS) |
|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | 44.1k | -4.00 | -4.25 | <= -124 | <= -124 | 117.1 | -123.0 | -122.9 | 118.4 | -46.0 | -26.2 |
| 5 | `DSD128` | 44.1k | -3.50 | -3.75 | -152.9 | -155.5 | 146.4 | -147.8 | -147.8 | 140.4 | -55.7 | -51.7 |
| 5 | `DSD256` | 44.1k | -2.25 | -2.50 | <= -171 | <= -171 | 169.1 | -163.7 | -162.9 | 153.0 | -75.5 | -73.8 |
| 5 | `DSD512` | 44.1k | -1.75 | -2.00 | <= -191 | <= -191 | >= 191 | -181.8 | -181.4 | 172.7 | -107.0 | -95.7 |
| 5 | `DSD64_48` | 48k | -4.25 | -4.50 | <= -145 | <= -145 | 120.9 | -127.0 | -126.8 | 122.4 | -41.5 | -28.3 |
| 5 | `DSD128_48` | 48k | -3.25 | -3.50 | <= -154 | <= -154 | 148.6 | -150.2 | -149.6 | 141.3 | -56.7 | -53.9 |
| 5 | `DSD256_48` | 48k | -2.25 | -2.50 | -178.6 | <= -180 | 172.6 | -166.8 | -166.5 | 158.9 | -80.8 | -77.3 |
| 5 | `DSD512_48` | 48k | -1.75 | -2.00 | <= -207 | <= -207 | 199.7 | -188.6 | -188.2 | 183.4 | -124.9 | -102.9 |
| 7 | `DSD64` | 44.1k | -4.50 | -4.75 | <= -141 | <= -141 | 135.0 | -141.9 | -141.7 | 137.3 | -34.7 | -21.1 |
| 7 | `DSD128` | 44.1k | -2.50 | -2.75 | <= -167 | <= -167 | 160.4 | -158.6 | -158.2 | 151.5 | -49.8 | -32.7 |
| 7 | `DSD256` | 44.1k | -2.00 | -2.25 | <= -174 | <= -174 | >= 174 | -172.7 | -172.3 | 167.9 | -61.4 | -44.0 |
| 7 | `DSD512` | 44.1k | -2.00 | -2.25 | <= -206 | <= -206 | >= 206 | -203.1 | -203.4 | 197.8 | -94.7 | -74.8 |
| 7 | `DSD64_48` | 48k | -4.50 | -4.75 | -148.7 | -151.7 | 140.9 | -146.6 | -145.9 | 142.5 | -35.9 | -21.8 |
| 7 | `DSD128_48` | 48k | -2.25 | -2.50 | <= -152 | <= -152 | >= 152 | -150.3 | -150.2 | 142.2 | -54.2 | -35.5 |
| 7 | `DSD256_48` | 48k | -2.00 | -2.25 | <= -170 | <= -170 | >= 170 | -165.7 | -166.2 | 158.8 | -65.3 | -44.1 |
| 7 | `DSD512_48` | 48k | -2.00 | -2.25 | <= -229 | <= -229 | 225.4 | -216.7 | -216.4 | 212.2 | -110.5 | -90.9 |

### Level sweep, 5th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 116.0 | 143.9 | 161.8 | 181.0 | 119.9 | 146.1 | 165.7 | 187.8 |
| 0 | 116.1 | 144.0 | 162.6 | 181.4 | 119.9 | 146.3 | 165.8 | 188.3 |
| -1 | 115.1 | 143.3 | 162.5 | 181.6 | 118.7 | 145.5 | 166.2 | 188.9 |
| -3 | 113.4 | 141.2 | 160.4 | 180.1 | 117.1 | 143.5 | 164.2 | 186.6 |
| -6 | 110.4 | 138.2 | 154.8 | 172.5 | 114.3 | 140.2 | 158.4 | 180.0 |
| -10 | 106.2 | 134.3 | 150.3 | 167.8 | 109.8 | 136.3 | 154.2 | 175.7 |
| -20 | 97.6 | 125.6 | 141.9 | 156.7 | 100.9 | 127.5 | 146.2 | 165.8 |
| -40 | 74.7 | 104.4 | 121.3 | 140.9 | 78.2 | 105.3 | 121.7 | 151.9 |
| -60 | 56.6 | 81.1 | 92.9 | 108.4 | 60.7 | 81.9 | 98.1 | 118.8 |
| -80 | 44.0 | 73.7 | >= 91 | 94.1 | 48.2 | >= 74 | 100.1 | 101.9 |
| -100 | >= 24 | 54.4 | >= 71 | >= 91 | 28.4 | >= 54 | 79.0 | 104.5 |
| -120 | >= 4 | 33.0 | >= 51 | >= 71 | 8.7 | >= 34 | 56.2 | 82.1 |

Worst spur per level (5th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 9.9 / -130.2 | 9.3 / -181.6 | 8.8 / -212.4 | 8.4 / -231.1 | 7.9 / -143.7 | 8.1 / -181.3 | 7.9 / -189.3 | 7.7 / -223.3 |
| 0 | 9.0 / -139.2 | 7.1 / -170.0 | 10.1 / -222.1 | 13.9 / -204.3 | 7.9 / -144.9 | 7.3 / -194.7 | 8.7 / -213.9 | 9.2 / -221.5 |
| -1 | 8.3 / -170.1 | 9.0 / -169.5 | 8.3 / -218.5 | 8.1 / -216.7 | 8.9 / -139.9 | 8.6 / -183.4 | 7.8 / -198.5 | 9.8 / -209.3 |
| -3 | 7.7 / -140.7 | 8.1 / -163.2 | 10.5 / -217.4 | 8.9 / -228.8 | 8.2 / -145.8 | 9.0 / -181.4 | 7.3 / -208.9 | 7.0 / -224.8 |
| -6 | 7.9 / -141.9 | 8.1 / -194.1 | 8.0 / -192.2 | 10.3 / -238.3 | 8.3 / -140.1 | 7.2 / -169.0 | 7.0 / -190.0 | 10.6 / -237.9 |
| -10 | 13.0 / -159.5 | 7.6 / -167.4 | 11.8 / -179.7 | 6.8 / -213.3 | 15.3 / -134.7 | 10.5 / -161.3 | 9.8 / -197.5 | 8.0 / -214.1 |
| -20 | 8.8 / -167.6 | 10.3 / -159.0 | 9.3 / -202.3 | 7.0 / -206.0 | 10.8 / -141.0 | 10.6 / -198.0 | 8.6 / -196.3 | 10.7 / -206.5 |
| -40 | 11.0 / -139.8 | 11.2 / -159.3 | 9.8 / -183.0 | 10.8 / -203.5 | 8.9 / -154.0 | 13.8 / -165.8 | 7.3 / -199.2 | 7.4 / -209.8 |
| -60 | 12.3 / -135.5 | 12.1 / -163.7 | 8.8 / -203.2 | 9.3 / -230.1 | 11.7 / -154.9 | 7.5 / -173.5 | 8.7 / -221.3 | 13.4 / -242.3 |
| -80 | 8.0 / -156.2 | 10.0 / -174.4 | 16.5 / -219.4 | 10.9 / -218.6 | 10.0 / -150.9 | 9.5 / -172.6 | 14.3 / -227.8 | 9.6 / -206.1 |
| -100 | 11.8 / -150.4 | 13.7 / -170.7 | 18.1 / -187.9 | 18.9 / -205.0 | 11.7 / -147.4 | 15.6 / -171.5 | 16.3 / -194.7 | 16.0 / -227.8 |
| -120 | 16.4 / -152.1 | 22.7 / -160.8 | 25.3 / -210.5 | 20.7 / -221.4 | 13.3 / -155.1 | 8.3 / -172.1 | 28.1 / -216.3 | 12.3 / -230.4 |
| silence | 3.1 / -325.3 | 8.6 / -200.5 | 8.6 / -214.8 | 9.0 / -212.3 | 2.5 / -330.6 | 8.4 / -181.6 | 8.6 / -223.0 | 7.7 / -231.0 |
| DC -40 | 3.1 / -325.3 | 8.4 / -188.7 | 9.4 / -210.3 | 7.9 / -215.8 | 2.5 / -330.6 | 9.7 / -213.8 | 7.6 / -212.4 | 7.6 / -231.3 |
| DC -80 | 3.1 / -325.3 | 8.6 / -200.5 | 8.3 / -215.1 | 15.8 / -258.1 | 2.5 / -330.6 | 8.4 / -181.6 | 8.6 / -223.0 | 8.3 / -256.2 |

### Level sweep, 7th order (1 kHz, dither on)

SINAD (dB) per PCM level. The first row is 0 dBFS with `M = 0` (sine peak at `L`). Constant noise with falling level shows
up as SINAD falling 1:1; a level-dependent noise floor or distortion would bend it.

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 134.7 | 156.3 | 171.5 | 202.7 | 139.7 | 148.4 | 165.0 | 216.1 |
| 0 | 134.2 | 156.4 | 171.6 | 202.5 | 139.8 | 148.7 | 164.5 | 216.2 |
| -1 | 133.7 | 156.0 | 171.5 | 203.0 | 139.2 | 148.5 | 165.3 | 216.5 |
| -3 | 131.7 | 154.5 | 167.1 | 197.1 | 137.0 | 145.6 | 159.5 | 210.9 |
| -6 | 128.6 | 150.3 | 161.9 | 191.2 | 134.1 | 140.1 | 153.9 | 205.3 |
| -10 | 124.8 | 145.3 | 158.7 | 187.9 | 129.9 | 136.4 | 150.8 | 202.2 |
| -20 | 115.7 | 136.9 | 146.0 | 174.6 | 121.2 | 126.3 | 136.7 | 189.4 |
| -40 | 94.7 | 116.3 | 128.4 | 156.3 | 99.7 | 105.4 | 117.7 | 171.3 |
| -60 | 75.6 | 92.2 | 107.0 | 135.8 | 81.0 | 82.7 | 97.1 | 149.6 |
| -80 | >= 61 | >= 87 | >= 94 | 118.3 | 68.2 | >= 72 | >= 90 | 131.4 |
| -100 | >= 41 | >= 67 | >= 74 | >= 106 | 48.2 | >= 52 | >= 70 | >= 129 |
| -120 | >= 21 | >= 47 | >= 54 | >= 86 | 28.6 | >= 32 | >= 50 | >= 109 |

Worst spur per level (7th order, dither on): prominence above the local noise (dB) / absolute level (dBFS).

| Level (dBFS) | `DSD64` | `DSD128` | `DSD256` | `DSD512` | `DSD64_48` | `DSD128_48` | `DSD256_48` | `DSD512_48` |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 0 (M = 0) | 8.2 / -174.2 | 7.3 / -185.3 | 7.0 / -209.5 | 12.1 / -220.9 | 7.9 / -173.5 | 9.0 / -188.0 | 12.6 / -181.5 | 8.5 / -248.0 |
| 0 | 8.4 / -169.4 | 9.5 / -205.9 | 8.5 / -208.7 | 9.2 / -229.9 | 9.7 / -170.7 | 8.3 / -177.9 | 12.0 / -181.1 | 11.6 / -253.2 |
| -1 | 8.3 / -157.8 | 8.7 / -190.2 | 8.6 / -192.3 | 7.6 / -227.2 | 9.4 / -200.9 | 7.2 / -175.9 | 9.3 / -210.5 | 8.0 / -237.6 |
| -3 | 9.7 / -163.8 | 8.2 / -198.3 | 11.0 / -228.6 | 8.8 / -219.9 | 9.8 / -189.8 | 9.1 / -163.0 | 9.7 / -199.4 | 7.8 / -239.1 |
| -6 | 9.1 / -156.6 | 9.5 / -188.6 | 11.3 / -190.6 | 7.6 / -223.0 | 7.8 / -170.0 | 8.3 / -165.3 | 10.9 / -183.5 | 8.2 / -244.7 |
| -10 | 8.3 / -158.4 | 7.3 / -183.6 | 7.1 / -213.8 | 8.3 / -233.0 | 8.1 / -183.7 | 9.2 / -193.1 | 8.0 / -190.9 | 8.6 / -235.0 |
| -20 | 7.8 / -157.6 | 10.0 / -174.4 | 8.9 / -194.2 | 8.3 / -249.1 | 9.8 / -203.9 | 7.4 / -186.0 | 6.9 / -206.1 | 7.2 / -237.6 |
| -40 | 8.2 / -160.2 | 7.8 / -186.3 | 11.5 / -217.7 | 7.1 / -218.1 | 9.1 / -170.8 | 9.1 / -158.4 | 7.8 / -198.4 | 9.4 / -253.7 |
| -60 | 8.3 / -175.5 | 7.8 / -181.4 | 9.3 / -206.1 | 7.0 / -223.3 | 10.4 / -199.3 | 7.4 / -185.8 | 6.9 / -187.2 | 7.5 / -232.0 |
| -80 | 8.0 / -183.1 | 8.8 / -199.7 | 7.9 / -208.9 | 11.6 / -217.4 | 8.7 / -166.2 | 11.1 / -204.1 | 11.1 / -230.8 | 7.7 / -239.3 |
| -100 | 8.2 / -161.2 | 15.8 / -196.0 | 15.2 / -231.1 | 13.1 / -257.8 | 10.2 / -208.2 | 16.3 / -174.3 | 12.7 / -216.9 | 10.3 / -259.3 |
| -120 | 9.1 / -180.7 | 21.4 / -178.9 | 20.0 / -199.5 | 14.6 / -231.5 | 8.2 / -175.7 | 23.6 / -178.8 | 16.8 / -195.5 | 8.9 / -256.3 |
| silence | 9.2 / -158.8 | 7.6 / -188.2 | 7.6 / -217.2 | 10.0 / -250.9 | 8.5 / -197.7 | 9.4 / -190.2 | 8.9 / -217.0 | 8.7 / -250.3 |
| DC -40 | 8.9 / -188.6 | 11.1 / -216.1 | 10.7 / -230.4 | 8.2 / -255.2 | 7.7 / -200.5 | 8.8 / -183.2 | 8.4 / -215.4 | 9.0 / -258.4 |
| DC -80 | 9.2 / -158.8 | 9.2 / -185.5 | 13.7 / -203.0 | 9.0 / -244.6 | 8.5 / -197.7 | 9.4 / -190.2 | 8.9 / -217.0 | 9.4 / -280.7 |

### Dither (TPDF 0.005) on vs off

Low-level stimuli with and without the production dither. "Noise delta" is the in-band noise with dither minus without at
-6 dBFS (the dither is added at the quantizer, so the loop shapes it like quantization noise). The spur columns give the
highest prominence over the stimuli -120/-100/-60 dBFS 1 kHz, silence and DC -40/-80 dBFS, with the stimulus and frequency.

| Order | Rate | Noise delta @ -6 dBFS (dB) | Worst spur, no dither | Worst spur, TPDF |
|---:|---|---:|---|---|
| 5 | `DSD64` | +0.07 | 15.7 dB (1 kHz -120, 0.78 kHz, -150.4 dBFS) | 16.4 dB (1 kHz -120, 0.77 kHz, -152.1 dBFS) |
| 5 | `DSD128` | +0.33 | 21.2 dB (1 kHz -120, 1.27 kHz, -170.9 dBFS) | 22.7 dB (1 kHz -120, 5.30 kHz, -160.8 dBFS) |
| 5 | `DSD256` | -0.15 | 26.4 dB (1 kHz -120, 0.11 kHz, -210.5 dBFS) | 25.3 dB (1 kHz -120, 0.12 kHz, -210.5 dBFS) |
| 5 | `DSD512` | at floor | 19.6 dB (1 kHz -120, 9.56 kHz, -212.0 dBFS) | 20.7 dB (1 kHz -120, 0.52 kHz, -221.4 dBFS) |
| 5 | `DSD64_48` | +0.14 | 18.3 dB (silence, 7.50 kHz, -343.3 dBFS) | 13.3 dB (1 kHz -120, 0.78 kHz, -155.1 dBFS) |
| 5 | `DSD128_48` | -0.10 | 16.3 dB (DC -40, 17.99 kHz, -345.5 dBFS) | 15.6 dB (1 kHz -100, 2.96 kHz, -171.5 dBFS) |
| 5 | `DSD256_48` | +0.41 | 26.0 dB (1 kHz -120, 0.09 kHz, -215.2 dBFS) | 28.1 dB (1 kHz -120, 0.09 kHz, -216.3 dBFS) |
| 5 | `DSD512_48` | +0.87 | 18.0 dB (DC -40, 19.88 kHz, -224.6 dBFS) | 16.0 dB (1 kHz -100, 9.89 kHz, -227.8 dBFS) |
| 7 | `DSD64` | +0.23 | 9.2 dB (1 kHz -60, 0.12 kHz, -184.5 dBFS) | 9.2 dB (silence, 19.75 kHz, -158.8 dBFS) |
| 7 | `DSD128` | -0.76 | 21.2 dB (1 kHz -120, 6.68 kHz, -181.2 dBFS) | 21.4 dB (1 kHz -120, 5.68 kHz, -178.9 dBFS) |
| 7 | `DSD256` | at floor | 22.6 dB (1 kHz -120, 19.60 kHz, -187.7 dBFS) | 20.0 dB (1 kHz -120, 9.58 kHz, -199.5 dBFS) |
| 7 | `DSD512` | at floor | 15.4 dB (1 kHz -120, 9.58 kHz, -231.7 dBFS) | 14.6 dB (1 kHz -120, 19.60 kHz, -231.5 dBFS) |
| 7 | `DSD64_48` | -0.07 | 11.3 dB (1 kHz -120, 0.04 kHz, -218.2 dBFS) | 10.4 dB (1 kHz -60, 0.06 kHz, -199.3 dBFS) |
| 7 | `DSD128_48` | at floor | 24.5 dB (1 kHz -120, 7.51 kHz, -177.6 dBFS) | 23.6 dB (1 kHz -120, 7.52 kHz, -178.8 dBFS) |
| 7 | `DSD256_48` | at floor | 22.6 dB (1 kHz -120, 12.63 kHz, -185.5 dBFS) | 16.8 dB (1 kHz -120, 1.65 kHz, -195.5 dBFS) |
| 7 | `DSD512_48` | -0.10 | 14.8 dB (DC -40, 0.16 kHz, -289.9 dBFS) | 10.3 dB (1 kHz -100, 7.50 kHz, -259.3 dBFS) |

### DC offsets: DC blocker off vs on (dither on)

A DC offset `u` at the modulator input gives an idle tone near `u * f_bit` that the dither does not remove (F9). The
production chain removes source DC with a first-order 1 Hz high-pass ahead of the cascade (`Design.md` §4.1). "Off" is the
chain without it (before O7). Residual DC is the record's windowed mean (dBFS). Prominence near 8.3 dB (white-noise
median, max 9.1 dB) means no tone above the noise.

| Order | Rate | DC (dBFS) | Off: worst spur (prominence, kHz, dBFS) | Off: residual DC (dBFS) | On: worst spur (prominence, kHz, dBFS) | On: residual DC (dBFS) |
|---:|---|---:|---|---:|---|---:|
| 5 | `DSD64` | -40 | 32.6 dB, 17.31 kHz, -118.9 | -40.0 | 3.1 dB, 16.03 kHz, -325.3 | -345.1 |
| 5 | `DSD64` | -80 | 35.7 dB, 0.16 kHz, -141.3 | -80.0 | 3.1 dB, 16.03 kHz, -325.3 | -345.1 |
| 5 | `DSD128` | -40 | 8.0 dB, 17.95 kHz, -186.5 | -40.0 | 8.4 dB, 18.33 kHz, -188.7 | -277.7 |
| 5 | `DSD128` | -80 | 35.3 dB, 0.36 kHz, -157.8 | -80.0 | 8.6 dB, 0.15 kHz, -200.5 | -267.2 |
| 5 | `DSD256` | -40 | 9.5 dB, 19.77 kHz, -177.0 | -40.0 | 9.4 dB, 10.43 kHz, -210.3 | -307.5 |
| 5 | `DSD256` | -80 | 36.7 dB, 0.83 kHz, -164.4 | -80.0 | 8.3 dB, 0.29 kHz, -215.1 | -295.8 |
| 5 | `DSD512` | -40 | 6.9 dB, 10.12 kHz, -217.6 | -40.0 | 7.9 dB, 19.83 kHz, -215.8 | -295.3 |
| 5 | `DSD512` | -80 | 38.5 dB, 1.74 kHz, -174.9 | -80.0 | 15.8 dB, 0.13 kHz, -258.1 | -288.1 |
| 5 | `DSD64_48` | -40 | 33.2 dB, 18.29 kHz, -133.6 | -40.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD64_48` | -80 | 33.7 dB, 0.18 kHz, -144.1 | -80.0 | 2.5 dB, 6.63 kHz, -330.6 | -345.4 |
| 5 | `DSD128_48` | -40 | 7.7 dB, 18.56 kHz, -175.1 | -40.0 | 9.7 dB, 0.07 kHz, -213.8 | -266.0 |
| 5 | `DSD128_48` | -80 | 37.2 dB, 0.39 kHz, -158.1 | -80.0 | 8.4 dB, 9.56 kHz, -181.6 | -271.9 |
| 5 | `DSD256_48` | -40 | 9.2 dB, 0.14 kHz, -218.7 | -40.0 | 7.6 dB, 0.46 kHz, -212.4 | -279.3 |
| 5 | `DSD256_48` | -80 | 37.3 dB, 0.90 kHz, -167.4 | -80.0 | 8.6 dB, 0.19 kHz, -223.0 | -294.3 |
| 5 | `DSD512_48` | -40 | 11.3 dB, 9.98 kHz, -220.0 | -40.0 | 7.6 dB, 7.48 kHz, -231.3 | -260.9 |
| 5 | `DSD512_48` | -80 | 37.8 dB, 1.92 kHz, -182.2 | -80.0 | 8.3 dB, 0.35 kHz, -256.2 | -281.3 |
| 7 | `DSD64` | -40 | 25.4 dB, 16.33 kHz, -138.1 | -40.0 | 8.9 dB, 0.20 kHz, -188.6 | -256.7 |
| 7 | `DSD64` | -80 | 27.2 dB, 0.16 kHz, -160.9 | -80.0 | 9.2 dB, 19.75 kHz, -158.8 | -244.0 |
| 7 | `DSD128` | -40 | 8.4 dB, 8.28 kHz, -204.2 | -40.0 | 11.1 dB, 0.13 kHz, -216.1 | -264.8 |
| 7 | `DSD128` | -80 | 27.8 dB, 0.40 kHz, -167.6 | -80.0 | 9.2 dB, 17.36 kHz, -185.5 | -261.1 |
| 7 | `DSD256` | -40 | 8.5 dB, 17.32 kHz, -195.9 | -40.0 | 10.7 dB, 0.08 kHz, -230.4 | -264.2 |
| 7 | `DSD256` | -80 | 28.0 dB, 0.84 kHz, -173.2 | -80.0 | 13.7 dB, 19.94 kHz, -203.0 | -275.0 |
| 7 | `DSD512` | -40 | 7.5 dB, 13.70 kHz, -227.1 | -40.0 | 8.2 dB, 14.23 kHz, -255.2 | -304.9 |
| 7 | `DSD512` | -80 | 14.0 dB, 5.25 kHz, -205.5 | -80.0 | 9.0 dB, 18.13 kHz, -244.6 | -305.7 |
| 7 | `DSD64_48` | -40 | 25.2 dB, 17.78 kHz, -141.5 | -40.0 | 7.7 dB, 14.78 kHz, -200.5 | -251.7 |
| 7 | `DSD64_48` | -80 | 28.0 dB, 0.17 kHz, -165.5 | -80.0 | 8.5 dB, 14.75 kHz, -197.7 | -251.3 |
| 7 | `DSD128_48` | -40 | 8.2 dB, 14.21 kHz, -172.5 | -40.0 | 8.8 dB, 9.86 kHz, -183.2 | -265.3 |
| 7 | `DSD128_48` | -80 | 31.2 dB, 0.45 kHz, -156.9 | -80.0 | 9.4 dB, 18.70 kHz, -190.2 | -265.6 |
| 7 | `DSD256_48` | -40 | 8.5 dB, 18.62 kHz, -189.9 | -40.0 | 8.4 dB, 8.53 kHz, -215.4 | -261.6 |
| 7 | `DSD256_48` | -80 | 20.3 dB, 2.86 kHz, -164.5 | -80.0 | 8.9 dB, 14.53 kHz, -217.0 | -254.4 |
| 7 | `DSD512_48` | -40 | 7.7 dB, 7.08 kHz, -242.4 | -40.0 | 9.0 dB, 3.40 kHz, -258.4 | -308.7 |
| 7 | `DSD512_48` | -80 | 16.5 dB, 9.42 kHz, -226.7 | -80.0 | 9.4 dB, 0.28 kHz, -280.7 | -318.5 |

### Two-tone intermodulation (0 dBFS peak, dither on)

IMD = power of the products `m*f1 + n*f2` (both non-zero, `|m| + |n| <= 7`) inside 20 Hz-20 kHz relative to the total tone
power; THD counts the harmonics of either tone. CCIF products: 1 kHz (d2), 18 kHz (d3); 21 kHz and above are out of band.

Tone level error is measured minus requested level of each tone (the cascade is flat to 1e-6 dB, so at 19/20 kHz this
is the modulator's signal transfer function).

| Order | Rate | Stimulus | IMD (dBc) | IMD level (dBFS) | THD (dBc) | Noise (dBFS) | Worst spur (prominence / dBFS) | Tone level error f1 / f2 (dB) | Peak abs state |
|---:|---|---|---:|---:|---:|---:|---|---:|---:|
| 5 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -124.6 | -126.3 | -118.7 | -119.0 | 9.7 / -176.8 | -0.0012 / +0.0013 | 1.070 |
| 5 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -130.2 | -133.2 | — | -116.3 | 9.5 / -162.4 | +0.0098 / +0.0110 | 1.036 |
| 5 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -149.2 | -150.8 | -147.3 | -147.1 | 12.9 / -161.8 | -0.0012 / +0.0003 | 0.913 |
| 5 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -153.9 | -156.9 | — | -145.2 | 11.5 / -193.6 | +0.0024 / +0.0027 | 0.922 |
| 5 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -167.7 | -169.4 | -167.0 | -164.0 | 23.5 / -173.3 | -0.0012 / +0.0001 | 0.652 |
| 5 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -170.7 | -173.8 | — | -164.9 | 23.3 / -175.5 | +0.0006 / +0.0007 | 0.652 |
| 5 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -186.9 | -188.6 | -187.4 | -182.1 | 25.5 / -190.9 | -0.0012 / +0.0000 | 0.476 |
| 5 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -189.7 | -192.8 | — | -184.5 | 25.7 / -195.9 | +0.0002 / +0.0002 | 0.476 |
| 5 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -127.6 | -129.3 | -122.6 | -123.0 | 8.4 / -182.9 | -0.0013 / +0.0011 | 1.015 |
| 5 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -133.7 | -136.7 | — | -120.4 | 9.3 / -141.0 | +0.0082 / +0.0092 | 1.013 |
| 5 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -151.6 | -153.3 | -148.9 | -150.4 | 16.1 / -162.1 | -0.0013 / +0.0003 | 0.864 |
| 5 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -155.4 | -158.4 | — | -147.8 | 13.8 / -161.4 | +0.0021 / +0.0023 | 0.874 |
| 5 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -171.4 | -173.1 | -168.0 | -169.7 | 22.5 / -179.0 | -0.0013 / +0.0001 | 0.652 |
| 5 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -174.8 | -177.8 | — | -168.8 | 21.7 / -178.3 | +0.0005 / +0.0006 | 0.649 |
| 5 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -193.6 | -195.3 | -190.5 | -192.4 | 26.0 / -201.1 | -0.0013 / +0.0000 | 0.532 |
| 5 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -197.6 | -200.6 | — | -192.6 | 24.0 / -200.0 | +0.0001 / +0.0001 | 0.532 |
| 7 | `DSD64` | SMPTE 60 Hz + 7 kHz (4:1) | -149.7 | -151.4 | -137.4 | -137.3 | 7.7 / -171.1 | -0.0012 / +0.0018 | 1.042 |
| 7 | `DSD64` | CCIF 19 kHz + 20 kHz (1:1) | -141.9 | -144.9 | — | -134.8 | 8.4 / -184.6 | +0.0136 / +0.0150 | 1.023 |
| 7 | `DSD128` | SMPTE 60 Hz + 7 kHz (4:1) | -169.0 | -170.7 | -161.8 | -160.5 | 17.6 / -183.2 | -0.0012 / +0.0005 | 0.694 |
| 7 | `DSD128` | CCIF 19 kHz + 20 kHz (1:1) | -160.5 | -163.5 | — | -158.9 | 18.4 / -189.3 | +0.0034 / +0.0038 | 0.697 |
| 7 | `DSD256` | SMPTE 60 Hz + 7 kHz (4:1) | -185.3 | -186.9 | -178.0 | -173.7 | 21.8 / -201.6 | -0.0012 / +0.0001 | 0.417 |
| 7 | `DSD256` | CCIF 19 kHz + 20 kHz (1:1) | -175.3 | -178.3 | — | -173.8 | 23.1 / -202.6 | +0.0009 / +0.0009 | 0.415 |
| 7 | `DSD512` | SMPTE 60 Hz + 7 kHz (4:1) | -216.6 | -218.3 | -209.1 | -203.5 | 26.3 / -215.9 | -0.0012 / +0.0000 | 0.328 |
| 7 | `DSD512` | CCIF 19 kHz + 20 kHz (1:1) | -206.5 | -209.6 | — | -204.5 | 25.2 / -219.8 | +0.0002 / +0.0002 | 0.328 |
| 7 | `DSD64_48` | SMPTE 60 Hz + 7 kHz (4:1) | -155.2 | -156.9 | -142.4 | -143.4 | 8.0 / -167.6 | -0.0013 / +0.0016 | 1.051 |
| 7 | `DSD64_48` | CCIF 19 kHz + 20 kHz (1:1) | -147.5 | -150.5 | — | -140.4 | 13.0 / -203.7 | +0.0115 / +0.0127 | 1.039 |
| 7 | `DSD128_48` | SMPTE 60 Hz + 7 kHz (4:1) | -162.6 | -164.3 | -152.6 | <= -152 | 19.6 / -173.5 | -0.0013 / +0.0004 | 0.550 |
| 7 | `DSD128_48` | CCIF 19 kHz + 20 kHz (1:1) | -153.3 | -156.3 | — | -150.9 | 21.7 / -168.4 | +0.0029 / +0.0031 | 0.550 |
| 7 | `DSD256_48` | SMPTE 60 Hz + 7 kHz (4:1) | -179.7 | -181.4 | -167.9 | -167.3 | 20.5 / -190.8 | -0.0013 / +0.0001 | 0.328 |
| 7 | `DSD256_48` | CCIF 19 kHz + 20 kHz (1:1) | -169.2 | -172.2 | — | -166.1 | 23.8 / -221.8 | +0.0007 / +0.0008 | 0.328 |
| 7 | `DSD512_48` | SMPTE 60 Hz + 7 kHz (4:1) | -230.0 | -231.6 | -220.2 | -218.9 | 24.7 / -241.2 | -0.0013 / +0.0000 | 0.386 |
| 7 | `DSD512_48` | CCIF 19 kHz + 20 kHz (1:1) | -219.4 | -222.4 | — | -218.9 | 23.8 / -247.7 | +0.0002 / +0.0002 | 0.387 |

### Ultrasonic noise (digital domain noise analysis)

Out-of-band power per band, dBFS (PCM full scale). This is mathematical noise analysis of audigma's output per band that would enter the receiving device, not a DAC analog output noise measurement. "dBc @ -6" is relative to a
-6 dBFS tone, for comparison with earlier reports.

| Order | Rate | Silence 20-50 kHz | Silence 50-100 kHz | Silence 20-100 kHz | 0 dBFS 20-50 kHz | 0 dBFS 50-100 kHz | 0 dBFS 20-100 kHz | 20-100 kHz, dBc @ -6 dBFS |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 5 | `DSD64` | -278.1 | -46.0 | -46.0 | -57.1 | -26.2 | -26.2 | -20.6 |
| 5 | `DSD128` | -92.9 | -55.7 | -55.7 | -86.1 | -51.7 | -51.7 | -45.1 |
| 5 | `DSD256` | -116.0 | -75.5 | -75.5 | -108.4 | -73.8 | -73.8 | -67.1 |
| 5 | `DSD512` | -142.4 | -107.0 | -107.0 | -127.3 | -95.7 | -95.7 | -88.2 |
| 5 | `DSD64_48` | -97.4 | -41.5 | -41.5 | -60.5 | -28.3 | -28.3 | -22.4 |
| 5 | `DSD128_48` | -95.9 | -56.7 | -56.7 | -88.5 | -53.9 | -53.9 | -47.8 |
| 5 | `DSD256_48` | -119.6 | -80.8 | -80.8 | -112.4 | -77.3 | -77.3 | -71.0 |
| 5 | `DSD512_48` | -159.0 | -124.9 | -124.9 | -135.2 | -102.9 | -102.9 | -97.7 |
| 7 | `DSD64` | -55.3 | -34.7 | -34.7 | -49.7 | -21.1 | -21.1 | -15.0 |
| 7 | `DSD128` | -83.3 | -49.8 | -49.8 | -74.7 | -32.7 | -32.7 | -25.8 |
| 7 | `DSD256` | -104.0 | -61.4 | -61.4 | -90.9 | -44.0 | -44.0 | -37.9 |
| 7 | `DSD512` | -143.6 | -94.7 | -94.7 | -121.0 | -74.8 | -74.8 | -69.6 |
| 7 | `DSD64_48` | -62.3 | -35.9 | -35.9 | -55.0 | -21.8 | -21.8 | -15.6 |
| 7 | `DSD128_48` | -72.7 | -54.3 | -54.2 | -67.0 | -35.5 | -35.5 | -28.6 |
| 7 | `DSD256_48` | -98.0 | -65.3 | -65.3 | -82.6 | -44.1 | -44.1 | -39.0 |
| 7 | `DSD512_48` | -158.9 | -110.5 | -110.5 | -135.4 | -90.9 | -90.9 | -84.7 |

### Rate family: 44.1 kHz vs 48 kHz

Same DSD multiple, 48 kHz family minus 44.1 kHz family (the 48k family runs at an 8.8% higher bit rate, so a higher OSR).
Positive SNR/DR deltas favour 48k; "at floor" means one side is within 10 dB of the analysis floor.

| Order | DSD multiple | SNR @ 0 dBFS delta (dB) | DR (A) delta (dB) | THD @ 0 dBFS delta (dB) | Ultrasonic 20-100 kHz @ 0 dBFS delta (dB) |
|---:|---|---:|---:|---:|---:|
| 5 | DSD64 | +3.8 | +4.0 | -4.0 | -2.1 |
| 5 | DSD128 | +2.1 | +0.9 | -2.5 | -2.2 |
| 5 | DSD256 | +3.5 | +5.8 | -3.2 | -3.6 |
| 5 | DSD512 | at floor | +10.7 | -6.8 | -7.2 |
| 7 | DSD64 | +5.8 | +5.3 | -4.7 | -0.7 |
| 7 | DSD128 | at floor | -9.3 | +8.2 | -2.7 |
| 7 | DSD256 | at floor | -9.1 | +7.0 | -0.1 |
| 7 | DSD512 | at floor | +14.4 | -13.6 | -16.2 |
