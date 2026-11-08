# audigma — Cascade Measurements

> **Generated:** `2026-11-08 11:16:15Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [report.md](report.md)

Half-band interpolation cascade as implemented (impulse response, tones) and cascade + modulator end to end.

---

## Implemented half-band cascade (impulse response)

Measured through `HalfBandCascade` (harness mode `cascade-impulse`): a unit impulse at the 44.1 kHz baseband rate, response
FFT-ed with 512 baseband frames (zero padded x8). The cascade is linear and time-invariant from the baseband
input, so this response describes it completely. No pass/fail thresholds are applied here; values are reported for audit
step 5. "Image rejection" is the worst level inside the bands `k*44.1 kHz +- 20 kHz` (k = 1, 2, ...) that must be removed;
"droop" is the gain at that frequency relative to DC. Every filter is designed for >= 140 dB (`design_halfband.py`); apodization
is `off` (half-band, stopband 24.1 kHz) or the pass edge of an apodizing stage 0 whose stopband is 22.05 kHz.

### Stage designs (`design_halfband.py`)

Interpolation stage k outputs at 88.2 kHz x 2^k; pass 20 kHz, stop at the mirror `fs/2 - 20 kHz`. The decimator runs the same
designs from its output backwards (its last stage is stage 0).

| Stage | Output rate | Taps | Stopband from | Design rejection, linear (dB) | Design rejection, minimum (dB) |
|---:|---:|---:|---:|---:|---:|
| 0 | 88.2 kHz | 199 | 24.1 kHz | 145.7 | 145.7 |
| 1 | 176.4 kHz | 31 | 68.2 kHz | 150.9 | 150.9 |
| 2 | 352.8 kHz | 19 | 156.4 kHz | 160.1 | 160.1 |
| 3 | 705.6 kHz | 15 | 332.8 kHz | 177.8 | 177.8 |
| 4 | 1,411.2 kHz | 11 | 685.6 kHz | 170.3 | 170.3 |
| 5 | 2,822.4 kHz | 11 | 1,391.2 kHz | 195.6 | 195.6 |
| 6 | 5,644.8 kHz | 7 | 2,802.4 kHz | 162.0 | 162.0 |
| 7 | 11,289.6 kHz | 7 | 5,624.8 kHz | 186.1 | 186.1 |
| 8 | 22,579.2 kHz | 7 | 11,269.6 kHz | 210.2 | 210.2 |

| Apodizing stage 0, pass edge | Taps | Design rejection >= 22.05 kHz, linear (dB) | minimum (dB) |
|---:|---:|---:|---:|
| 18000 Hz | 209 | 143.0 | 143.0 |
| 18500 Hz | 237 | 143.0 | 143.0 |
| 19000 Hz | 277 | 143.0 | 143.0 |
| 20000 Hz | 409 | 141.8 | 141.8 |

### Every stage against the design

The design cascade is rebuilt in Python from the `design_halfband.py` taps of each stage (zero-stuff by 2, filter with 2h) and
compared sample by sample with the measured impulse response, after the best integer alignment. Deviation is relative to the
response peak; tolerance 1e-12.

| Phase | Apod. | K = 1 | K = 2 | K = 3 | K = 4 | K = 6 | K = 9 | Match |
|---|---|---:|---:|---:|---:|---:|---:|:---:|
| linear | off | 0.0e+00 | 2.2e-16 | 2.2e-16 | 3.3e-16 | 3.3e-16 | 5.6e-16 | yes |
| linear | 18000 Hz | 0.0e+00 | 2.4e-16 | 3.7e-16 | 3.7e-16 | 4.9e-16 | 6.1e-16 | yes |
| linear | 18500 Hz | 0.0e+00 | 1.2e-16 | 1.2e-16 | 1.8e-16 | 2.4e-16 | 4.8e-16 | yes |
| linear | 19000 Hz | 0.0e+00 | 3.6e-16 | 3.6e-16 | 3.6e-16 | 6.0e-16 | 7.2e-16 | yes |
| linear | 20000 Hz | 0.0e+00 | 2.3e-16 | 3.5e-16 | 3.5e-16 | 4.7e-16 | 8.2e-16 | yes |
| minimum | off | 0.0e+00 | 1.6e-16 | 3.2e-16 | 4.8e-16 | 8.0e-16 | 9.6e-16 | yes |
| minimum | 18000 Hz | 0.0e+00 | 1.8e-16 | 2.7e-16 | 4.5e-16 | 7.1e-16 | 7.1e-16 | yes |
| minimum | 18500 Hz | 0.0e+00 | 1.8e-16 | 3.6e-16 | 5.3e-16 | 5.3e-16 | 1.1e-15 | yes |
| minimum | 19000 Hz | 0.0e+00 | 3.6e-16 | 3.5e-16 | 3.5e-16 | 7.1e-16 | 8.9e-16 | yes |
| minimum | 20000 Hz | 0.0e+00 | 3.5e-16 | 3.5e-16 | 3.5e-16 | 5.3e-16 | 8.8e-16 | yes |

### Stage 0 (K = 1) against the design

| Phase | Apod. | Design ripple (dB) | Measured ripple (dB) | Design rejection (dB) | Measured rejection (dB) | Match |
|---|---|---:|---:|---:|---:|:---:|
| linear | off | 4.50e-07 | 4.47e-07 | 145.7 | 145.8 | yes |
| linear | 18000 Hz | 6.37e-07 | 6.37e-07 | 143.0 | 143.0 | yes |
| linear | 18500 Hz | 6.25e-07 | 6.23e-07 | 143.0 | 143.2 | yes |
| linear | 19000 Hz | 6.17e-07 | 6.04e-07 | 143.0 | 143.2 | yes |
| linear | 20000 Hz | 7.39e-07 | 5.83e-07 | 141.8 | 141.8 | yes |
| minimum | off | 1.86e-06 | 1.79e-06 | 145.7 | 145.8 | yes |
| minimum | 18000 Hz | 6.38e-07 | 6.38e-07 | 143.0 | 143.0 | yes |
| minimum | 18500 Hz | 2.64e-06 | 2.63e-06 | 143.0 | 143.2 | yes |
| minimum | 19000 Hz | 1.43e-06 | 1.43e-06 | 143.0 | 143.1 | yes |
| minimum | 20000 Hz | 2.51e-06 | 2.31e-06 | 141.8 | 141.8 | yes |

Bands: `off` passband 0-20 kHz, stopband 24.1 kHz-Nyquist; apodizing passband 0-edge, stopband 22.05 kHz-Nyquist (the design's own definition). Match = rejection within 1.0 dB and ripple within 0.0001 dB of the design. The tails of all impulse records are below 1e-12 of the energy, so 512 frames capture the full response.

### Full cascade at DSD64 (K = 6, 64x), all filter settings

| Phase | Apod. | Ripple 0-20 kHz (dB) | Gain @ 17.5 kHz (dB) | Gain @ 20 kHz (dB) | Gain @ 22.05 kHz (dB) | Image rejection, band 1 (dB) | Worst image rejection (dB) | Group delay @ 1 kHz (ms) | @ 20 kHz (ms) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| linear | off | 0.000001 | -0.000 | -0.000 | -6.0 | 145.8 | 145.8 | 1.271 | 1.271 |
| linear | 18000 Hz | 5.616201 | -0.000 | -5.698 | -160.3 | 149.8 | 149.8 | 1.316 | 1.316 |
| linear | 18500 Hz | 2.747538 | -0.000 | -2.803 | -145.3 | 149.9 | 149.9 | 1.475 | 1.475 |
| linear | 19000 Hz | 0.785499 | -0.000 | -0.812 | -172.0 | 151.6 | 151.6 | 1.701 | 1.701 |
| linear | 20000 Hz | 0.000001 | -0.000 | -0.000 | -141.8 | 154.4 | 152.1 | 2.450 | 2.450 |
| minimum | off | 0.000002 | -0.000 | -0.000 | -6.0 | 145.8 | 145.8 | 0.086 | 0.298 |
| minimum | 18000 Hz | 5.616201 | -0.000 | -5.698 | -160.2 | 149.8 | 149.8 | 0.100 | 0.703 |
| minimum | 18500 Hz | 2.747541 | -0.000 | -2.803 | -145.3 | 149.9 | 149.9 | 0.100 | 0.719 |
| minimum | 19000 Hz | 0.785502 | -0.000 | -0.812 | -170.8 | 151.6 | 151.6 | 0.100 | 0.708 |
| minimum | 20000 Hz | 0.000002 | -0.000 | -0.000 | -141.8 | 154.4 | 152.1 | 0.100 | 0.589 |

### Rate family: 44.1 kHz vs 48 kHz sources (K = 6, DSD64 / DSD64_48)

The cascade uses the same normalised filters for both families, so a 48 kHz source sees every frequency scaled by 48/44.1
(apodizing pass edge 19 kHz -> 20.7 kHz). The audio band stays 20 kHz, so the 48k family has more passband margin and a wider
first image gap (28 kHz instead of 24.1 kHz). Image bands: `k*fs +- 20 kHz`.

| Phase | Apod. | Gain @ 20 kHz, 44.1k / 48k (dB) | Ripple 0-20 kHz, 44.1k / 48k (dB) | Worst image rejection, 44.1k / 48k (dB) | Group delay @ 1 kHz, 44.1k / 48k (ms) |
|---|---|---:|---:|---:|---:|
| linear | off | -0.000 / 0.000 | 7.54e-07 / 7.54e-07 | 145.8 / 145.8 | 1.271 / 1.167 |
| linear | 18000 Hz | -5.698 / -0.002 | 5.62e+00 / 1.63e-03 | 149.8 / 152.4 | 1.316 / 1.209 |
| linear | 18500 Hz | -2.803 / -0.000 | 2.75e+00 / 6.83e-07 | 149.9 / 152.4 | 1.475 / 1.355 |
| linear | 19000 Hz | -0.812 / 0.000 | 7.85e-01 / 5.07e-07 | 151.6 / 152.4 | 1.701 / 1.563 |
| linear | 20000 Hz | -0.000 / -0.000 | 7.13e-07 / 4.12e-07 | 152.1 / 152.4 | 2.450 / 2.251 |
| minimum | off | -0.000 / -0.000 | 1.95e-06 / 1.51e-06 | 145.8 / 145.8 | 0.086 / 0.079 |
| minimum | 18000 Hz | -5.698 / -0.002 | 5.62e+00 / 1.63e-03 | 149.8 / 152.4 | 0.100 / 0.092 |
| minimum | 18500 Hz | -2.803 / -0.000 | 2.75e+00 / 2.82e-06 | 149.9 / 152.4 | 0.100 / 0.092 |
| minimum | 19000 Hz | -0.812 / -0.000 | 7.86e-01 / 9.97e-07 | 151.6 / 152.4 | 0.100 / 0.091 |
| minimum | 20000 Hz | -0.000 / -0.000 | 2.48e-06 / 1.51e-06 | 152.1 / 152.4 | 0.100 / 0.092 |

Worst image rejection over all 60 combinations, 48k family: **145.8 dB** (44.1k family: 145.8 dB).

### Stage count (K) scaling, `linear` / `off`

K = 1..4 are the PCM tiers (88.2k..705.6k), K = 6 is DSD64 and K = 9 is DSD512.

| K | Output rate | DC gain (dB) | Ripple 0-20 kHz (dB) | Worst image rejection (dB) | Image rejection per band 1, 2, 3 (dB) | Group delay @ 1 kHz (ms) |
|---:|---:|---:|---:|---:|---|---:|
| 1 | 88.2 kHz | 1.93e-15 | 4.47e-07 | 145.8 | 145.8 | 1.134 |
| 2 | 176.4 kHz | 0.00e+00 | 6.88e-07 | 145.8 | 145.8, 150.9 | 1.224 |
| 3 | 352.8 kHz | 0.00e+00 | 7.42e-07 | 145.8 | 145.8, 152.1, 199.7 | 1.253 |
| 4 | 705.6 kHz | 1.93e-15 | 7.45e-07 | 145.8 | 145.8, 152.1, 200.7 | 1.264 |
| 6 | 2,822.4 kHz | 0.00e+00 | 7.54e-07 | 145.8 | 145.8, 152.1, 200.8 | 1.271 |
| 9 | 22,579.2 kHz | 0.00e+00 | 7.85e-07 | 145.8 | 145.8, 152.1, 200.8 | 1.272 |

Worst image rejection over all 60 combinations (2 phases x 5 apodizations x K in [1, 2, 3, 4, 6, 9]): **145.8 dB**; largest DC-gain deviation 5.79e-15 dB.

---

## Cascade tone cross-check

Coherent tones (1, 10, 18, 20, 21, 22 kHz, -6 dBFS) pushed through the same cascade (harness mode `cascade-tone`, 4096 baseband
frames, Nuttall window). "Predicted" is `|H(f)|` of the impulse response above evaluated at the exact tone frequency. This validates the
impulse method (tile boundaries, state handling, FTZ/DAZ); the spur column is the strongest component outside the tone, so it is the
actual image/alias level seen with a real signal.

| Phase | Apod. | K | Max measured-predicted gain difference (dB) | Gain @ 20 kHz measured (dB) | Gain @ 22 kHz measured (dB) | Worst spur, tones <= 20 kHz (dBc) | Spur frequency (kHz) | Agrees |
|---|---|---:|---:|---:|---:|---:|---:|:---:|
| linear | off | 1 | 2.24e-09 | -0.0000 | -5.349 | -142.1 | 24.08 | yes |
| linear | off | 6 | 2.24e-09 | -0.0000 | -5.349 | -142.1 | 24.08 | yes |
| linear | 18000 Hz | 1 | 5.97e-08 | -5.7526 | -123.116 | -147.9 | 24.08 | yes |
| linear | 18000 Hz | 6 | 5.84e-08 | -5.7526 | -123.116 | -147.9 | 24.08 | yes |
| linear | 18500 Hz | 1 | 5.60e-08 | -2.8413 | -117.871 | -147.0 | 24.08 | yes |
| linear | 18500 Hz | 6 | 5.67e-08 | -2.8413 | -117.871 | -147.0 | 24.08 | yes |
| linear | 19000 Hz | 1 | 6.60e-08 | -0.8292 | -117.674 | -150.5 | 24.08 | yes |
| linear | 19000 Hz | 6 | 6.49e-08 | -0.8292 | -117.674 | -150.5 | 24.08 | yes |
| linear | 20000 Hz | 1 | 4.21e-08 | -0.0000 | -107.223 | -158.9 | 26.09 | yes |
| linear | 20000 Hz | 6 | 7.01e-08 | -0.0000 | -107.223 | -151.8 | 68.18 | yes |
| minimum | off | 1 | 2.24e-09 | -0.0000 | -5.349 | -142.1 | 24.08 | yes |
| minimum | off | 6 | 2.24e-09 | -0.0000 | -5.349 | -142.1 | 24.08 | yes |
| minimum | 18000 Hz | 1 | 5.91e-08 | -5.7526 | -123.115 | -147.9 | 24.08 | yes |
| minimum | 18000 Hz | 6 | 5.87e-08 | -5.7526 | -123.116 | -147.9 | 24.08 | yes |
| minimum | 18500 Hz | 1 | 5.57e-08 | -2.8413 | -117.871 | -147.0 | 24.08 | yes |
| minimum | 18500 Hz | 6 | 5.68e-08 | -2.8413 | -117.871 | -147.0 | 24.08 | yes |
| minimum | 19000 Hz | 1 | 6.54e-08 | -0.8292 | -117.676 | -150.5 | 24.08 | yes |
| minimum | 19000 Hz | 6 | 6.44e-08 | -0.8292 | -117.676 | -150.5 | 24.08 | yes |
| minimum | 20000 Hz | 1 | 7.06e-08 | -0.0000 | -107.224 | -158.9 | 26.09 | yes |
| minimum | 20000 Hz | 6 | 7.07e-08 | -0.0000 | -107.224 | -151.8 | 68.18 | yes |

Agreement tolerance: 0.001 dB.

---

## End-to-End Cascade + Modulator Verification

Test conditions: Baseband PCM 1 kHz coherent tone at -6.0 dBFS upsampled via `HalfBandCascade` (2^K interpolation; linear or minimum phase, apodization `off` or pass edge 20 kHz) directly into `Modulator`.

### 5th-Order Modulator Pipeline

| Target Rate | Phase | Apodization | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Verdict |
|---|---|---|:---:|---:|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `linear` | `off` | 6 | **115.14 dB** | 133 dB | 114.52 | -123.25 | -24.93 dB | -131.6 | 0.9759 | **PASS** |
| `DSD64` | `linear` | `20000` | 6 | **115.23 dB** | 133 dB | 114.59 | -123.24 | -24.78 dB | -130.9 | 0.9792 | **PASS** |
| `DSD64` | `minimum` | `off` | 6 | **115.20 dB** | 133 dB | 114.46 | -122.50 | -25.03 dB | -132.6 | 0.9775 | **PASS** |
| `DSD64` | `minimum` | `20000` | 6 | **115.04 dB** | 133 dB | 114.42 | -123.21 | -24.90 dB | -130.6 | 0.9799 | **PASS** |
| `DSD128` | `linear` | `off` | 7 | **148.21 dB** | 171 dB | 147.15 | -153.78 | -53.56 dB | -161.1 | 0.9798 | **PASS** |
| `DSD128` | `linear` | `20000` | 7 | **148.03 dB** | 171 dB | 146.90 | -153.29 | -53.45 dB | -160.8 | 0.9864 | **PASS** |
| `DSD128` | `minimum` | `off` | 7 | **148.56 dB** | 171 dB | 147.46 | -153.97 | -53.74 dB | -163.0 | 0.9827 | **PASS** |
| `DSD128` | `minimum` | `20000` | 7 | **148.24 dB** | 171 dB | 147.24 | -154.13 | -53.68 dB | -161.4 | 0.9770 | **PASS** |
| `DSD256` | `linear` | `off` | 8 | **181.59 dB** | 200 dB | 179.77 | -184.44 | -86.88 dB | -194.5 | 0.9842 | **PASS** |
| `DSD256` | `linear` | `20000` | 8 | **181.72 dB** | 200 dB | 179.62 | -183.77 | -86.42 dB | -192.2 | 0.9774 | **PASS** |
| `DSD256` | `minimum` | `off` | 8 | **181.55 dB** | 200 dB | 179.61 | -184.04 | -87.09 dB | -193.3 | 0.9773 | **PASS** |
| `DSD256` | `minimum` | `20000` | 8 | **181.16 dB** | 200 dB | 179.34 | -183.98 | -86.62 dB | -191.7 | 0.9805 | **PASS** |
| `DSD512` | `linear` | `off` | 9 | **214.90 dB** | 235 dB | 211.31 | -213.81 | -119.85 dB | -227.2 | 0.9778 | **PASS** |
| `DSD512` | `linear` | `20000` | 9 | **215.54 dB** | 235 dB | 211.53 | -213.74 | -120.17 dB | -225.9 | 0.9813 | **PASS** |
| `DSD512` | `minimum` | `off` | 9 | **215.59 dB** | 235 dB | 211.70 | -213.97 | -120.51 dB | -224.8 | 0.9869 | **PASS** |
| `DSD512` | `minimum` | `20000` | 9 | **215.11 dB** | 235 dB | 211.37 | -213.75 | -120.03 dB | -226.8 | 0.9834 | **PASS** |

### 7th-Order Modulator Pipeline

| Target Rate | Phase | Apodization | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Verdict |
|---|---|---|:---:|---:|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `linear` | `off` | 6 | **134.00 dB** | 150 dB | 133.38 | -142.11 | -19.92 dB | -150.6 | 0.9874 | **PASS** |
| `DSD64` | `linear` | `20000` | 6 | **133.85 dB** | 150 dB | 133.21 | -141.83 | -19.90 dB | -147.6 | 0.9824 | **PASS** |
| `DSD64` | `minimum` | `off` | 6 | **133.86 dB** | 150 dB | 133.19 | -141.67 | -19.91 dB | -149.3 | 0.9803 | **PASS** |
| `DSD64` | `minimum` | `20000` | 6 | **134.26 dB** | 150 dB | 133.62 | -142.23 | -20.00 dB | -150.3 | 0.9845 | **PASS** |
| `DSD128` | `linear` | `off` | 7 | **178.99 dB** | 201 dB | 177.84 | -184.19 | -46.16 dB | -192.3 | 0.9903 | **PASS** |
| `DSD128` | `linear` | `20000` | 7 | **179.16 dB** | 201 dB | 177.89 | -183.84 | -46.02 dB | -193.8 | 0.9923 | **PASS** |
| `DSD128` | `minimum` | `off` | 7 | **178.65 dB** | 201 dB | 177.46 | -183.67 | -46.02 dB | -190.1 | 0.9987 | **PASS** |
| `DSD128` | `minimum` | `20000` | 7 | **179.00 dB** | 201 dB | 177.80 | -183.99 | -46.05 dB | -192.0 | 0.9954 | **PASS** |
| `DSD256` | `linear` | `off` | 8 | **224.37 dB** | 242 dB | 222.21 | -226.29 | -91.63 dB | -233.4 | 0.9864 | **PASS** |
| `DSD256` | `linear` | `20000` | 8 | **224.49 dB** | 242 dB | 222.23 | -226.15 | -91.39 dB | -234.3 | 0.9915 | **PASS** |
| `DSD256` | `minimum` | `off` | 8 | **224.60 dB** | 242 dB | 222.55 | -226.79 | -91.46 dB | -235.1 | 1.0236 | **PASS** |
| `DSD256` | `minimum` | `20000` | 8 | **224.74 dB** | 242 dB | 222.58 | -226.64 | -91.57 dB | -236.6 | 1.0338 | **PASS** |
| `DSD512` | `linear` | `off` | 9 | **270.60 dB** | 281 dB | 266.56 | -268.74 | -136.86 dB | -282.2 | 0.9982 | **PASS** |
| `DSD512` | `linear` | `20000` | 9 | **>= 271 dB** | 281 dB | 266.41 | -268.15 | -137.00 dB | -284.2 | 1.0043 | **PASS** |
| `DSD512` | `minimum` | `off` | 9 | **270.09 dB** | 281 dB | 266.26 | -268.57 | -136.13 dB | -279.7 | 0.9917 | **PASS** |
| `DSD512` | `minimum` | `20000` | 9 | **270.20 dB** | 281 dB | 266.01 | -268.09 | -136.37 dB | -279.4 | 0.9991 | **PASS** |
