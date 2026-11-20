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
  each (as in the modulator report).
- **Quality:** the production chain records of the signal report (dither on, DC blocker on: level sweep incl. 0 dBFS at
  `M = 0`, silence, DC, SMPTE, CCIF), reduced the same way. The verdict is against the preset's target anchors
  (`Design.md §8.6`): `reference` audibility and 24-bit transparency at every rate, `comfortable` audibility; the fixed
  presets have no target. A **SHORTFALL** is a cell the design marks as missing its target (`meets_target` false in
  `crfb_designs.json`, listed in `Design.md §8.6`); it fails the run if the chain meets the target after all.
- A cell whose coefficients equal an earlier preset's is measured once; its H_inf is marked with the preset it shares.

### `reference`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 118.4 | -130.2 | -116.3 | -117.2 | **SHORTFALL** (DR (A) 118.4 dB < 120; spur -130.2 dBFS > -141; noise -116.3 dBFS > -141; distortion -117.2 dBFS > -141; documented, H_inf 1.40 kept) |
| 5 | `DSD128` | 1.34 | -3.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 140.4 | -159.0 | -145.2 | -141.8 | **PASS** |
| 5 | `DSD256` | 1.22 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 153.0 | -173.3 | -164.0 | -153.1 | **PASS** |
| 5 | `DSD512` | 1.15 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 172.7 | -190.9 | -182.1 | -168.5 | **PASS** |
| 5 | `DSD64_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | **SHORTFALL** (spur -134.7 dBFS > -141; noise -120.2 dBFS > -141; distortion -120.2 dBFS > -141; documented, H_inf 1.40 kept) |
| 5 | `DSD128_48` | 1.32 | -3.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 141.3 | -161.3 | -147.8 | -142.4 | **PASS** |
| 5 | `DSD256_48` | 1.22 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 158.9 | -178.3 | -168.8 | -158.3 | **PASS** |
| 5 | `DSD512_48` | 1.17 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 183.4 | -200.0 | -192.4 | -178.8 | **PASS** |
| 7 | `DSD64` | 1.40 | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 137.3 | -156.6 | -134.8 | -139.0 | **SHORTFALL** (noise -134.8 dBFS > -141; distortion -139.0 dBFS > -141; documented, H_inf 1.40 kept) |
| 7 | `DSD128` | 1.24 | -2.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 151.5 | -174.4 | -158.9 | -152.7 | **PASS** |
| 7 | `DSD256` | 1.13 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 167.9 | -190.6 | -170.4 | -166.5 | **PASS** |
| 7 | `DSD512` | 1.10 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 197.8 | -215.9 | -197.8 | -194.8 | **PASS** |
| 7 | `DSD64_48` | 1.40 | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 142.5 | -166.2 | -140.4 | -144.1 | **SHORTFALL** (noise -140.4 dBFS > -141; documented, H_inf 1.40 kept) |
| 7 | `DSD128_48` | 1.18 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 142.2 | -158.4 | -147.6 | -143.2 | **PASS** |
| 7 | `DSD256_48` | 1.10 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 158.8 | -181.1 | -161.7 | -156.9 | **PASS** |
| 7 | `DSD512_48` | 1.12 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 212.2 | -232.0 | -212.0 | -209.7 | **PASS** |

### `comfortable`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 (= `reference`) | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 118.4 | -130.2 | -116.3 | -117.2 | **SHORTFALL** (DR (A) 118.4 dB < 120; documented, H_inf 1.40 kept) |
| 5 | `DSD128` | 1.22 | -2.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 124.0 | -145.7 | -132.8 | -125.0 | **PASS** |
| 5 | `DSD256` | 1.12 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 132.8 | -155.8 | -144.6 | -131.9 | **PASS** |
| 5 | `DSD512` | 1.15 (= `reference`) | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 172.7 | -190.9 | -182.1 | -168.5 | **PASS** |
| 5 | `DSD64_48` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | **PASS** |
| 5 | `DSD128_48` | 1.18 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 120.0 | -143.9 | -130.9 | -120.9 | **PASS** |
| 5 | `DSD256_48` | 1.12 | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 138.0 | -162.5 | -149.5 | -136.9 | **PASS** |
| 5 | `DSD512_48` | 1.17 (= `reference`) | -1.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 183.4 | -200.0 | -192.4 | -178.8 | **PASS** |
| 7 | `DSD64` | 1.29 | -3.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 121.9 | -138.7 | -122.0 | -119.4 | **PASS** |
| 7 | `DSD128` | 1.13 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 118.9 | -144.7 | -128.2 | -120.0 | **FAIL** (DR (A) 118.9 dB < 120) |
| 7 | `DSD256` | 1.13 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 167.9 | -190.6 | -170.4 | -166.5 | **PASS** |
| 7 | `DSD512` | 1.10 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 197.8 | -215.9 | -197.8 | -194.8 | **PASS** |
| 7 | `DSD64_48` | 1.27 | -2.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 122.9 | -141.7 | -124.7 | -123.4 | **PASS** |
| 7 | `DSD128_48` | 1.13 | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 124.9 | -149.2 | -133.5 | -125.8 | **PASS** |
| 7 | `DSD256_48` | 1.10 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 158.8 | -181.1 | -161.7 | -156.9 | **PASS** |
| 7 | `DSD512_48` | 1.12 (= `reference`) | -2.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 212.2 | -232.0 | -212.0 | -209.7 | **PASS** |

### `fixed_h_inf_1_4`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.40 (= `reference`) | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 118.4 | -130.2 | -116.3 | -117.2 | — |
| 5 | `DSD128` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 147.6 | -161.7 | -149.0 | -149.4 | — |
| 5 | `DSD256` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 175.6 | -198.0 | -181.8 | -176.1 | — |
| 5 | `DSD512` | 1.40 | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 210.7 | -229.6 | -215.7 | -208.8 | — |
| 5 | `DSD64_48` | 1.40 (= `reference`) | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 122.4 | -134.7 | -120.2 | -120.2 | — |
| 5 | `DSD128_48` | 1.40 | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 150.7 | -171.2 | -153.6 | -152.2 | — |
| 5 | `DSD256_48` | 1.40 | -4.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 179.5 | -201.9 | -186.4 | -179.6 | — |
| 5 | `DSD512_48` | 1.40 | -4.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 212.4 | -227.3 | -220.1 | -210.4 | — |
| 7 | `DSD64` | 1.40 (= `reference`) | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 137.3 | -156.6 | -134.8 | -139.0 | — |
| 7 | `DSD128` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 179.9 | -193.6 | -179.5 | -182.7 | — |
| 7 | `DSD256` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 221.9 | -243.2 | -224.5 | -222.4 | — |
| 7 | `DSD512` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 267.7 | -262.6 | -243.8 | -265.4 | — |
| 7 | `DSD64_48` | 1.40 (= `reference`) | -4.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 142.5 | -166.2 | -140.4 | -144.1 | — |
| 7 | `DSD128_48` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 184.0 | -201.6 | -185.3 | -186.1 | — |
| 7 | `DSD256_48` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 226.0 | -247.5 | -230.2 | -226.6 | — |
| 7 | `DSD512_48` | 1.40 | -4.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | >= 273 | -262.8 | -243.4 | -267.9 | — |

### `fixed_h_inf_1_5`

| Order | Rate | H_inf | L (dBFS) | Bit-exact | Overloads at L (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Chain overloads | DR (A) (dB) | Worst spur (dBFS) | Max noise (dBFS) | Max harmonic / IMD level (dBFS) | Verdict |
|---:|---|---:|---:|:---:|---|---:|---:|---:|---:|---:|---|
| 5 | `DSD64` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 124.8 | -141.2 | -120.9 | -124.8 | — |
| 5 | `DSD128` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 157.5 | -171.8 | -154.1 | -157.6 | — |
| 5 | `DSD256` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 187.5 | -197.3 | -187.3 | -189.4 | — |
| 5 | `DSD512` | 1.50 | -6.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 220.6 | -235.0 | -220.1 | -220.3 | — |
| 5 | `DSD64_48` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 128.8 | -140.4 | -125.0 | -128.7 | — |
| 5 | `DSD128_48` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 161.1 | -174.1 | -157.9 | -161.4 | — |
| 5 | `DSD256_48` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 192.0 | -202.4 | -191.2 | -193.6 | — |
| 5 | `DSD512_48` | 1.50 | -5.75 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 223.9 | -233.4 | -224.3 | -223.6 | — |
| 7 | `DSD64` | 1.50 | -6.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 146.6 | -168.1 | -142.0 | -146.4 | — |
| 7 | `DSD128` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 190.8 | -200.4 | -186.9 | -190.9 | — |
| 7 | `DSD256` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 235.3 | -245.1 | -231.8 | -235.8 | — |
| 7 | `DSD512` | 1.50 | -7.25 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | >= 276 | -262.6 | -243.8 | -266.0 | — |
| 7 | `DSD64_48` | 1.50 | -6.50 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 152.3 | -164.7 | -147.5 | -151.0 | — |
| 7 | `DSD128_48` | 1.50 | -6.75 | yes | 1, 0, 0, 0, 0, 0, 0 | 0 | 196.5 | -204.8 | -192.4 | -196.3 | — |
| 7 | `DSD256_48` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | 241.1 | -247.3 | -236.6 | -240.8 | — |
| 7 | `DSD512_48` | 1.50 | -7.00 | yes | 0, 0, 0, 0, 0, 0, 0 | 0 | >= 276 | -262.8 | -243.4 | -268.2 | — |

Levels within 10 dB of the analysis floor are printed as bounds. Tone levels up to 10 kHz are
checked as in the signal report (a level error fails the run).
