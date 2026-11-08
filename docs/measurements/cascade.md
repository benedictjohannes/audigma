# audigma — Cascade Measurements

> **Generated:** `2026-11-08 06:54:47Z`  
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
"droop" is the gain at that frequency relative to DC.

### Stage 0 (K = 1) against the design

| Preset | Phase | Apod. | Design ripple (dB) | Measured ripple (dB) | Design rejection (dB) | Measured rejection (dB) | Match |
|---|---|---|---:|---:|---:|---:|:---:|
| `balanced` | linear | off | 0.000005 | 0.000005 | 124.8 | 124.8 | yes |
| `balanced` | linear | gentle | 0.000006 | 0.000006 | 123.4 | 123.4 | yes |
| `balanced` | minimum | off | 0.000005 | 0.000005 | 124.8 | 124.8 | yes |
| `balanced` | minimum | gentle | 0.000008 | 0.000008 | 123.4 | 123.4 | yes |
| `reference` | linear | off | 0.000001 | 0.000001 | 141.4 | 141.5 | yes |
| `reference` | linear | gentle | 0.000001 | 0.000001 | 143.0 | 143.0 | yes |
| `reference` | minimum | off | 0.000001 | 0.000001 | 141.4 | 141.5 | yes |
| `reference` | minimum | gentle | 0.000001 | 0.000001 | 143.0 | 143.0 | yes |
| `low-latency` | linear | off | 0.000084 | 0.000084 | 100.3 | 100.3 | yes |
| `low-latency` | linear | gentle | 0.000057 | 0.000057 | 104.1 | 104.2 | yes |
| `low-latency` | minimum | off | 0.000089 | 0.000088 | 100.3 | 100.3 | yes |
| `low-latency` | minimum | gentle | 0.000057 | 0.000057 | 104.1 | 104.2 | yes |

Bands: `off` passband 0-20 kHz, stopband 24.1 kHz-Nyquist; `gentle` passband 0-17.5 kHz, stopband 22.05 kHz-Nyquist (the design's own definition). Match = rejection within 1.0 dB and ripple within 0.0001 dB of the design. The tails of all impulse records are below 1e-12 of the energy, so 512 frames capture the full response.

### Full cascade at DSD64 (K = 6, 64x), all filter settings

| Preset | Phase | Apod. | Ripple 0-20 kHz (dB) | Droop @ 17.5 kHz (dB) | Droop @ 20 kHz (dB) | Gain @ 22.05 kHz (dB) | Image rejection, band 1 (dB) | Worst image rejection (dB) | Group delay @ 1 kHz (ms) | @ 20 kHz (ms) |
|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `balanced` | linear | off | 0.000010 | -0.000 | -0.000 | -6.0 | 124.8 | 124.8 | 1.152 | 1.152 |
| `balanced` | linear | gentle | 8.725975 | -0.000 | -8.816 | -172.5 | 129.1 | 129.0 | 1.049 | 1.049 |
| `balanced` | minimum | off | 0.000013 | -0.000 | -0.000 | -6.0 | 124.8 | 124.8 | 0.081 | 0.274 |
| `balanced` | minimum | gentle | 8.725983 | -0.000 | -8.816 | -157.2 | 129.1 | 129.0 | 0.094 | 0.583 |
| `reference` | linear | off | 0.000002 | 0.000 | 0.000 | -6.0 | 141.5 | 141.5 | 1.310 | 1.310 |
| `reference` | linear | gentle | 8.962247 | 0.000 | -9.061 | -143.8 | 148.5 | 146.6 | 1.219 | 1.219 |
| `reference` | minimum | off | 0.000002 | 0.000 | 0.000 | -6.0 | 141.5 | 141.5 | 0.091 | 0.303 |
| `reference` | minimum | gentle | 8.962247 | -0.000 | -9.061 | -143.8 | 148.5 | 146.6 | 0.106 | 0.671 |
| `low-latency` | linear | off | 0.000082 | 0.000 | -0.000 | -6.0 | 100.3 | 100.3 | 0.925 | 0.925 |
| `low-latency` | linear | gentle | 8.433049 | -0.000 | -8.513 | -106.5 | 110.7 | 110.7 | 0.869 | 0.869 |
| `low-latency` | minimum | off | 0.000082 | 0.000 | -0.000 | -6.0 | 100.3 | 100.3 | 0.069 | 0.232 |
| `low-latency` | minimum | gentle | 8.433050 | -0.000 | -8.513 | -106.5 | 110.7 | 110.7 | 0.080 | 0.482 |

### Stage count (K) scaling, `linear` / `off`

K = 1..4 are the PCM tiers (88.2k..705.6k), K = 6 is DSD64 and K = 9 is DSD512.

| Preset | K | Output rate | DC gain (dB) | Ripple 0-20 kHz (dB) | Worst image rejection (dB) | Image rejection per band 1, 2, 3 (dB) | Group delay @ 1 kHz (ms) |
|---|---:|---:|---:|---:|---:|---|---:|
| `balanced` | 1 | 88.2 kHz | 0.000000 | 0.000005 | 124.8 | 124.8 | 0.998 |
| `balanced` | 2 | 176.4 kHz | 0.000000 | 0.000005 | 124.8 | 124.8, 128.2 | 1.077 |
| `balanced` | 3 | 352.8 kHz | 0.000000 | 0.000006 | 124.8 | 124.8, 129.0, 200.6 | 1.117 |
| `balanced` | 4 | 705.6 kHz | 0.000000 | 0.000008 | 124.8 | 124.8, 129.0, 200.8 | 1.137 |
| `balanced` | 6 | 2,822.4 kHz | 0.000000 | 0.000010 | 124.8 | 124.8, 129.0, 200.8 | 1.152 |
| `balanced` | 9 | 22,579.2 kHz | 0.000000 | 0.000010 | 124.8 | 124.8, 129.0, 200.8 | 1.156 |
| `reference` | 1 | 88.2 kHz | -0.000000 | 0.000001 | 141.5 | 141.5 | 1.134 |
| `reference` | 2 | 176.4 kHz | 0.000000 | 0.000001 | 141.5 | 141.5, 145.4 | 1.224 |
| `reference` | 3 | 352.8 kHz | 0.000000 | 0.000002 | 141.5 | 141.5, 145.9, 224.9 | 1.270 |
| `reference` | 4 | 705.6 kHz | 0.000000 | 0.000002 | 141.5 | 141.5, 145.9, 225.0 | 1.293 |
| `reference` | 6 | 2,822.4 kHz | -0.000000 | 0.000002 | 141.5 | 141.5, 145.9, 225.0 | 1.310 |
| `reference` | 9 | 22,579.2 kHz | 0.000000 | 0.000002 | 141.5 | 141.5, 145.9, 225.0 | 1.314 |
| `low-latency` | 1 | 88.2 kHz | 0.000000 | 0.000084 | 100.3 | 100.3 | 0.794 |
| `low-latency` | 2 | 176.4 kHz | 0.000000 | 0.000084 | 100.3 | 100.3, 110.0 | 0.862 |
| `low-latency` | 3 | 352.8 kHz | 0.000000 | 0.000082 | 100.3 | 100.3, 110.8, 168.6 | 0.896 |
| `low-latency` | 4 | 705.6 kHz | 0.000000 | 0.000082 | 100.3 | 100.3, 110.8, 168.8 | 0.913 |
| `low-latency` | 6 | 2,822.4 kHz | 0.000000 | 0.000082 | 100.3 | 100.3, 110.8, 168.8 | 0.925 |
| `low-latency` | 9 | 22,579.2 kHz | 0.000000 | 0.000082 | 100.3 | 100.3, 110.8, 168.8 | 0.929 |

Worst image rejection over all 72 combinations (3 presets x 2 phases x 2 apodizations x K in [1, 2, 3, 4, 6, 9]): **100.3 dB**; largest DC-gain deviation 1.54e-14 dB.

---

## Cascade tone cross-check

Coherent tones (1, 10, 18, 20, 21, 22 kHz, -6 dBFS) pushed through the same cascade (harness mode `cascade-tone`, 4096 baseband
frames, Nuttall window). "Predicted" is `|H(f)|` of the impulse response above evaluated at the exact tone frequency. This validates the
impulse method (tile boundaries, state handling, FTZ/DAZ); the spur column is the strongest component outside the tone, so it is the
actual image/alias level seen with a real signal.

| Preset | Phase | Apod. | K | Max measured-predicted gain difference (dB) | Gain @ 20 kHz measured (dB) | Gain @ 22 kHz measured (dB) | Worst spur, tones <= 20 kHz (dBc) | Spur frequency (kHz) | Agrees |
|---|---|---|---:|---:|---:|---:|---:|---:|:---:|
| `balanced` | linear | off | 1 | 2.12e-09 | -0.0000 | -5.389 | -126.2 | 26.09 | yes |
| `balanced` | linear | off | 6 | 2.12e-09 | -0.0000 | -5.389 | -126.2 | 26.09 | yes |
| `balanced` | linear | gentle | 1 | 4.96e-08 | -8.8776 | -109.479 | -139.2 | 24.08 | yes |
| `balanced` | linear | gentle | 6 | 4.94e-08 | -8.8776 | -109.479 | -128.6 | 68.18 | yes |
| `balanced` | minimum | off | 1 | 2.12e-09 | -0.0000 | -5.389 | -126.2 | 26.09 | yes |
| `balanced` | minimum | off | 6 | 2.12e-09 | -0.0000 | -5.389 | -126.2 | 26.09 | yes |
| `balanced` | minimum | gentle | 1 | 4.96e-08 | -8.8776 | -109.480 | -139.2 | 24.08 | yes |
| `balanced` | minimum | gentle | 6 | 4.95e-08 | -8.8776 | -109.480 | -128.6 | 68.18 | yes |
| `reference` | linear | off | 1 | 2.24e-09 | -0.0000 | -5.349 | -142.8 | 34.09 | yes |
| `reference` | linear | off | 6 | 2.24e-09 | 0.0000 | -5.349 | -143.3 | 34.09 | yes |
| `reference` | linear | gentle | 1 | 4.83e-08 | -9.1285 | -121.207 | -147.4 | 24.08 | yes |
| `reference` | linear | gentle | 6 | 4.76e-08 | -9.1285 | -121.207 | -147.4 | 24.08 | yes |
| `reference` | minimum | off | 1 | 2.24e-09 | -0.0000 | -5.349 | -142.8 | 34.09 | yes |
| `reference` | minimum | off | 6 | 2.24e-09 | 0.0000 | -5.349 | -143.3 | 34.09 | yes |
| `reference` | minimum | gentle | 1 | 4.80e-08 | -9.1285 | -121.207 | -147.4 | 24.08 | yes |
| `reference` | minimum | gentle | 6 | 4.85e-08 | -9.1285 | -121.207 | -147.4 | 24.08 | yes |
| `low-latency` | linear | off | 1 | 1.90e-09 | -0.0001 | -5.454 | -99.2 | 24.08 | yes |
| `low-latency` | linear | off | 6 | 1.91e-09 | -0.0000 | -5.455 | -99.2 | 24.08 | yes |
| `low-latency` | linear | gentle | 1 | 3.57e-08 | -8.5668 | -90.839 | -110.1 | 24.08 | yes |
| `low-latency` | linear | gentle | 6 | 3.57e-08 | -8.5668 | -90.840 | -110.1 | 24.08 | yes |
| `low-latency` | minimum | off | 1 | 1.90e-09 | -0.0001 | -5.454 | -99.2 | 24.08 | yes |
| `low-latency` | minimum | off | 6 | 1.91e-09 | -0.0001 | -5.455 | -99.2 | 24.08 | yes |
| `low-latency` | minimum | gentle | 1 | 3.57e-08 | -8.5668 | -90.839 | -110.1 | 24.08 | yes |
| `low-latency` | minimum | gentle | 6 | 3.57e-08 | -8.5668 | -90.840 | -110.1 | 24.08 | yes |

Agreement tolerance: 0.001 dB.

---

## End-to-End Cascade + Modulator Verification

Test conditions: Baseband PCM 1 kHz coherent tone at -6.0 dBFS upsampled via `HalfBandCascade` (2^K linear-phase interpolation) directly into `Modulator`.

### 5th-Order Modulator Pipeline

| Target Rate | Preset | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x5 | Verdict |
|---|---|:---:|---:|---:|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `balanced` | 6 | **112.84 dB** | 139 dB | 111.83 | -118.68 | -25.06 dB | -133.7 | 0.8557 | **PASS** |
| `DSD64` | `reference` | 6 | **112.80 dB** | 139 dB | 111.76 | -118.50 | -24.92 dB | -132.6 | 0.8058 | **PASS** |
| `DSD64` | `low-latency` | 6 | **112.72 dB** | 139 dB | 111.80 | -118.96 | -24.97 dB | -133.0 | 0.9073 | **PASS** |
| `DSD128` | `balanced` | 7 | **145.73 dB** | 177 dB | 143.94 | -148.67 | -53.43 dB | -162.8 | 0.8558 | **PASS** |
| `DSD128` | `reference` | 7 | **145.91 dB** | 177 dB | 144.21 | -149.10 | -53.54 dB | -163.5 | 0.8744 | **PASS** |
| `DSD128` | `low-latency` | 7 | **145.66 dB** | 177 dB | 144.08 | -149.21 | -53.56 dB | -162.1 | 0.8253 | **PASS** |
| `DSD256` | `balanced` | 8 | **179.08 dB** | 206 dB | 175.96 | -178.86 | -86.69 dB | -193.1 | 0.9170 | **PASS** |
| `DSD256` | `reference` | 8 | **179.09 dB** | 206 dB | 176.32 | -179.59 | -86.72 dB | -193.6 | 0.7730 | **PASS** |
| `DSD256` | `low-latency` | 8 | **178.98 dB** | 206 dB | 176.18 | -179.41 | -86.67 dB | -194.5 | 0.6887 | **PASS** |
| `DSD512` | `balanced` | 9 | **212.48 dB** | 241 dB | 207.71 | -209.47 | -120.07 dB | -221.4 | 0.7723 | **PASS** |
| `DSD512` | `reference` | 9 | **212.87 dB** | 241 dB | 207.67 | -209.24 | -120.39 dB | -223.6 | 0.8286 | **PASS** |
| `DSD512` | `low-latency` | 9 | **212.47 dB** | 241 dB | 207.99 | -209.90 | -110.95 dB | -224.3 | 0.8751 | **PASS** |

### 7th-Order Modulator Pipeline

| Target Rate | Preset | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x7 | Verdict |
|---|---|:---:|---:|---:|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `balanced` | 6 | **134.02 dB** | 150 dB | 133.40 | -142.13 | -20.08 dB | -148.9 | 0.8651 | **PASS** |
| `DSD64` | `reference` | 6 | **133.94 dB** | 150 dB | 133.38 | -142.55 | -19.89 dB | -149.8 | 0.9334 | **PASS** |
| `DSD64` | `low-latency` | 6 | **133.79 dB** | 150 dB | 133.19 | -142.05 | -20.19 dB | -148.7 | 0.8966 | **PASS** |
| `DSD128` | `balanced` | 7 | **179.33 dB** | 201 dB | 177.98 | -183.73 | -46.16 dB | -192.4 | 0.8769 | **PASS** |
| `DSD128` | `reference` | 7 | **179.20 dB** | 201 dB | 177.97 | -184.05 | -45.97 dB | -192.6 | 0.8962 | **PASS** |
| `DSD128` | `low-latency` | 7 | **179.21 dB** | 201 dB | 177.85 | -183.58 | -46.00 dB | -192.1 | 0.8733 | **PASS** |
| `DSD256` | `balanced` | 8 | **224.53 dB** | 242 dB | 222.45 | -226.64 | -91.08 dB | -237.2 | 0.8593 | **PASS** |
| `DSD256` | `reference` | 8 | **224.03 dB** | 242 dB | 222.12 | -226.61 | -91.32 dB | -234.9 | 0.7960 | **PASS** |
| `DSD256` | `low-latency` | 8 | **224.64 dB** | 242 dB | 222.29 | -226.07 | -91.52 dB | -233.3 | 0.8919 | **PASS** |
| `DSD512` | `balanced` | 9 | **270.34 dB** | 281 dB | 266.26 | -268.42 | -135.76 dB | -280.8 | 0.7778 | **PASS** |
| `DSD512` | `reference` | 9 | **270.84 dB** | 281 dB | 266.24 | -268.09 | -136.17 dB | -282.4 | 0.6742 | **PASS** |
| `DSD512` | `low-latency` | 9 | **270.49 dB** | 281 dB | 266.00 | -267.90 | -111.54 dB | -282.8 | 0.8996 | **PASS** |
