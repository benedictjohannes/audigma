# audigma — Resampler Measurements

> **Generated:** `2026-11-08 10:56:20Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [report.md](report.md)

Polyphase rational SRC (44.1k <-> 48k families) and ASRC (clock-drift tracking).

---

## Banks

`PolyphaseResampler` builds its bank at construction from the closed-form prototype (`fir::src_prototype`, Kaiser-windowed sinc,
parameters exported from `tools/design/design_src.py`). The band edges are relative to the **lower** of the two rates (scale
`min(1, out/in)`), so a downsampler is also the anti-alias filter: passband to 20 kHz and stopband from 24.1 kHz at 44.1 kHz, scaled
with the rate. Rational banks have one row per exact output phase; the ASRC bank has 256 rows plus guard rows and is cubic interpolated
between rows. Harness mode `src-tables` dumps the compiled bank; it is compared with `design_src.bank_row`, and the prototype is rebuilt
from the rows (`h[i*phases + p] = row_p[i]`). Gains are relative to DC at the stated frequency of the lower rate (e.g. 20 kHz for 44.1 kHz).

| Mode | In -> out (Hz) | Phases | Taps | Max abs diff vs `design_src.py` | Gain @ 18 kHz (dB) | @ 20 kHz (dB) | @ 21 kHz (dB) | @ low Nyquist (dB) | Passband deviation 0-20 kHz (dB) | Rejection >= 24.1 kHz (dB) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rational | 44100 -> 48000 | 160 | 110 | 1.1e-15 | 0.000000 | -0.000001 | -0.11 | -6.02 | 8.7e-07 | 141.4 |
| rational | 48000 -> 44100 | 147 | 120 | 1.0e-15 | 0.000000 | -0.000001 | -0.11 | -6.02 | 9.0e-07 | 140.7 |
| rational | 96000 -> 44100 | 147 | 240 | 6.7e-16 | 0.000000 | -0.000001 | -0.11 | -6.02 | 9.0e-07 | 140.7 |
| asrc | 44100 -> 44100 | 256 | 110 | 1.3e-15 | 0.000000 | -0.000001 | -0.11 | -6.02 | 8.7e-07 | 141.4 |
| asrc | 48000 -> 44100 | 256 | 120 | 1.0e-15 | 0.000000 | -0.000001 | -0.11 | -6.02 | 9.0e-07 | 140.7 |

The frequencies are those of a 44.1 kHz-family lower rate scaled to the actual one (e.g. 20 kHz at 44.1 kHz is 21.77 kHz at 48 kHz).

---

## Rational SRC (44.1 kHz family <-> 48 kHz family)

Harness mode `src-tone`: `PolyphaseResampler::new_rational(in, out)`, a coherent -6 dBFS tone (input frequency in the first
column), 131,072 output frames analysed with the Nuttall window after discarding the first 4,096. Rates are the pipeline's matched tiers
(1x = 44.1k/48k, 2x = 88.2k/96k, ... 16x = 705.6k/768k); the tone grid is scaled with the multiplier so results are
comparable. The spur is the strongest component outside the tone (image, alias or error); the residual is everything outside the tone lobe.
For an input tone above the output Nyquist the "gain" is the level of its alias; for a tone in the stopband only that column is
meaningful (the spur and residual are relative to the tiny alias). Images or aliases of 20-22.05 kHz content land in the transition band
(20-24.1 kHz), above the audio band, by design (the same convention as the half-band cascade). Output positions are exact rational phases (integer
phase accumulator, one bank row per phase), so there is no phase quantisation.

### 44.1k -> 48k (1x): input/output ratio 0.918750, 160 exact phases, 110 taps

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -160.5 | 4.90 | -160.3 | -160.3 |
| 10 kHz | -0.0000 | -159.5 | 13.90 | -157.5 | -157.6 |
| 18 kHz | 0.0000 | -146.2 | 21.90 | -146.1 | -163.3 |
| 20 kHz | -0.0000 | -141.4 | 23.90 | -141.4 | -170.9 |
| 21 kHz | -0.1083 | -38.0 | 23.10 | -38.0 | -170.1 |

### 48k -> 44.1k (1x): input/output ratio 1.088435, 147 exact phases, 120 taps

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -161.0 | 2.90 | -159.4 | -159.4 |
| 10 kHz | -0.0000 | -169.2 | 13.90 | -166.2 | -166.3 |
| 18 kHz | 0.0000 | -156.7 | 14.10 | -156.4 | -156.6 |
| 20 kHz | -0.0000 | -148.8 | 16.10 | -148.8 | -148.8 |
| 21 kHz | -0.1067 | -169.3 | 13.20 | -166.2 | -166.2 |
| 22 kHz | -5.3383 | -143.5 | 18.10 | -143.5 | -143.5 |
| 23 kHz | -33.2165 | -113.3 | 19.10 | -113.3 | -113.3 |
| 23.5 kHz | -66.2206 | -75.9 | 19.60 | -75.9 | -75.9 |
| 24.5 kHz | -142.1261 | 75.9 | 20.60 | 75.9 | -25.9 |
| 26 kHz | -148.8902 | 143.6 | 22.00 | 143.6 | -22.1 |

### 96k -> 44.1k: input/output ratio 2.176871, 147 exact phases, 240 taps

Reached when a PCM target is below the input's matched tier (e.g. 96 kHz input, 44.1 kHz PCM output).

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -171.7 | 6.80 | -169.3 | -169.4 |
| 10 kHz | -0.0000 | -176.9 | 2.20 | -174.3 | -174.4 |
| 18 kHz | 0.0000 | -176.0 | 10.20 | -174.0 | -174.0 |
| 20 kHz | -0.0000 | -189.0 | 12.20 | -187.8 | -187.8 |
| 21 kHz | -0.1067 | -169.2 | 13.20 | -167.8 | -167.9 |
| 22 kHz | -5.3383 | -171.8 | 14.20 | -170.3 | -170.3 |
| 23 kHz | -33.2165 | -137.5 | 15.20 | -135.3 | -135.3 |
| 23.5 kHz | -66.2206 | -104.7 | 15.70 | -103.5 | -103.6 |
| 24.5 kHz | -142.1262 | -35.3 | 16.70 | -30.7 | -30.8 |
| 26 kHz | -148.6435 | -30.2 | 18.20 | -29.4 | -29.4 |
| 30 kHz | -155.6057 | -22.3 | 21.90 | -21.8 | -30.8 |
| 40 kHz | -176.9201 | 1.2 | 11.90 | 1.6 | 1.6 |

### 88.2k -> 48k: input/output ratio 1.837500, 80 exact phases, 204 taps

Reached when a PCM target is below the input's matched tier (e.g. 96 kHz input, 44.1 kHz PCM output).

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -166.5 | 6.80 | -162.3 | -162.6 |
| 10 kHz | -0.0000 | -166.6 | 17.80 | -165.6 | -166.0 |
| 18 kHz | 0.0000 | -164.1 | 22.20 | -161.2 | -164.6 |
| 20 kHz | -0.0000 | -165.5 | 20.20 | -162.3 | -165.4 |
| 21 kHz | 0.0000 | -165.3 | 19.20 | -160.7 | -160.8 |
| 24.5 kHz | -15.3884 | -150.6 | 15.70 | -146.5 | -146.6 |
| 26 kHz | -152.3280 | -12.8 | 14.20 | -11.9 | -11.9 |
| 30 kHz | -150.0234 | -10.8 | 10.20 | -9.1 | -9.6 |
| 40 kHz | -158.5640 | -1.6 | 0.20 | 0.1 | -0.2 |

### Summary across rate tiers

Tone frequencies in the header are for the 1x tier and scale with the multiplier (spur and gain columns are per scaled tone).

| Direction | Tier | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 44.1k -> 48k | x1 | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | x1 | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |
| 44.1k -> 48k | x2 | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | x2 | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |
| 44.1k -> 48k | x4 | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | x4 | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |
| 44.1k -> 48k | x8 | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | x8 | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |
| 44.1k -> 48k | x16 | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | x16 | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |

---

## ASRC (continuously variable ratio)

`PolyphaseResampler::new_asrc(in, out)` with `set_drift_ppm(ppm)`: ratio = nominal x (1 + ppm x 1e-6). The fractional position selects
four adjacent bank rows (256 phases per input sample) and the outputs of the four are combined with 4-point Lagrange weights. Live
adaptive mode (`live_pipeline.rs`) uses this with nominal ratio `input_rate / intermediate_rate` (1 when they are equal) and drift within
`max_correction_ppm` (default 500). With 0 ppm and a nominal ratio of 1 the position never leaves phase 0, so only drifting or non-unity
ratios exercise the interpolation.

### Summary

| Case | Drift | Gain @ 1 kHz (dB) | Gain @ 18 kHz (dB) | Gain @ 20 kHz (dB) | Spur @ 1 kHz (dBc) | Spur @ 10 kHz (dBc) | Spur @ 18 kHz (dBc) | Worst of the three (dBc) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 44.1k -> 44.1k | +0 ppm | 0.0000 | -0.000 | 0.00 | -253.1 | -232.4 | -228.8 | -228.8 |
| 44.1k -> 44.1k | +10 ppm | -0.0000 | 0.000 | -0.00 | -193.5 | -188.0 | -187.5 | -187.5 |
| 44.1k -> 44.1k | +50 ppm | -0.0000 | 0.000 | -0.00 | -160.5 | -159.3 | -146.2 | -146.2 |
| 44.1k -> 44.1k | -50 ppm | -0.0000 | 0.000 | -0.00 | -160.5 | -159.3 | -146.2 | -146.2 |
| 44.1k -> 44.1k | +500 ppm | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 176.4k -> 176.4k | +50 ppm | -0.0000 | 0.000 | -0.00 | -160.5 | -159.3 | -146.2 | -146.2 |
| 48k -> 48k | +50 ppm | -0.0000 | 0.000 | -0.00 | -164.9 | -158.4 | -149.6 | -149.6 |
| 44.1k -> 48k | +0 ppm | -0.0000 | 0.000 | -0.00 | -160.5 | -159.5 | -146.2 | -146.2 |
| 48k -> 44.1k | +0 ppm | -0.0000 | 0.000 | -0.00 | -161.0 | -169.2 | -156.7 | -156.7 |
| 48k -> 44.1k | +100 ppm | -0.0000 | 0.000 | -0.00 | -161.0 | -169.1 | -156.7 | -156.7 |

Interpolation error estimate: for a prototype sampled at 256 points per input sample, the 4-point Lagrange error for a tone at f
is about `(2*pi*f/(fs*256))^4 * 0.023`, i.e. -189 dBc at 20 kHz (44.1 kHz input):
below the prototype's own stopband, so the measured spurs are set by the prototype and the analysis, not by the interpolation.

### Tone detail, 44.1 kHz -> 44.1 kHz at +50 ppm

| Input tone | Gain (dB) | Worst spur (dBc) | Spur frequency (kHz) | Total residual (dBc) | Residual 20 Hz-20 kHz (dBc) |
|---:|---:|---:|---:|---:|---:|
| 1 kHz | -0.0000 | -160.5 | 1.00 | -160.4 | -160.4 |
| 10 kHz | -0.0000 | -159.3 | 10.00 | -157.5 | -157.5 |
| 18 kHz | 0.0000 | -146.2 | 18.00 | -146.1 | -146.1 |
| 20 kHz | -0.0000 | -141.4 | 20.00 | -141.4 | -141.4 |
| 21 kHz | -0.1082 | -38.0 | 21.00 | -38.0 | -211.2 |
