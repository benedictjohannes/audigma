# audigma — Modulator Measurements

> **Generated:** `2026-11-10 03:39:44Z`  
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
| **DSD256 (OSR ~282.2)** | **181.66 dB** (Oracle: 184.6 dB) | **224.31 dB** (Oracle: 225.9 dB) | 7th-order higher SNR by 43 dB |
| **DSD512 (OSR ~564.5)** | **215.59 dB** (Oracle: 221.2 dB) | **270.09 dB** (Oracle: 270.4 dB) | 7th-order higher SNR by 54 dB |

> [!NOTE]
> All measurements in this report evaluate the **compiled Rust DSP code** against live theoretical oracle synthesis from `tools/design/design_crfb.py`.


---

## Direct Modulator Verification (Theoretical Oracle vs. Compiled Bitstream)

Test conditions: Direct coherent 1 kHz sine at -6.0 dBFS (f_tone = k * f_bit / N), N = 1,048,576 samples (2^20), evaluating peak modulator dynamic range and noise-shaping NTF depth against the live mathematical oracle.
Verdict = stable and bit-exact against the reference loop. Quality figures here are noise-shaper limits without dither or gain staging; the quality requirements (`Design.md` §8.6) are checked on the production chain (`signal.md`).

### 5th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 115.5 dB | **115.22 dB** | -0.3 dB | 133 dB | 114.59 | -123.27 | -24.79 dB | -132.3 | 0.9778 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 149.0 dB | **148.52 dB** | -0.5 dB | 171 dB | 147.26 | -153.27 | -53.71 dB | -160.5 | 0.9727 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 184.6 dB | **181.66 dB** | -3.0 dB | 200 dB | 179.64 | -183.94 | -86.95 dB | -193.4 | 0.9825 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 221.2 dB | **215.59 dB** | -5.6 dB | 235 dB | 211.76 | -214.07 | -120.02 dB | -224.4 | 0.9854 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 119.4 dB | **119.32 dB** | -0.1 dB | 153 dB | 118.64 | -127.00 | -26.98 dB | -134.1 | 0.9759 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 152.8 dB | **152.14 dB** | -0.7 dB | 170 dB | 151.05 | -157.59 | -57.53 dB | -164.8 | 0.9837 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 188.9 dB | **186.11 dB** | -2.8 dB | 210 dB | 183.50 | -186.96 | -90.83 dB | -199.8 | 0.9784 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 224.1 dB | **219.37 dB** | -4.7 dB | 246 dB | 215.36 | -217.56 | -123.60 dB | -227.8 | 0.9795 | yes | **PASS** |

### 7th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Peak abs state | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 134.6 dB | **133.82 dB** | -0.8 dB | 150 dB | 133.17 | -141.73 | -19.87 dB | -148.6 | 1.0015 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 180.9 dB | **179.05 dB** | -1.8 dB | 201 dB | 177.97 | -184.54 | -46.23 dB | -193.1 | 1.0061 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 225.9 dB | **224.31 dB** | -1.6 dB | 242 dB | 221.99 | -225.83 | -91.66 dB | -234.7 | 0.9970 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 270.4 dB | **270.09 dB** | -0.3 dB | 281 dB | 266.08 | -268.28 | -136.40 dB | -282.3 | 1.0282 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 140.1 dB | **139.46 dB** | -0.7 dB | 172 dB | 138.67 | -146.45 | -20.51 dB | -154.3 | 0.9851 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 186.1 dB | **184.62 dB** | -1.5 dB | 201 dB | 183.27 | -188.98 | -51.53 dB | -198.2 | 0.9900 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 230.8 dB | **229.88 dB** | -1.0 dB | 253 dB | 227.54 | -231.35 | -97.09 dB | -241.6 | 0.9899 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 266.2 dB | **>= 272 dB** | — | 282 dB | 271.55 | -274.00 | -141.61 dB | -282.6 | 0.9885 | yes | **PASS** |

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
| `DSD128` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0341, 0.1863, 0.6791]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0343, 0.1866, 0.6805]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0343, 0.1868, 0.6794]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0044, 0.0362, 0.2008, 0.6931]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0341, 0.1865, 0.6798]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0343, 0.1865, 0.6800]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0003, 0.0042, 0.0343, 0.1866, 0.6795]` | 0 | **PASS** |

#### 7th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_7] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0392, 0.2053, 0.6999]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0057, 0.0419, 0.2108, 0.7002]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0006, 0.0059, 0.0417, 0.2093, 0.7008]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0006, 0.0057, 0.0414, 0.2087, 0.6996]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0005, 0.0055, 0.0396, 0.2069, 0.6983]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0006, 0.0058, 0.0412, 0.2090, 0.7028]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0006, 0.0057, 0.0416, 0.2109, 0.7011]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0006, 0.0057, 0.0415, 0.2103, 0.6992]` | 0 | **PASS** |

### Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak abs state | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 5 | +0.50 | 0.7500 | 0.9849 | 0 | **PASS** |
| `DSD128` | 5 | +0.50 | 0.7500 | 0.9922 | 0 | **PASS** |
| `DSD256` | 5 | +0.50 | 0.7500 | 0.9895 | 0 | **PASS** |
| `DSD512` | 5 | +0.50 | 0.7500 | 0.9858 | 0 | **PASS** |
| `DSD64_48` | 5 | +0.50 | 0.7500 | 0.9874 | 0 | **PASS** |
| `DSD128_48` | 5 | +0.50 | 0.7500 | 0.9850 | 0 | **PASS** |
| `DSD256_48` | 5 | +0.50 | 0.7500 | 0.9905 | 0 | **PASS** |
| `DSD512_48` | 5 | +0.50 | 0.7500 | 0.9869 | 0 | **PASS** |
| `DSD64` | 7 | +0.50 | 0.7500 | 1.0003 | 0 | **PASS** |
| `DSD128` | 7 | +0.50 | 0.7500 | 1.0139 | 0 | **PASS** |
| `DSD256` | 7 | +0.50 | 0.7500 | 1.0090 | 0 | **PASS** |
| `DSD512` | 7 | +0.50 | 0.7500 | 1.0028 | 0 | **PASS** |
| `DSD64_48` | 7 | +0.50 | 0.7500 | 1.0095 | 0 | **PASS** |
| `DSD128_48` | 7 | +0.50 | 0.7500 | 1.0091 | 0 | **PASS** |
| `DSD256_48` | 7 | +0.50 | 0.7500 | 1.0082 | 0 | **PASS** |
| `DSD512_48` | 7 | +0.50 | 0.7500 | 1.0094 | 0 | **PASS** |

### High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak abs state | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
| `DSD64` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9259 | 0 | **PASS** |
| `DSD128` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9042 | 0 | **PASS** |
| `DSD256` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9153 | 0 | **PASS** |
| `DSD512` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.9196 | 0 | **PASS** |
| `DSD64` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9150 | 0 | **PASS** |
| `DSD128` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9030 | 0 | **PASS** |
| `DSD256` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9461 | 0 | **PASS** |
| `DSD512` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.9258 | 0 | **PASS** |

### Input limit L (compiled, per order and rate)
- `L` is `max_stable_input_dbfs` from the coefficient tables (generated by `design_crfb.measure_msa_dbfs`: DC and 50 Hz-20 kHz sines, 0.25 dB
  level steps, 2^21 samples, Python reference loop without dither). Here the **production** modulator (`process_tile_bytes`, TPDF dither on) runs
  at exactly `L` for the same stimuli, 2^21 bits each from a reset state. DSD targets get headroom `L - M` (M = 0.5 dB), so this is 0.5 dB above
  the level a 0 dBFS PCM peak reaches.
- **Criterion:** 0 overloads for every stimulus.

| Target Rate | Order | L (dBFS) | Headroom L - M (dB) | Worst max abs state | Overloads (DC, 50 Hz, 1k, 5k, 10k, 15k, 20k) | Verdict |
|---|:---:|---:|---:|---:|---|:---:|
| `dsd64` | 5 | -4.00 | -4.50 | 1.199 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 5 | -4.00 | -4.50 | 1.197 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 5 | -4.25 | -4.75 | 1.169 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 5 | -4.00 | -4.50 | 1.234 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 5 | -4.25 | -4.75 | 1.186 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 5 | -4.00 | -4.50 | 1.305 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 5 | -4.00 | -4.50 | 1.246 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 5 | -4.25 | -4.75 | 1.167 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64` | 7 | -4.50 | -5.00 | 1.264 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128` | 7 | -4.75 | -5.25 | 1.167 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256` | 7 | -4.75 | -5.25 | 1.256 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512` | 7 | -4.75 | -5.25 | 1.256 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd64_48` | 7 | -4.50 | -5.00 | 1.247 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd128_48` | 7 | -4.75 | -5.25 | 1.300 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd256_48` | 7 | -4.75 | -5.25 | 1.216 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |
| `dsd512_48` | 7 | -4.75 | -5.25 | 1.348 | 0, 0, 0, 0, 0, 0, 0 | **PASS** |

### Intentional Overload & Autonomous Recovery
- Injects a severe +5.0 step (2 tiles) into a 0.1-amplitude 1 kHz sine (timed at each target's own bit rate), triggering `OverloadPolicy::MuteRecover`.
- **Expected behavior:** the overload is recorded, integrator states are zeroed, the output falls back to the balanced DSD idle sequence (`0x69`) with no `0x00` bytes, and modulation resumes cleanly.
- **Clean resumption** is checked on the tail (after 4 settling tiles): no further overloads, no rail-lock bytes (`0x00` / `0xFF`), bounded states, and a low-passed reconstruction error
  (mean of the bits over 128-bit blocks against the mean input) of at most 1.5x that of a reference modulator that never overloaded (same seed, same sine).

| Target Rate | Order | Overloads (step / tail) | Idle 0x69 bytes in step | Tail rail bytes | Tail error rms | Reference error rms | Verdict |
|---|:---:|:---:|:---:|:---:|---:|---:|:---:|
| `DSD64` | 5 | 341 / 0 | 341 | 0 | 7.64e-03 | 7.46e-03 | **PASS** |
| `DSD128` | 5 | 341 / 0 | 341 | 0 | 7.34e-03 | 7.35e-03 | **PASS** |
| `DSD256` | 5 | 341 / 0 | 341 | 0 | 7.13e-03 | 7.51e-03 | **PASS** |
| `DSD512` | 5 | 341 / 0 | 341 | 0 | 7.35e-03 | 7.15e-03 | **PASS** |
| `DSD64` | 7 | 215 / 0 | 242 | 0 | 8.64e-03 | 8.48e-03 | **PASS** |
| `DSD128` | 7 | 215 / 0 | 215 | 0 | 7.81e-03 | 7.94e-03 | **PASS** |
| `DSD256` | 7 | 215 / 0 | 215 | 0 | 7.53e-03 | 7.64e-03 | **PASS** |
| `DSD512` | 7 | 215 / 0 | 242 | 0 | 7.66e-03 | 7.64e-03 | **PASS** |
