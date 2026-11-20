# audigma — Modulator Measurements

> **Specification:** `Design.md §4` and `§8`.  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [README.md](README.md)

CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope.

## Modulator Architecture & Selection Matrix

| Parameter / Metric | 5th-Order CRFB | 7th-Order CRFB | Comparison (this test only) |
|---|---|---|---|
| **Resonator Couplings** | 2 pairs (g1, g2) | 3 pairs (g1, g2, g3) | — |
| **Out-of-Band Gain H_inf** (preset `reference`, per rate) | 1.40 to 1.15 | 1.40 to 1.10 | Pinned designs (`crfb_designs.json`) |
| **Input limit L (compiled, all rates)** | -4.25 to -1.75 dBFS | -4.50 to -2.00 dBFS | — |
| **DSD64 (OSR ~70.6)** | **115.22 dB** (Oracle: 115.5 dB) | **133.82 dB** (Oracle: 134.6 dB) | 7th-order higher SNR by 19 dB |
| **DSD128 (OSR ~141.1)** | **143.91 dB** (Oracle: 145.2 dB) | **157.18 dB** (Oracle: 157.9 dB) | 7th-order higher SNR by 13 dB |
| **DSD256 (OSR ~282.2)** | **164.61 dB** (Oracle: 167.6 dB) | **>= 170 dB** (Oracle: 175.6 dB) | 7th-order higher SNR by 9 dB |
| **DSD512 (OSR ~564.5)** | **185.81 dB** (Oracle: 193.1 dB) | **>= 202 dB** (Oracle: 210.9 dB) | 7th-order higher SNR by 20 dB |

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
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 145.2 dB | **143.91 dB** | -1.3 dB | 165 dB | 142.23 | -147.18 | -49.05 dB | -155.1 | 0.8551 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 167.6 dB | **164.61 dB** | -3.0 dB | 178 dB | 159.13 | -160.57 | -69.67 dB | -174.4 | 0.5800 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 193.1 dB | **185.81 dB** | -7.2 dB | 197 dB | 176.15 | -176.64 | -90.96 dB | -194.1 | 0.4096 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 119.4 dB | **119.32 dB** | -0.1 dB | 153 dB | 118.64 | -127.00 | -26.98 dB | -134.1 | 0.9759 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 146.7 dB | **145.93 dB** | -0.8 dB | 162 dB | 144.09 | -148.71 | -51.30 dB | -158.6 | 0.8072 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 169.3 dB | **169.39 dB** | +0.1 dB | 187 dB | 163.23 | -164.44 | -73.90 dB | -180.7 | 0.5784 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 200.9 dB | **194.82 dB** | -6.1 dB | 213 dB | 184.29 | -184.69 | -99.19 dB | -203.1 | 0.4579 | yes | **PASS** |

### 7th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 134.6 dB | **133.82 dB** | -0.8 dB | 150 dB | 133.17 | -141.73 | -19.87 dB | -148.6 | 1.0015 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 157.9 dB | **157.18 dB** | -0.7 dB | 174 dB | 154.45 | -157.75 | -29.16 dB | -170.6 | 0.6287 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 175.6 dB | **>= 170 dB** | — | 180 dB | 165.79 | -166.51 | -40.07 dB | -187.8 | 0.3588 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 210.9 dB | **>= 202 dB** | — | 212 dB | 195.00 | -195.40 | -72.28 dB | -211.2 | 0.2820 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 140.1 dB | **139.58 dB** | -0.5 dB | 172 dB | 138.74 | -146.29 | -20.66 dB | -153.9 | 0.9879 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 150.0 dB | **>= 148 dB** | — | 158 dB | 145.45 | -147.34 | -31.91 dB | -164.2 | 0.4822 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 168.0 dB | **165.82 dB** | -2.2 dB | 176 dB | 157.21 | -157.85 | -40.02 dB | -178.7 | 0.2817 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 226.8 dB | **221.55 dB** | -5.3 dB | 236 dB | 209.48 | -209.76 | -86.66 dB | -229.9 | 0.3333 | yes | **PASS** |

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
| `DSD128` | 0.5000 | 0 / 0 | `[0.0001, 0.0026, 0.0249, 0.1559, 0.6029]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0006, 0.0080, 0.0698, 0.4025]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0002, 0.0029, 0.0359, 0.2845]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0044, 0.0362, 0.2008, 0.6931]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0001, 0.0021, 0.0214, 0.1405, 0.5704]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0006, 0.0080, 0.0704, 0.4028]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0002, 0.0041, 0.0448, 0.3190]` | 0 | **PASS** |

#### 7th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_7] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0392, 0.2053, 0.6999]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0009, 0.0103, 0.0817, 0.4355]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0021, 0.0277, 0.2495]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0000, 0.0010, 0.0173, 0.1956]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0396, 0.2069, 0.6983]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0004, 0.0051, 0.0519, 0.3382]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0000, 0.0010, 0.0172, 0.1955]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0000, 0.0001, 0.0017, 0.0241, 0.2319]` | 0 | **PASS** |

### Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak abs state | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 5 | +0.50 | 0.7500 | 0.9849 | 0 | **PASS** |
| `DSD128` | 5 | +0.50 | 0.7500 | 0.8522 | 0 | **PASS** |
| `DSD256` | 5 | +0.50 | 0.7500 | 0.5727 | 0 | **PASS** |
| `DSD512` | 5 | +0.50 | 0.7500 | 0.4068 | 0 | **PASS** |
| `DSD64_48` | 5 | +0.50 | 0.7500 | 0.9874 | 0 | **PASS** |
| `DSD128_48` | 5 | +0.50 | 0.7500 | 0.8112 | 0 | **PASS** |
| `DSD256_48` | 5 | +0.50 | 0.7500 | 0.5719 | 0 | **PASS** |
| `DSD512_48` | 5 | +0.50 | 0.7500 | 0.4564 | 0 | **PASS** |
| `DSD64` | 7 | +0.50 | 0.7500 | 1.0003 | 0 | **PASS** |
| `DSD128` | 7 | +0.50 | 0.7500 | 0.6312 | 0 | **PASS** |
| `DSD256` | 7 | +0.50 | 0.7500 | 0.3615 | 0 | **PASS** |
| `DSD512` | 7 | +0.50 | 0.7500 | 0.2887 | 0 | **PASS** |
| `DSD64_48` | 7 | +0.50 | 0.7500 | 1.0033 | 0 | **PASS** |
| `DSD128_48` | 7 | +0.50 | 0.7500 | 0.4909 | 0 | **PASS** |
| `DSD256_48` | 7 | +0.50 | 0.7500 | 0.2862 | 0 | **PASS** |
| `DSD512_48` | 7 | +0.50 | 0.7500 | 0.3489 | 0 | **PASS** |

### High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak abs state | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
| `DSD64` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9259 | 0 | **PASS** |
| `DSD128` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7816 | 0 | **PASS** |
| `DSD256` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.5410 | 0 | **PASS** |
| `DSD512` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.3857 | 0 | **PASS** |
| `DSD64` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9150 | 0 | **PASS** |
| `DSD128` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.5894 | 0 | **PASS** |
| `DSD256` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.3375 | 0 | **PASS** |
| `DSD512` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.2652 | 0 | **PASS** |

### Input limit L (compiled, per order and rate)
- `L` is `max_stable_input_dbfs` from the coefficient tables for the `reference` NTF preset (`Design.md §4.3`; `design_crfb.measure_msa_dbfs` climb,
  window minimum over neighbouring H_inf, confirmed with further dither seeds). Here the **production** modulator (`process_tile_bytes`, TPDF dither on)
  runs at exactly `L` for DC and 50 Hz-20 kHz sines, 2^21 bits each from a reset state. DSD targets get headroom `L - M` (M = 0.5 dB), so this is 0.5 dB above
  the level a 0 dBFS PCM peak reaches.
- **Criterion:** 0 overloads for every stimulus.

| Target Rate | Order | L (dBFS) | Headroom L - M (dB) | Worst max abs state | Overloads (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Verdict |
|---|:---:|---:|---:|---:|---|:---:|
| `dsd64` | 5 | -4.00 | -4.50 | 1.199 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 5 | -3.50 | -4.00 | 1.009 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 5 | -2.25 | -2.75 | 0.663 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 5 | -1.75 | -2.25 | 0.539 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 5 | -4.25 | -4.75 | 1.186 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 5 | -3.25 | -3.75 | 0.928 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 5 | -2.25 | -2.75 | 0.665 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 5 | -1.75 | -2.25 | 0.885 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64` | 7 | -4.50 | -5.00 | 1.264 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 7 | -2.50 | -3.00 | 0.721 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 7 | -2.00 | -2.50 | 0.558 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 7 | -2.00 | -2.50 | 0.376 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 7 | -4.50 | -5.00 | 1.217 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 7 | -2.25 | -2.75 | 0.850 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 7 | -2.00 | -2.50 | 0.345 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 7 | -2.00 | -2.50 | 0.409 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |

### Intentional Overload & Autonomous Recovery
- Injects a severe +5.0 step (2 tiles) into a 0.1-amplitude 1 kHz sine (timed at each target's own bit rate), triggering `OverloadPolicy::MuteRecover`.
- **Expected behavior:** the overload is recorded, integrator states are zeroed, the output falls back to the balanced DSD idle sequence (`0x69`) with no `0x00` bytes, and modulation resumes cleanly.
- **Clean resumption** is checked on the tail (after 4 settling tiles): no further overloads, no rail-lock bytes (`0x00` / `0xFF`), bounded states, and a low-passed reconstruction error
  (mean of the bits over 128-bit blocks against the mean input) of at most 1.5x that of a reference modulator that never overloaded (same seed, same sine).

| Target Rate | Order | Overloads (step / tail) | Idle 0x69 bytes in step | Tail rail bytes | Tail error rms | Reference error rms | Verdict |
|---|:---:|:---:|:---:|:---:|---:|---:|:---:|
| `DSD64` | 5 | 341 / 0 | 341 | 0 | 7.64e-03 | 7.46e-03 | **PASS** |
| `DSD128` | 5 | 303 / 0 | 303 | 0 | 7.20e-03 | 7.11e-03 | **PASS** |
| `DSD256` | 5 | 204 / 0 | 309 | 0 | 7.06e-03 | 6.76e-03 | **PASS** |
| `DSD512` | 5 | 143 / 0 | 150 | 0 | 6.78e-03 | 6.59e-03 | **PASS** |
| `DSD64` | 7 | 215 / 0 | 242 | 0 | 8.64e-03 | 8.48e-03 | **PASS** |
| `DSD128` | 7 | 138 / 0 | 163 | 0 | 7.83e-03 | 7.73e-03 | **PASS** |
| `DSD256` | 7 | 77 / 0 | 77 | 0 | 7.30e-03 | 7.29e-03 | **PASS** |
| `DSD512` | 7 | 60 / 0 | 105 | 0 | 7.00e-03 | 6.82e-03 | **PASS** |
