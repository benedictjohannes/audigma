# audigma — Modulator Measurements

> **Generated:** `2026-11-08 06:54:29Z`  
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
| **Stable Operating Limit (MSA)** | -3.8 to -3.2 dBFS | -4.0 to -3.8 dBFS | — |
| **DSD64 (OSR ~70.6)** | **112.48 dB** (Oracle: 113.1 dB) | **134.07 dB** (Oracle: 134.4 dB) | 7th-order higher SNR by 22 dB |
| **DSD128 (OSR ~141.1)** | **145.73 dB** (Oracle: 146.4 dB) | **179.10 dB** (Oracle: 180.2 dB) | 7th-order higher SNR by 33 dB |
| **DSD256 (OSR ~282.2)** | **179.27 dB** (Oracle: 180.6 dB) | **224.51 dB** (Oracle: 226.2 dB) | 7th-order higher SNR by 45 dB |
| **DSD512 (OSR ~564.5)** | **213.10 dB** (Oracle: 220.7 dB) | **270.30 dB** (Oracle: 273.4 dB) | 7th-order higher SNR by 57 dB |

> [!NOTE]
> All measurements in this report evaluate the **compiled Rust DSP code** against live theoretical oracle synthesis from `tools/design/design_crfb.py`.


---

## Direct Modulator Verification (Theoretical Oracle vs. Compiled Bitstream)

Test conditions: Direct coherent 1 kHz sine at -6.0 dBFS (f_tone = k * f_bit / N), N = 1,048,576 samples (2^20), evaluating peak modulator dynamic range and noise-shaping NTF depth against the live mathematical oracle.

### 5th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x5 | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 113.1 dB | **112.48 dB** | -0.6 dB | 139 dB | 111.56 | -118.75 | -24.97 dB | -132.7 | 0.9281 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 146.4 dB | **145.73 dB** | -0.7 dB | 177 dB | 144.16 | -149.32 | -53.60 dB | -162.5 | 0.8932 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 180.6 dB | **179.27 dB** | -1.3 dB | 206 dB | 176.36 | -179.46 | -87.02 dB | -193.1 | 0.9075 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 220.7 dB | **213.10 dB** | -7.6 dB | 241 dB | 207.96 | -209.55 | -120.12 dB | -223.2 | 0.9255 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 117.3 dB | **116.82 dB** | -0.5 dB | 159 dB | 115.83 | -122.71 | -27.00 dB | -136.0 | 0.8813 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 151.0 dB | **149.72 dB** | -1.2 dB | 176 dB | 147.98 | -152.79 | -57.64 dB | -165.9 | 0.8887 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 184.8 dB | **183.43 dB** | -1.4 dB | 216 dB | 180.31 | -183.21 | -91.08 dB | -197.4 | 0.9555 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 221.9 dB | **217.12 dB** | -4.7 dB | 253 dB | 211.62 | -213.05 | -124.52 dB | -228.5 | 0.9345 | yes | **PASS** |

### 7th-Order CRFB Modulator

| Target Rate | Family | f_bit (Hz) | OSR | Tone (dBFS) | Theoretical Oracle SNR | Measured In-Band SNR | Delta | Analysis floor | SINAD (dB) | THD (dB) | Ultrasonic 20–100 kHz | Worst spur (dBc) | Max abs x7 | Bit-exact vs reference | Verdict |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 44.1k | 2,822,400 | 70.6 | -9.01 | 134.4 dB | **134.07 dB** | -0.3 dB | 150 dB | 133.31 | -141.26 | -20.00 dB | -150.0 | 0.9039 | yes | **PASS** |
| `DSD128` | 44.1k | 5,644,800 | 141.1 | -9.01 | 180.2 dB | **179.10 dB** | -1.1 dB | 201 dB | 177.89 | -184.03 | -46.01 dB | -191.6 | 0.8782 | yes | **PASS** |
| `DSD256` | 44.1k | 11,289,600 | 282.2 | -9.01 | 226.2 dB | **224.51 dB** | -1.7 dB | 242 dB | 222.11 | -225.83 | -91.14 dB | -234.4 | 0.8580 | yes | **PASS** |
| `DSD512` | 44.1k | 22,579,200 | 564.5 | -9.01 | 273.4 dB | **270.30 dB** | -3.1 dB | 281 dB | 266.15 | -268.25 | -136.35 dB | -280.3 | 0.9052 | yes | **PASS** |
| `DSD64_48` | 48k | 3,072,000 | 76.8 | -9.01 | 139.9 dB | **139.74 dB** | -0.1 dB | 172 dB | 138.95 | -146.73 | -20.70 dB | -155.0 | 0.9169 | yes | **PASS** |
| `DSD128_48` | 48k | 6,144,000 | 153.6 | -9.01 | 185.4 dB | **184.89 dB** | -0.5 dB | 201 dB | 183.44 | -188.89 | -51.46 dB | -198.6 | 0.8581 | yes | **PASS** |
| `DSD256_48` | 48k | 12,288,000 | 307.2 | -9.01 | 233.7 dB | **230.18 dB** | -3.5 dB | 253 dB | 227.47 | -230.80 | -96.90 dB | -242.8 | 0.8919 | yes | **PASS** |
| `DSD512_48` | 48k | 24,576,000 | 614.4 | -9.01 | 266.0 dB | **>= 272 dB** | — | 282 dB | 271.45 | -273.80 | -142.57 dB | -281.8 | 0.9291 | yes | **PASS** |

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
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0018, 0.0200, 0.1199]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0021, 0.0236, 0.1633]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0149, 0.0991]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0002, 0.0050, 0.0638]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0019, 0.0192, 0.1231]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0001, 0.0019, 0.0222, 0.1777]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0009, 0.0151, 0.1086]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0008, 0.0141, 0.1013]` | 0 | **PASS** |

#### 7th-Order CRFB Modulator
| Target Rate | Ones Ratio | Rail Locks (0x00 / 0xFF) | Max State Vector [x_1 ... x_7] | Overload Count | Verdict |
|---|---:|:---:|---|:---:|:---:|
| `DSD64` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0011, 0.0108, 0.0685, 0.2719]` | 0 | **PASS** |
| `DSD128` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0087, 0.0517, 0.2329]` | 0 | **PASS** |
| `DSD256` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0012, 0.0110, 0.0648, 0.2476]` | 0 | **PASS** |
| `DSD512` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0012, 0.0111, 0.0650, 0.2421]` | 0 | **PASS** |
| `DSD64_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0087, 0.0537, 0.2315]` | 0 | **PASS** |
| `DSD128_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0010, 0.0086, 0.0508, 0.2251]` | 0 | **PASS** |
| `DSD256_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0011, 0.0102, 0.0648, 0.2541]` | 0 | **PASS** |
| `DSD512_48` | 0.5000 | 0 / 0 | `[0.0000, 0.0000, 0.0001, 0.0012, 0.0108, 0.0618, 0.2558]` | 0 | **PASS** |

### Positive DC Offset (+0.5 Normalized)
- Evaluates behavior under sustained DC bias within the stable operating envelope.

| Target Rate | Order | Input DC | Ones Ratio | Peak State (xn) | Overload Count | Verdict |
|---|:---:|---:|---:|---:|:---:|:---:|
| `DSD64` | 5 | +0.50 | 0.7500 | 0.9249 | 0 | **PASS** |
| `DSD128` | 5 | +0.50 | 0.7500 | 0.9489 | 0 | **PASS** |
| `DSD256` | 5 | +0.50 | 0.7500 | 0.9324 | 0 | **PASS** |
| `DSD512` | 5 | +0.50 | 0.7500 | 0.9249 | 0 | **PASS** |
| `DSD64_48` | 5 | +0.50 | 0.7500 | 0.9116 | 0 | **PASS** |
| `DSD128_48` | 5 | +0.50 | 0.7500 | 0.9278 | 0 | **PASS** |
| `DSD256_48` | 5 | +0.50 | 0.7500 | 0.9179 | 0 | **PASS** |
| `DSD512_48` | 5 | +0.50 | 0.7500 | 0.9445 | 0 | **PASS** |
| `DSD64` | 7 | +0.50 | 0.7500 | 0.8839 | 0 | **PASS** |
| `DSD128` | 7 | +0.50 | 0.7500 | 0.8472 | 0 | **PASS** |
| `DSD256` | 7 | +0.50 | 0.7500 | 0.8815 | 0 | **PASS** |
| `DSD512` | 7 | +0.50 | 0.7500 | 0.8166 | 0 | **PASS** |
| `DSD64_48` | 7 | +0.50 | 0.7500 | 0.8960 | 0 | **PASS** |
| `DSD128_48` | 7 | +0.50 | 0.7500 | 0.8438 | 0 | **PASS** |
| `DSD256_48` | 7 | +0.50 | 0.7500 | 0.8406 | 0 | **PASS** |
| `DSD512_48` | 7 | +0.50 | 0.7500 | 0.8327 | 0 | **PASS** |

### High-Crest Multitone Bursts
- Sum of simultaneous audio frequencies with peak amplitude within -6.0 dBFS.

| Target Rate | Order | Test Description | Peak State (xn) | Overload Count | Verdict |
|---|:---:|---|---:|:---:|:---:|
| `DSD64` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7680 | 0 | **PASS** |
| `DSD128` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7876 | 0 | **PASS** |
| `DSD256` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.6747 | 0 | **PASS** |
| `DSD512` | 5 | 10-Tone Composite (100Hz–18kHz) | 0.7470 | 0 | **PASS** |
| `DSD64` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7516 | 0 | **PASS** |
| `DSD128` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7146 | 0 | **PASS** |
| `DSD256` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.7041 | 0 | **PASS** |
| `DSD512` | 7 | 10-Tone Composite (100Hz–18kHz) | 0.6739 | 0 | **PASS** |

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
| `DSD256` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (215 bytes) | Yes (States Bounded) | **PASS** |
| `DSD512` | 7 | +5.0 (step overload) | Yes (216 events) | Yes (242 bytes) | Yes (States Bounded) | **PASS** |
