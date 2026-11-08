# audigma — Modulator Measurements

> **Generated:** `2026-11-08 07:50:56Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Index:** [report.md](report.md)

CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope.

## Modulator Architecture & Selection Matrix

| Parameter / Metric | 5th-Order CRFB | 7th-Order CRFB | Comparison (this test only) |
|---|---|---|---|
| **Resonator Couplings** | 2 pairs (g1, g2) | 3 pairs (g1, g2, g3) | — |
| **Out-of-Band Gain H_inf** | 1.40 | 1.40 | Synthesized via Schreier prototype |
| **Input limit L (compiled, all rates)** | -4.25 to -4.00 dBFS | -4.75 to -4.50 dBFS | — |
| **DSD64 (OSR ~70.6)** | **115.22 dB** (Oracle: 115.5 dB) | **133.82 dB** (Oracle: 134.6 dB) | 7th-order higher SNR by 19 dB |
| **DSD128 (OSR ~141.1)** | **148.52 dB** (Oracle: 149.0 dB) | **179.05 dB** (Oracle: 180.9 dB) | 7th-order higher SNR by 31 dB |
| **DSD256 (OSR ~282.2)** | **181.66 dB** (Oracle: 184.6 dB) | **223.97 dB** (Oracle: 226.3 dB) | 7th-order higher SNR by 42 dB |
| **DSD512 (OSR ~564.5)** | **215.59 dB** (Oracle: 221.2 dB) | **269.89 dB** (Oracle: 268.0 dB) | 7th-order higher SNR by 54 dB |

> [!NOTE]
> All measurements in this report evaluate the **compiled Rust DSP code** against live theoretical oracle synthesis from `tools/design/design_crfb.py`.


---

## Direct Modulator Verification (Theoretical Oracle vs. Compiled Bitstream)

Test conditions: Direct coherent 1 kHz sine at -6.0 dBFS (f_tone = k * f_bit / N), N = 1,048,576 samples (2^20), evaluating peak modulator dynamic range and noise-shaping NTF depth against the live mathematical oracle.

### 5th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x5 | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 115.5 dB | **115.22 dB** | -0.3 dB | 133 dB | 114.59 | -123.27 | -24.79 dB | -132.3 | 0.9470 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 149.0 dB | **148.52 dB** | -0.5 dB | 171 dB | 147.26 | -153.27 | -53.71 dB | -160.5 | 0.9148 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 184.6 dB | **181.66 dB** | -3.0 dB | 200 dB | 179.64 | -183.94 | -86.95 dB | -193.4 | 0.8903 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 221.2 dB | **215.59 dB** | -5.6 dB | 235 dB | 211.76 | -214.07 | -120.02 dB | -224.4 | 0.8873 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 119.4 dB | **119.32 dB** | -0.1 dB | 153 dB | 118.64 | -127.00 | -26.98 dB | -134.1 | 0.9132 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 152.8 dB | **152.14 dB** | -0.7 dB | 170 dB | 151.05 | -157.59 | -57.53 dB | -164.8 | 0.8538 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 188.9 dB | **186.11 dB** | -2.8 dB | 210 dB | 183.50 | -186.96 | -90.83 dB | -199.8 | 0.8943 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 224.1 dB | **219.37 dB** | -4.7 dB | 246 dB | 215.36 | -217.56 | -123.60 dB | -227.8 | 0.8517 | yes | **PASS** |

### 7th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x7 | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 134.6 dB | **133.82 dB** | -0.8 dB | 150 dB | 133.17 | -141.73 | -19.87 dB | -148.6 | 0.9023 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 180.9 dB | **179.05 dB** | -1.8 dB | 201 dB | 177.97 | -184.54 | -46.23 dB | -193.0 | 0.9139 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 226.3 dB | **223.97 dB** | -2.3 dB | 242 dB | 222.01 | -226.41 | -91.78 dB | -234.4 | 0.8805 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 268.0 dB | **269.89 dB** | +1.8 dB | 281 dB | 266.08 | -268.41 | -136.14 dB | -280.3 | 0.8950 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 140.1 dB | **139.46 dB** | -0.7 dB | 172 dB | 138.67 | -146.45 | -20.51 dB | -154.3 | 0.8716 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 186.1 dB | **184.32 dB** | -1.8 dB | 201 dB | 183.04 | -188.99 | -51.76 dB | -195.1 | 0.9106 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 230.8 dB | **229.77 dB** | -1.1 dB | 253 dB | 227.02 | -230.30 | -96.61 dB | -239.8 | 0.9072 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 269.7 dB | **>= 272 dB** | — | 282 dB | 271.41 | -273.26 | -142.11 dB | -286.3 | 0.8689 | yes | **PASS** |

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
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0019, 0.0200, 0.1678]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0151, 0.1121]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0150, 0.1073]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0156, 0.1117]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0019, 0.0224, 0.1471]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0138, 0.1027]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0142, 0.1064]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0002, 0.0050, 0.0605]` | 0 | **PASS** |

#### 7th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_7] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0088, 0.0542, 0.2172]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0087, 0.0517, 0.2329]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0012, 0.0110, 0.0648, 0.2476]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0097, 0.0599, 0.2522]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0009, 0.0090, 0.0599, 0.2471]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0086, 0.0508, 0.2251]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0011, 0.0102, 0.0648, 0.2541]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0011, 0.0103, 0.0590, 0.2341]` | 0 | **PASS** |

### Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak State (xn) | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 5 | +0.50 | 0.7500 | 0.9514 | 0 | **PASS** |
| `DSD128` | 5 | +0.50 | 0.7500 | 0.8967 | 0 | **PASS** |
| `DSD256` | 5 | +0.50 | 0.7500 | 0.9227 | 0 | **PASS** |
| `DSD512` | 5 | +0.50 | 0.7500 | 0.9379 | 0 | **PASS** |
| `DSD64_48` | 5 | +0.50 | 0.7500 | 0.9264 | 0 | **PASS** |
| `DSD128_48` | 5 | +0.50 | 0.7500 | 0.9288 | 0 | **PASS** |
| `DSD256_48` | 5 | +0.50 | 0.7500 | 0.9091 | 0 | **PASS** |
| `DSD512_48` | 5 | +0.50 | 0.7500 | 0.9236 | 0 | **PASS** |
| `DSD64` | 7 | +0.50 | 0.7500 | 0.8506 | 0 | **PASS** |
| `DSD128` | 7 | +0.50 | 0.7500 | 0.8055 | 0 | **PASS** |
| `DSD256` | 7 | +0.50 | 0.7500 | 0.8694 | 0 | **PASS** |
| `DSD512` | 7 | +0.50 | 0.7500 | 0.8591 | 0 | **PASS** |
| `DSD64_48` | 7 | +0.50 | 0.7500 | 0.9289 | 0 | **PASS** |
| `DSD128_48` | 7 | +0.50 | 0.7500 | 0.8680 | 0 | **PASS** |
| `DSD256_48` | 7 | +0.50 | 0.7500 | 0.8639 | 0 | **PASS** |
| `DSD512_48` | 7 | +0.50 | 0.7500 | 0.8482 | 0 | **PASS** |

### High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak State (xn) | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
| `DSD64` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7514 | 0 | **PASS** |
| `DSD128` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.8023 | 0 | **PASS** |
| `DSD256` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7118 | 0 | **PASS** |
| `DSD512` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7784 | 0 | **PASS** |
| `DSD64` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7379 | 0 | **PASS** |
| `DSD128` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7249 | 0 | **PASS** |
| `DSD256` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7339 | 0 | **PASS** |
| `DSD512` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.6464 | 0 | **PASS** |

### Input limit L (compiled, per order and rate)
- `L` is `max_stable_input_dbfs` from the coefficient tables (generated by `design_crfb.measure_msa_dbfs`: DC and 50 Hz-20 kHz sines, 0.25 dB
  level steps, 2^21 samples, Python reference loop without dither). Here the **production** modulator (`process_tile_bytes`, TPDF dither on) runs
  at exactly `L` for the same stimuli, 2^21 bits each from a reset state. DSD targets get headroom `L - M` (M = 0.5 dB), so this is 0.5 dB above
  the level a 0 dBFS PCM peak reaches.
- **Criterion:** 0 overloads for every stimulus.

| Target Rate | Order | L (dBFS) | Headroom L - M (dB) | Worst max abs state | Overloads (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Verdict |
|---|:---:|---:|---:|---:|---|:---:|
| `dsd64` | 5 | -4.00 | -4.50 | 1.018 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 5 | -4.00 | -4.50 | 1.064 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 5 | -4.25 | -4.75 | 1.061 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 5 | -4.00 | -4.50 | 1.018 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 5 | -4.25 | -4.75 | 1.042 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 5 | -4.00 | -4.50 | 1.107 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 5 | -4.00 | -4.50 | 1.019 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 5 | -4.25 | -4.75 | 0.994 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64` | 7 | -4.50 | -5.00 | 0.998 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 7 | -4.75 | -5.25 | 1.003 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 7 | -4.75 | -5.25 | 1.047 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 7 | -4.75 | -5.25 | 0.987 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 7 | -4.50 | -5.00 | 0.996 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 7 | -4.75 | -5.25 | 1.045 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 7 | -4.75 | -5.25 | 0.990 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 7 | -4.75 | -5.25 | 0.978 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |

### Intentional Overload & Autonomous Recovery
- Injects a severe +5.0 step transient (violating the stable envelope), triggering `OverloadPolicy::MuteRecover`.
- **Expected Behavior:** Overload event is recorded, integrator states are zeroed, output falls back to balanced DSD idle sequence (`0x69`), and modulation cleanly resumes once signal returns to normal.

| Target Rate | Order | Injected Transient | Overload Detected | Idle 0x69 Pattern Emitted | States Clamped & Recovered | Verdict |
|---|:---:|---:|:---:|:---:|:---:|:---:|
| `DSD64` | 5 | +5.0 (step overload) | Yes (342 events) | Yes (341 bytes) | Yes (States Bounded) | **PASS** |
| `DSD128` | 5 | +5.0 (step overload) | Yes (342 events) | Yes (341 bytes) | Yes (States Bounded) | **PASS** |
| `DSD256` | 5 | +5.0 (step overload) | Yes (342 events) | Yes (341 bytes) | Yes (States Bounded) | **PASS** |
| `DSD512` | 5 | +5.0 (step overload) | Yes (342 events) | Yes (341 bytes) | Yes (States Bounded) | **PASS** |
| `DSD64` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (242 bytes) | Yes (States Bounded) | **PASS** |
| `DSD128` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (242 bytes) | Yes (States Bounded) | **PASS** |
| `DSD256` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (242 bytes) | Yes (States Bounded) | **PASS** |
| `DSD512` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (242 bytes) | Yes (States Bounded) | **PASS** |
