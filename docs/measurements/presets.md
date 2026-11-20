# audigma — NTF presets Measurements

> **Specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [README.md](README.md)

`[resampler] modulator_ntf` presets: per preset, order and rate, bit-exact against the pinned design, stability at L, quality against the preset's target.

---

## NTF presets

Each `[resampler] modulator_ntf` preset (`Design.md §4.3`) for every order and rate, measured on the compiled code with the
table it selects (harness `--ntf`). Per cell:

- **Bit-exact:** a direct sine (no dither, -6 dBFS or 1 dB below `L` where that is lower) equals, bit for bit,
  the Python reference loop with the pinned coefficients of that preset (`crfb_designs.json`).
- **Overloads at L:** the production modulator (TPDF dither) at exactly the table's `L`, DC and 50 Hz-20 kHz sines, 2^21 bits
  each (as in the modulator report), with 9 dither streams (seeds 12345, 1, 2, 3, 4, 5, 6, 7, 8; 12345 is the
  production default); per stimulus, the overloads summed over the streams. Any overload fails the run.
- **Above L:** the same check 0.25 dB above `L` (one guard step), reported only: the streams that overload
  there and at which stimuli. Overloads here mean `L` has no measured headroom in that cell.
- **Quality:** the production chain records of the signal report (dither on, DC blocker on: level sweep incl. 0 dBFS at
  `M = 0`, silence, DC, SMPTE, CCIF), reduced the same way. The verdict is against the preset's target anchors
  (`Design.md §8.6`): `reference` audibility and 24-bit transparency at every rate, `comfortable` audibility; the fixed
  presets have no target. A **SHORTFALL** is a cell the design marks as missing its target (`meets_target` false in
  `crfb_designs.json`: no H_inf meets it throughout its window; listed in `Design.md §8.6`); it keeps H_inf 1.40 and is
  reported, not failed.
- A cell whose coefficients equal an earlier preset's is measured once; its H_inf is marked with the preset it shares.

### `reference`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Streams overloading at L + 0.25 | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 118.1 | -135.9 | -116.1 | -116.4 | **SHORTFALL** (DR (A) 118.1 dB < 120; spur -135.9 dBFS > -141; noise -116.1 dBFS > -141; distortion -116.4 dBFS > -141; documented, H_inf 1.40 kept) |
| 5 | `DSD128` | 1.35 | -3.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 141.8 | -162.3 | -146.1 | -143.2 | **PASS** |
| 5 | `DSD256` | 1.21 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 152.3 | -173.5 | -163.0 | -152.0 | **PASS** |
| 5 | `DSD512` | 1.16 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 175.1 | -193.0 | -184.3 | -170.9 | **PASS** |
| 5 | `DSD64_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | **SHORTFALL** (spur -134.7 dBFS > -141; noise -120.2 dBFS > -141; distortion -120.2 dBFS > -141; documented, H_inf 1.40 kept) |
| 5 | `DSD128_48` | 1.33 | -3.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 142.6 | -161.5 | -148.6 | -143.7 | **PASS** |
| 5 | `DSD256_48` | 1.21 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 157.7 | -177.8 | -167.7 | -157.1 | **PASS** |
| 5 | `DSD512_48` | 1.18 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 185.5 | -203.0 | -194.3 | -180.9 | **PASS** |
| 7 | `DSD64` | 1.40 | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 137.3 | -156.6 | -134.8 | -139.0 | **SHORTFALL** (noise -134.8 dBFS > -141; distortion -139.0 dBFS > -141; documented, H_inf 1.40 kept) |
| 7 | `DSD128` | 1.25 | -2.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 153.6 | -173.5 | -160.6 | -154.8 | **PASS** |
| 7 | `DSD256` | 1.14 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 171.4 | -187.5 | -174.4 | -171.0 | **PASS** |
| 7 | `DSD512` | 1.11 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 202.0 | -216.9 | -202.6 | -199.9 | **PASS** |
| 7 | `DSD64_48` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 141.9 | -152.7 | -139.9 | -143.8 | **SHORTFALL** (noise -139.9 dBFS > -141; documented, H_inf 1.40 kept) |
| 7 | `DSD128_48` | 1.19 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | **4/9** (DC) | 0 | 144.9 | -165.5 | -151.0 | -145.9 | **PASS** |
| 7 | `DSD256_48` | 1.11 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 163.8 | -182.0 | -166.2 | -161.8 | **PASS** |
| 7 | `DSD512_48` | 1.13 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | **1/9** (DC) | 0 | 215.2 | -234.4 | -216.7 | -214.3 | **PASS** |

### `comfortable`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Streams overloading at L + 0.25 | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 118.1 | -135.9 | -116.1 | -116.4 | **SHORTFALL** (DR (A) 118.1 dB < 120; documented, H_inf 1.40 kept) |
| 5 | `DSD128` | 1.23 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 125.6 | -146.2 | -134.1 | -126.6 | **PASS** |
| 5 | `DSD256` | 1.13 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 135.7 | -157.4 | -147.2 | -134.9 | **PASS** |
| 5 | `DSD512` | 1.16 (= `reference`) | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 175.1 | -193.0 | -184.3 | -170.9 | **PASS** |
| 5 | `DSD64_48` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | **PASS** |
| 5 | `DSD128_48` | 1.19 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 122.0 | -144.3 | -132.7 | -122.9 | **PASS** |
| 5 | `DSD256_48` | 1.13 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 141.0 | -163.2 | -152.2 | -140.0 | **PASS** |
| 5 | `DSD512_48` | 1.18 (= `reference`) | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 185.5 | -203.0 | -194.3 | -180.9 | **PASS** |
| 7 | `DSD64` | 1.30 | -3.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 123.7 | -140.0 | -123.9 | -121.5 | **PASS** |
| 7 | `DSD128` | 1.14 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 123.2 | -145.0 | -130.6 | -124.2 | **PASS** |
| 7 | `DSD256` | 1.14 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 171.4 | -187.5 | -174.4 | -171.0 | **PASS** |
| 7 | `DSD512` | 1.11 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 202.0 | -216.9 | -202.6 | -199.9 | **PASS** |
| 7 | `DSD64_48` | 1.28 | -2.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 124.9 | -141.5 | -126.2 | -125.7 | **PASS** |
| 7 | `DSD128_48` | 1.14 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 129.1 | -154.2 | -136.5 | -130.0 | **PASS** |
| 7 | `DSD256_48` | 1.11 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 163.8 | -182.0 | -166.2 | -161.8 | **PASS** |
| 7 | `DSD512_48` | 1.13 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | **1/9** (DC) | 0 | 215.2 | -234.4 | -216.7 | -214.3 | **PASS** |

### `fixed_h_inf_1_4`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Streams overloading at L + 0.25 | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 118.1 | -135.9 | -116.1 | -116.4 | — |
| 5 | `DSD128` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 147.6 | -161.7 | -149.0 | -149.4 | — |
| 5 | `DSD256` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 175.6 | -198.0 | -181.8 | -176.1 | — |
| 5 | `DSD512` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 208.9 | -225.7 | -215.7 | -207.8 | — |
| 5 | `DSD64_48` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | — |
| 5 | `DSD128_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 150.7 | -171.6 | -153.3 | -152.2 | — |
| 5 | `DSD256_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 179.5 | -201.9 | -186.4 | -179.6 | — |
| 5 | `DSD512_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 214.0 | -228.4 | -219.6 | -211.4 | — |
| 7 | `DSD64` | 1.40 (= `reference`) | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 137.3 | -156.6 | -134.8 | -139.0 | — |
| 7 | `DSD128` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 180.0 | -194.0 | -178.9 | -182.7 | — |
| 7 | `DSD256` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 221.8 | -243.0 | -224.4 | -222.5 | — |
| 7 | `DSD512` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 267.3 | -262.6 | -243.8 | -265.3 | — |
| 7 | `DSD64_48` | 1.40 (= `reference`) | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 141.9 | -152.7 | -139.9 | -143.8 | — |
| 7 | `DSD128_48` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | **2/9** (DC) | 0 | 184.0 | -201.6 | -185.3 | -186.1 | — |
| 7 | `DSD256_48` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 225.8 | -243.7 | -230.2 | -226.3 | — |
| 7 | `DSD512_48` | 1.40 | -5.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 270.5 | -262.8 | -243.4 | -267.8 | — |

### `fixed_h_inf_1_5`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Streams overloading at L + 0.25 | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 124.8 | -141.3 | -120.2 | -124.0 | — |
| 5 | `DSD128` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 157.2 | -166.9 | -153.4 | -157.2 | — |
| 5 | `DSD256` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 187.6 | -196.7 | -186.9 | -190.0 | — |
| 5 | `DSD512` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 220.6 | -235.0 | -220.1 | -220.3 | — |
| 5 | `DSD64_48` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 128.7 | -142.7 | -124.7 | -128.1 | — |
| 5 | `DSD128_48` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 161.1 | -171.8 | -157.7 | -161.2 | — |
| 5 | `DSD256_48` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 192.0 | -202.3 | -191.1 | -193.6 | — |
| 5 | `DSD512_48` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 224.3 | -242.6 | -224.1 | -224.6 | — |
| 7 | `DSD64` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 146.3 | -153.5 | -141.7 | -145.8 | — |
| 7 | `DSD128` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 190.2 | -203.6 | -186.3 | -190.3 | — |
| 7 | `DSD256` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 235.0 | -243.7 | -231.4 | -235.4 | — |
| 7 | `DSD512` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | >= 276 | -262.6 | -243.8 | -266.0 | — |
| 7 | `DSD64_48` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 151.7 | -162.9 | -147.0 | -150.8 | — |
| 7 | `DSD128_48` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 195.8 | -204.4 | -192.2 | -195.1 | — |
| 7 | `DSD256_48` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0/9 | 0 | 240.7 | -248.7 | -236.4 | -240.6 | — |
| 7 | `DSD512_48` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | **1/9** (DC) | 0 | >= 276 | -262.8 | -243.4 | -268.2 | — |

Levels within 10 dB of the analysis floor are printed as bounds. Tone levels up to 10 kHz are
checked as in the signal report (a level error fails the run).
