# audigma — Cascade Measurements

> **Generated:** `2026-11-08 07:51:14Z`  
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
| `DSD64` | `balanced` | 6 | **115.12 dB** | 133 dB | 114.51 | -123.33 | -24.82 dB | -132.6 | 0.8591 | **PASS** |
| `DSD64` | `reference` | 6 | **114.93 dB** | 133 dB | 114.30 | -123.01 | -25.02 dB | -130.9 | 0.9162 | **PASS** |
| `DSD64` | `low-latency` | 6 | **115.18 dB** | 133 dB | 114.56 | -123.34 | -24.85 dB | -131.1 | 0.9223 | **PASS** |
| `DSD128` | `balanced` | 7 | **147.97 dB** | 171 dB | 146.92 | -153.59 | -53.65 dB | -162.3 | 0.8266 | **PASS** |
| `DSD128` | `reference` | 7 | **148.00 dB** | 171 dB | 146.88 | -153.31 | -53.55 dB | -161.6 | 0.9017 | **PASS** |
| `DSD128` | `low-latency` | 7 | **148.34 dB** | 171 dB | 147.23 | -153.70 | -53.56 dB | -162.3 | 0.8577 | **PASS** |
| `DSD256` | `balanced` | 8 | **181.51 dB** | 200 dB | 179.52 | -183.86 | -86.95 dB | -191.7 | 0.8762 | **PASS** |
| `DSD256` | `reference` | 8 | **181.08 dB** | 200 dB | 179.05 | -183.32 | -86.71 dB | -191.6 | 0.8562 | **PASS** |
| `DSD256` | `low-latency` | 8 | **181.15 dB** | 200 dB | 179.33 | -184.00 | -86.66 dB | -190.4 | 0.8661 | **PASS** |
| `DSD512` | `balanced` | 9 | **215.66 dB** | 235 dB | 211.62 | -213.80 | -120.40 dB | -225.1 | 0.7831 | **PASS** |
| `DSD512` | `reference` | 9 | **215.05 dB** | 235 dB | 211.19 | -213.49 | -119.70 dB | -223.6 | 0.7302 | **PASS** |
| `DSD512` | `low-latency` | 9 | **215.22 dB** | 235 dB | 211.54 | -213.98 | -110.97 dB | -225.1 | 0.8692 | **PASS** |

### 7th-Order Modulator Pipeline

| Target Rate | Preset | Cascade Stages | In-Band SNR | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x7 | Verdict |
|---|---|:---:|---:|---:|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `balanced` | 6 | **133.94 dB** | 150 dB | 133.29 | -141.86 | -20.01 dB | -150.3 | 0.9001 | **PASS** |
| `DSD64` | `reference` | 6 | **134.09 dB** | 150 dB | 133.41 | -141.78 | -20.01 dB | -148.4 | 0.8324 | **PASS** |
| `DSD64` | `low-latency` | 6 | **133.88 dB** | 150 dB | 133.21 | -141.66 | -19.96 dB | -149.1 | 0.9030 | **PASS** |
| `DSD128` | `balanced` | 7 | **179.05 dB** | 201 dB | 177.92 | -184.32 | -46.04 dB | -191.9 | 0.9218 | **PASS** |
| `DSD128` | `reference` | 7 | **179.09 dB** | 201 dB | 177.83 | -183.82 | -45.85 dB | -191.4 | 0.8579 | **PASS** |
| `DSD128` | `low-latency` | 7 | **178.95 dB** | 201 dB | 177.86 | -184.42 | -46.05 dB | -191.8 | 0.8922 | **PASS** |
| `DSD256` | `balanced` | 8 | **224.48 dB** | 242 dB | 222.57 | -227.07 | -91.27 dB | -235.8 | 0.8435 | **PASS** |
| `DSD256` | `reference` | 8 | **224.71 dB** | 242 dB | 222.63 | -226.84 | -91.15 dB | -236.4 | 0.9045 | **PASS** |
| `DSD256` | `low-latency` | 8 | **224.54 dB** | 242 dB | 222.34 | -226.34 | -91.50 dB | -236.1 | 0.8454 | **PASS** |
| `DSD512` | `balanced` | 9 | **269.99 dB** | 281 dB | 266.07 | -268.33 | -135.94 dB | -280.9 | 0.6705 | **PASS** |
| `DSD512` | `reference` | 9 | **270.48 dB** | 281 dB | 266.17 | -268.18 | -136.80 dB | -281.1 | 0.7327 | **PASS** |
| `DSD512` | `low-latency` | 9 | **269.22 dB** | 281 dB | 265.27 | -267.52 | -111.54 dB | -275.9 | 0.8513 | **PASS** |
