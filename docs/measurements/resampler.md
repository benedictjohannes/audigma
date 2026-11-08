# audigma — Resampler Measurements

> **Generated:** `2026-11-08 06:54:48Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [report.md](report.md)

Polyphase rational SRC (44.1k <-> 48k families) and ASRC (clock-drift tracking).

---

## Exported tables

Harness mode `src-tables` dumps `SRC_RATIONAL_160_PHASES` and `SRC_ASRC_256_PHASES` as compiled. The prototype filter
is rebuilt from them (`h[j*phases + p] = table[p][j]`) and its response is given relative to the input rate. The tap and phase
counts come from `audigma_dsp::{SRC_TAPS_PER_PHASE, SRC_RATIONAL_PHASES, SRC_ASRC_PHASES}`, which are derived from the tables.

| Table | Phases | Taps/phase | Max abs diff vs `design_src.py` | Gain @ 18 kHz (dB) | @ 20 kHz (dB) | @ 21 kHz (dB) | @ Nyquist (dB) | Stopband rejection (>= 0.5465 fs_in) (dB) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rational | 160 | 48 | 0.0e+00 | -0.202 | -6.02 | -15.56 | -33.9 | 122.4 |
| asrc | 256 | 48 | 0.0e+00 | -0.202 | -6.02 | -15.56 | -33.9 | 122.4 |

Gain is relative to DC and expressed for a 44.1 kHz input rate (frequencies scale with the input rate).

---

## Rational SRC (44.1 kHz family <-> 48 kHz family)

Harness mode `src-tone`: `PolyphaseResampler::new_rational(in, out)`, a coherent -6 dBFS tone (input frequency in the first
column), 131,072 output frames analysed with the Nuttall window after discarding the first 4,096. Rates are the pipeline's matched tiers
(1x = 44.1k/48k, 2x = 88.2k/96k, ... 16x = 705.6k/768k); the tone grid is scaled with the multiplier so results are
comparable. The spur is the strongest component outside the tone (image, alias or error); the residual is everything outside the tone lobe.

### 44.1k -> 48k (1x): input/output ratio 0.918750, 160 table phases

Fractional output positions are multiples of 1/160 input samples; flooring them onto the 160-phase table gives a worst-case delay error of 0.00000 input samples (on the table grid: no quantisation).

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -130.2 | 2.90 | -127.3 | -127.4 |
| 10 kHz | -0.0000 | -124.6 | 13.90 | -122.8 | -122.9 |
| 18 kHz | -0.2021 | -123.6 | 21.90 | -120.9 | -125.5 |
| 20 kHz | -6.0241 | -111.0 | 23.90 | -110.6 | -122.2 |
| 21 kHz | -15.5547 | -53.5 | 23.10 | -53.5 | -114.5 |

### 48k -> 44.1k (1x): input/output ratio 1.088435, 160 table phases

Fractional output positions are multiples of 1/147 input samples; flooring them onto the 160-phase table gives a worst-case delay error of 0.00621 input samples (**not on the table grid**).

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -77.7 | 5.60 | -72.5 | -72.7 |
| 10 kHz | -0.0000 | -57.7 | 3.40 | -52.5 | -53.0 |
| 18 kHz | 0.0001 | -52.4 | 19.50 | -47.4 | -47.5 |
| 20 kHz | -0.4936 | -51.7 | 13.40 | -46.5 | -46.6 |
| 21 kHz | -2.4846 | -51.0 | 14.40 | -46.1 | -46.1 |
| 22 kHz | -7.5518 | -50.8 | 15.40 | -45.7 | -45.7 |
| 23 kHz | -17.3056 | -45.7 | 19.10 | -42.5 | -42.5 |
| 23.5 kHz | -24.5611 | -21.6 | 19.60 | -21.6 | -21.6 |

### Summary across rate tiers

Tone frequencies in the header are for the 1x tier and scale with the multiplier (spur and gain columns are per scaled tone).

| Direction | Tier | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 44.1k -> 48k | x1 | -0.0000 | -0.202 | -6.02 | -130.2 | -124.6 | -123.6 | -123.6 |
| 48k -> 44.1k | x1 | -0.0000 | 0.000 | -0.49 | -77.7 | -57.7 | -52.4 | -52.4 |
| 44.1k -> 48k | x2 | -0.0000 | -0.202 | -6.02 | -130.2 | -124.6 | -123.6 | -123.6 |
| 48k -> 44.1k | x2 | -0.0000 | 0.000 | -0.49 | -77.7 | -57.7 | -52.4 | -52.4 |
| 44.1k -> 48k | x4 | -0.0000 | -0.202 | -6.02 | -130.2 | -124.6 | -123.6 | -123.6 |
| 48k -> 44.1k | x4 | -0.0000 | 0.000 | -0.49 | -77.7 | -57.7 | -52.4 | -52.4 |
| 44.1k -> 48k | x8 | -0.0000 | -0.202 | -6.02 | -130.2 | -124.6 | -123.6 | -123.6 |
| 48k -> 44.1k | x8 | -0.0000 | 0.000 | -0.49 | -77.7 | -57.7 | -52.4 | -52.4 |
| 44.1k -> 48k | x16 | -0.0000 | -0.202 | -6.02 | -130.2 | -124.6 | -123.6 | -123.6 |
| 48k -> 44.1k | x16 | -0.0000 | 0.000 | -0.49 | -77.7 | -57.7 | -52.4 | -52.4 |

---

## ASRC (continuously variable ratio)

`PolyphaseResampler::new_asrc(in, out)` with `set_drift_ppm(ppm)`: ratio = nominal x (1 + ppm x 1e-6), the ASRC table indexed by the
floored fractional position. Live adaptive mode (`live_pipeline.rs`) uses this with nominal ratio `input_rate / intermediate_rate` (1 when they are equal) and drift within `max_correction_ppm` (default 500).
With 0 ppm and a nominal ratio of 1 the fractional position never leaves phase 0, so only drifting or non-unity ratios exercise the phase selection.

### Summary

| Case | Drift | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 44.1k -> 44.1k | +0 ppm | -0.0000 | -0.202 | -6.02 | -253.1 | -232.3 | -228.6 | -228.6 |
| 44.1k -> 44.1k | +10 ppm | -0.0000 | -0.202 | -6.03 | -81.1 | -61.0 | -55.9 | -55.9 |
| 44.1k -> 44.1k | +50 ppm | -0.0000 | -0.202 | -6.02 | -81.1 | -61.0 | -55.9 | -55.9 |
| 44.1k -> 44.1k | -50 ppm | -0.0000 | -0.202 | -6.02 | -81.1 | -61.0 | -55.9 | -55.9 |
| 44.1k -> 44.1k | +500 ppm | -0.0000 | -0.202 | -6.03 | -81.1 | -61.0 | -55.9 | -55.9 |
| 176.4k -> 176.4k | +50 ppm | -0.0000 | -0.202 | -6.02 | -81.1 | -61.0 | -55.9 | -55.9 |
| 48k -> 48k | +50 ppm | -0.0000 | -0.000 | -0.49 | -81.8 | -61.8 | -56.7 | -56.7 |
| 44.1k -> 48k | +0 ppm | -0.0000 | -0.202 | -6.02 | -80.5 | -60.5 | -55.4 | -55.4 |
| 48k -> 44.1k | +0 ppm | -0.0000 | -0.000 | -0.49 | -81.8 | -61.8 | -56.5 | -56.5 |

Phase selection floors the position to 1/256 of an input sample (`audigma_dsp::SRC_ASRC_PHASES`), so the delay error is uniform
in [0, 0.00391) samples (rms 0.00113). First-order error spur for a tone at f: `2*pi*(f/fs)*err_rms`, i.e. about
-76 dBc at 1 kHz, -56 dBc at 10 kHz and -50 dBc at 20 kHz (44.1 kHz input); compare the measured spur columns.

### Tone detail, 44.1 kHz -> 44.1 kHz at +50 ppm

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -81.1 | 0.44 | -75.9 | -75.9 |
| 10 kHz | -0.0000 | -61.0 | 9.44 | -55.9 | -55.9 |
| 18 kHz | -0.2020 | -55.9 | 17.44 | -50.8 | -51.0 |
| 20 kHz | -6.0248 | -55.0 | 19.44 | -49.9 | -52.5 |
| 21 kHz | -15.5538 | -53.5 | 21.00 | -48.0 | -55.5 |
