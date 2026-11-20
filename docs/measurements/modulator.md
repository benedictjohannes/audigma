# audigma — Modulator Measurements

> **Specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [README.md](README.md)

CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope.

## Modulator Architecture & Selection Matrix

| Parameter / Metric | 5th-Order CRFB | 7th-Order CRFB | Comparison (this test only) |
|---|---|---|---|
| **Resonator Couplings** | 2 pairs (g1, g2) | 3 pairs (g1, g2, g3) | — |
| **Out-of-Band Gain H_inf** (preset `reference`, per rate) | 1.40 to 1.16 | 1.40 to 1.11 | Pinned designs (`crfb_designs.json`) |
| **Input limit L (compiled, all rates)** | -4.25 to -1.75 dBFS | -5.00 to -2.00 dBFS | — |
| **DSD64 (OSR ~70.6)** | **115.22 dB** (Oracle: 115.5 dB) | **133.82 dB** (Oracle: 134.6 dB) | 7th-order higher SNR by 19 dB |
| **DSD128 (OSR ~141.1)** | **144.70 dB** (Oracle: 145.8 dB) | **159.09 dB** (Oracle: 160.1 dB) | 7th-order higher SNR by 14 dB |
| **DSD256 (OSR ~282.2)** | **163.12 dB** (Oracle: 165.8 dB) | **>= 175 dB** (Oracle: 179.5 dB) | 7th-order higher SNR by 14 dB |
| **DSD512 (OSR ~564.5)** | **187.81 dB** (Oracle: 194.5 dB) | **>= 207 dB** (Oracle: 216.1 dB) | 7th-order higher SNR by 23 dB |

> [!NOTE]
> All measurements in this report evaluate the **compiled Rust DSP code** against the pinned CRFB designs (`tools/design/crfb_designs.json`, the source of the coefficient tables), preset `reference`.


---

## Direct Modulator Verification (Theoretical Oracle vs. Compiled Bitstream)

Test conditions: Direct coherent 1 kHz sine at -6.0 dBFS (f_tone = k * f_bit / N), N = 1,048,576 samples (2^20), evaluating peak modulator dynamic range and noise-shaping NTF depth against the pinned design (`crfb_designs.json`).
Verdict = stable and bit-exact against the reference loop. Quality figures here are noise-shaper limits without dither or gain staging; the quality requirements (`Design.md` §8.6) are checked on the production chain (`signal.md`).

### 5th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 115.5 dB | **115.22 dB** | -0.3 dB | 133 dB | 114.59 | -123.27 | -24.79 dB | -132.3 | 0.9778 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 145.8 dB | **144.70 dB** | -1.1 dB | 166 dB | 143.21 | -148.57 | -49.56 dB | -157.8 | 0.8749 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 165.8 dB | **163.12 dB** | -2.7 dB | 176 dB | 157.43 | -158.80 | -68.06 dB | -173.9 | 0.5551 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 194.5 dB | **187.81 dB** | -6.7 dB | 199 dB | 178.08 | -178.57 | -93.06 dB | -196.0 | 0.4351 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 119.4 dB | **119.32 dB** | -0.1 dB | 153 dB | 118.64 | -127.00 | -26.98 dB | -134.1 | 0.9759 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 147.8 dB | **147.34 dB** | -0.4 dB | 163 dB | 145.43 | -149.93 | -52.03 dB | -162.5 | 0.8285 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 169.9 dB | **167.89 dB** | -2.0 dB | 185 dB | 161.19 | -162.24 | -72.45 dB | -180.0 | 0.5546 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 203.6 dB | **196.51 dB** | -7.0 dB | 216 dB | 186.25 | -186.68 | -101.16 dB | -206.4 | 0.4841 | yes | **PASS** |

### 7th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 134.6 dB | **133.82 dB** | -0.8 dB | 150 dB | 133.17 | -141.73 | -19.87 dB | -148.6 | 1.0015 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 160.1 dB | **159.09 dB** | -1.0 dB | 176 dB | 156.60 | -160.21 | -29.14 dB | -173.0 | 0.6486 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 179.5 dB | **>= 175 dB** | — | 185 dB | 169.57 | -170.37 | -40.64 dB | -187.4 | 0.3851 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 216.1 dB | **>= 207 dB** | — | 217 dB | 200.06 | -200.44 | -76.70 dB | -219.4 | 0.3078 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 140.1 dB | **139.58 dB** | -0.5 dB | 172 dB | 138.74 | -146.29 | -20.66 dB | -153.9 | 0.9879 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 152.7 dB | **>= 151 dB** | — | 161 dB | 148.03 | -149.97 | -31.26 dB | -167.7 | 0.5081 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 173.6 dB | **170.11 dB** | -3.4 dB | 181 dB | 162.20 | -162.96 | -39.86 dB | -179.9 | 0.3080 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 233.2 dB | **224.90 dB** | -8.3 dB | 240 dB | 213.41 | -213.73 | -90.69 dB | -232.9 | 0.3588 | yes | **PASS** |

> [!NOTE]
> **Method** (`tools/design/dsm_analysis.py`). Each bitstream is analysed with one full-record periodic Nuttall window (4-term, continuous first derivative: -93 dB sidelobes, 18 dB/octave decay); the tone is coherent (on a bin). Noise excludes the tone lobe and harmonic lobes (bins +-4); "Worst spur" is the strongest 9-bin group in the remaining 20 Hz-20 kHz bins relative to the tone. The modulator runs without dither here, so these are noise-shaper limits.
> **Analysis floor** is the highest SNR the method resolves for that modulator's NTF: a synthetic non-periodic record with the same out-of-band noise shape and an in-band level 300 dB below the tone is analysed the same way. SNRs within 10 dB of it are printed as lower bounds and are not compared.
> **Bit-exact vs reference**: the Rust record equals, bit for bit, a Python run of the designed CRFB loop (`simulate_crfb`, coefficients from `design_crfb.py`) on the same input. The oracle SNR is that loop with a 2^18 record and small TPDF dither (0.005), analysed the same way.
> Earlier windows were invalid. A rectangular window treats the non-periodic record as periodic, so every integrator state jumps at the wrap and the out-of-band noise spreads over all bins (58-115 dB "SNR"). The 4-term Blackman-Harris window has a small edge step, decays only 6 dB/octave and floored results at ~145-150 dB.

---

## Stability & Dynamic Envelope Verification

Modulator stability was evaluated across four distinct dynamic edge cases per `Design.md §4.3` and `§8`:

### Digital Silence (Zero Input)
- Evaluates balanced ones/zeros ratio and checks for absence of subnormal float decay, rail-lock sequences (0x00 / 0xFF), and idle tone spikes.
- **Criterion:** Ones ratio 0.50 +/- 0.05, 0 rail-lock bytes (0x00 / 0xFF), all states |x_i| < 50.0, 0 overloads.

#### 5th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_5] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0003, 0.0043, 0.0361, 0.2005, 0.6910]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0002, 0.0029, 0.0267, 0.1635, 0.6189]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0005, 0.0074, 0.0681, 0.3886]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0002, 0.0035, 0.0403, 0.3018]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0044, 0.0362, 0.2008, 0.6931]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0001, 0.0024, 0.0231, 0.1478, 0.5853]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0005, 0.0072, 0.0655, 0.3868]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0003, 0.0048, 0.0495, 0.3359]` | 0 | **PASS** |

#### 7th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_7] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0392, 0.2053, 0.6999]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0011, 0.0121, 0.0929, 0.4550]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0025, 0.0317, 0.2671]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0013, 0.0206, 0.2137]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0396, 0.2069, 0.6983]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0004, 0.0059, 0.0572, 0.3551]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0013, 0.0205, 0.2135]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0021, 0.0279, 0.2496]` | 0 | **PASS** |

### Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak abs state | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 5 | +0.50 | 0.7500 | 0.9849 | 0 | **PASS** |
| `DSD128` | 5 | +0.50 | 0.7500 | 0.8822 | 0 | **PASS** |
| `DSD256` | 5 | +0.50 | 0.7500 | 0.5540 | 0 | **PASS** |
| `DSD512` | 5 | +0.50 | 0.7500 | 0.4337 | 0 | **PASS** |
| `DSD64_48` | 5 | +0.50 | 0.7500 | 0.9874 | 0 | **PASS** |
| `DSD128_48` | 5 | +0.50 | 0.7500 | 0.8372 | 0 | **PASS** |
| `DSD256_48` | 5 | +0.50 | 0.7500 | 0.5479 | 0 | **PASS** |
| `DSD512_48` | 5 | +0.50 | 0.7500 | 0.4749 | 0 | **PASS** |
| `DSD64` | 7 | +0.50 | 0.7500 | 1.0003 | 0 | **PASS** |
| `DSD128` | 7 | +0.50 | 0.7500 | 0.6573 | 0 | **PASS** |
| `DSD256` | 7 | +0.50 | 0.7500 | 0.3900 | 0 | **PASS** |
| `DSD512` | 7 | +0.50 | 0.7500 | 0.3106 | 0 | **PASS** |
| `DSD64_48` | 7 | +0.50 | 0.7500 | 1.0033 | 0 | **PASS** |
| `DSD128_48` | 7 | +0.50 | 0.7500 | 0.5124 | 0 | **PASS** |
| `DSD256_48` | 7 | +0.50 | 0.7500 | 0.3059 | 0 | **PASS** |
| `DSD512_48` | 7 | +0.50 | 0.7500 | 0.3569 | 0 | **PASS** |

### High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak abs state | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
| `DSD64` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9259 | 0 | **PASS** |
| `DSD128` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.8071 | 0 | **PASS** |
| `DSD256` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.5230 | 0 | **PASS** |
| `DSD512` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.4085 | 0 | **PASS** |
| `DSD64` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9150 | 0 | **PASS** |
| `DSD128` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.6129 | 0 | **PASS** |
| `DSD256` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.3600 | 0 | **PASS** |
| `DSD512` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.2890 | 0 | **PASS** |

### Input limit L (compiled, per order and rate)
- `L` is `max_stable_input_dbfs` from the coefficient tables for the `reference` NTF preset (`Design.md §4.3`; `design_crfb.measure_msa_dbfs` climb,
  window minimum over neighbouring H_inf, confirmed with further dither seeds). Here the **production** modulator (`process_tile_bytes`, TPDF dither on)
  runs at exactly `L` for DC and 50 Hz-20 kHz sines, 2^21 bits each from a reset state. DSD targets get headroom `L - M` (M = 0.5 dB), so this is 0.5 dB above
  the level a 0 dBFS PCM peak reaches.
- **Criterion:** 0 overloads for every stimulus.

| Target Rate | Order | L (dBFS) | Headroom L - M (dB) | Worst max abs state | Overloads (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Verdict |
|---|:---:|---:|---:|---:|---|:---:|
| `dsd64` | 5 | -4.25 | -4.75 | 1.133 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 5 | -3.50 | -4.00 | 1.041 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 5 | -2.00 | -2.50 | 0.645 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 5 | -1.75 | -2.25 | 1.045 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 5 | -4.25 | -4.75 | 1.186 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 5 | -3.25 | -3.75 | 0.979 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 5 | -2.00 | -2.50 | 0.644 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 5 | -1.75 | -2.25 | 0.663 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64` | 7 | -4.50 | -5.00 | 1.264 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 7 | -2.50 | -3.00 | 0.904 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 7 | -2.00 | -2.50 | 0.630 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 7 | -2.00 | -2.50 | 0.370 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 7 | -5.00 | -5.50 | 1.095 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 7 | -2.25 | -2.75 | 0.672 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 7 | -2.00 | -2.50 | 0.364 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 7 | -2.00 | -2.50 | 0.493 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |

### Intentional Overload & Autonomous Recovery
- Injects a severe +5.0 step (2 tiles) into a 0.1-amplitude 1 kHz sine (timed at each target's own bit rate), triggering `OverloadPolicy::MuteRecover`.
- **Expected behavior:** the overload is recorded, integrator states are zeroed, the output falls back to the balanced DSD idle sequence (`0x69`) with no `0x00` bytes, and modulation resumes cleanly.
- **Clean resumption** is checked on the tail (after 4 settling tiles): no further overloads, no rail-lock bytes (`0x00` / `0xFF`), bounded states, and a low-passed reconstruction error
  (mean of the bits over 128-bit blocks against the mean input) of at most 1.5x that of a reference modulator that never overloaded (same seed, same sine).

| Target Rate | Order | Overloads (step / tail) | Idle 0x69 bytes in step | Tail rail bytes | Tail error rms | Reference error rms | Verdict |
|---|:---:|:---:|:---:|:---:|---:|---:|:---:|
| `DSD64` | 5 | 341 / 0 | 341 | 0 | 7.64e-03 | 7.46e-03 | **PASS** |
| `DSD128` | 5 | 315 / 0 | 315 | 0 | 7.37e-03 | 7.23e-03 | **PASS** |
| `DSD256` | 5 | 199 / 0 | 208 | 0 | 6.77e-03 | 6.76e-03 | **PASS** |
| `DSD512` | 5 | 151 / 0 | 170 | 0 | 6.94e-03 | 6.67e-03 | **PASS** |
| `DSD64` | 7 | 215 / 0 | 242 | 0 | 8.64e-03 | 8.48e-03 | **PASS** |
| `DSD128` | 7 | 146 / 0 | 292 | 0 | 7.97e-03 | 7.71e-03 | **PASS** |
| `DSD256` | 7 | 84 / 0 | 102 | 0 | 7.57e-03 | 7.11e-03 | **PASS** |
| `DSD512` | 7 | 66 / 0 | 70 | 0 | 6.81e-03 | 6.85e-03 | **PASS** |
