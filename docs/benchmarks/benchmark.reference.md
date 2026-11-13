# audigma — Benchmark Measurements

> **Generated:** `2026-11-13 22:52:42Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.100.0 (0ef1bc386 2026-11-09)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/benchmark/benchmark.py`  
> **Tag:** `reference` (Official Reference Baseline)  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-13 22:52:42Z`)  
> **DSP Quality Report:** [../measurements/README.md](../measurements/README.md)

## What the numbers show

- CPU throughput and real-time deadline budget.
- Pinned like a runtime DSP worker (`cpu [0], highest_perf 208`, normal scheduling class).
- Each configuration runs 20,000 timed blocks; p99.9 is the 20th-worst block.
- Every measurement is a single channel on a single core.

The benchmark excludes processing outside the DSP audio pipeline, such as:
- Worker hand-off between threads.
- DoP packing and sample format conversion.
- Audio I/O and transport overhead.

## The Target Headroom: Stereo on One Core

Most listening is stereo, and stereo on a single core takes twice the mono time. The target is defined for stereo on one core:

> **at least 2x real-time headroom** with a **p99.9 block time of at most 70% of the deadline budget**.

For mono measurements, the thresholds below determine the PASS/FAIL verdicts and marks:
- **Throughput:** at least **4x** real-time
- **p99.9 Budget Used:** at most **35%** of the block deadline

In runtime, audigma can utilize multiple DSP workers, where channels run in parallel on separate cores. As such, the single-core measurement shown below represents the conservative worst case.

---

## Modulator Benchmarks

This section shows the modulator-only benchmark: the half-band cascade (`HalfBandCascade`, driven directly), followed by the CRFB delta-sigma modulator, measured on one channel with a synthetic tone.

The modulator applies only to DSD output and constitutes the heaviest stage of the DSP chain; it excludes DSD input decimation and rational sample rate conversion.

- **Benchmark Parameters:**
  - *Tile Size:* Blocks of 256 baseband frames (5.805 ms audio duration at 44.1 kHz), matching the production tile.
  - *Subtiles:* Executed in the worker's cache-sized subtiles (256 frames at DSD64, 128 at DSD128, 64 at DSD256, 32 at DSD512).
  - *Block Budget:* Audio duration of the tile.

### 5th-Order

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 4x, <= 35%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 5805.0 | 122.1 | 166.4 | **2.9%** | **47.53x** | **PASS** |
| `DSD64` | apodization 20000 | 5805.0 | 124.4 | 161.8 | **2.8%** | **46.64x** | **PASS** |
| `DSD128` | apodization off | 5805.0 | 241.3 | 328.2 | **5.7%** | **24.05x** | **PASS** |
| `DSD128` | apodization 20000 | 5805.0 | 244.5 | 330.9 | **5.7%** | **23.74x** | **PASS** |
| `DSD256` | apodization off | 5805.0 | 480.0 | 629.3 | **10.8%** | **12.09x** | **PASS** |
| `DSD256` | apodization 20000 | 5805.0 | 483.4 | 613.3 | **10.6%** | **12.01x** | **PASS** |
| `DSD512` | apodization off | 5805.0 | 962.6 | 1172.3 | **20.2%** | **6.03x** | **PASS** |
| `DSD512` | apodization 20000 | 5805.0 | 964.0 | 1143.5 | **19.7%** | **6.02x** | **PASS** |

### 7th-Order

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 4x, <= 35%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 5805.0 | 123.7 | 160.7 | **2.8%** | **46.92x** | **PASS** |
| `DSD64` | apodization 20000 | 5805.0 | 126.5 | 180.0 | **3.1%** | **45.88x** | **PASS** |
| `DSD128` | apodization off | 5805.0 | 244.8 | 323.6 | **5.6%** | **23.71x** | **PASS** |
| `DSD128` | apodization 20000 | 5805.0 | 247.4 | 324.6 | **5.6%** | **23.46x** | **PASS** |
| `DSD256` | apodization off | 5805.0 | 487.4 | 634.0 | **10.9%** | **11.91x** | **PASS** |
| `DSD256` | apodization 20000 | 5805.0 | 489.9 | 609.4 | **10.5%** | **11.85x** | **PASS** |
| `DSD512` | apodization off | 5805.0 | 981.9 | 1360.9 | **23.4%** | **5.91x** | **PASS** |
| `DSD512` | apodization 20000 | 5805.0 | 980.8 | 1183.4 | **20.4%** | **5.92x** | **PASS** |

---

## Full Pipeline Benchmarks

This section shows the production worker benchmark as executed by `audigma run` on a single calling thread (one channel, no worker hand-off), using blocks of 256 baseband frames matching the live loop.

Per audio block, the pipeline executes the following stages:
- **DSD Unpack & Decimation:** Raw DSD decoded to its intermediate PCM rate (176.4 kHz for DSD64). *Decimation is not applied for PCM input.*
- **Rational SRC:** Sample rate conversion applied when rate families differ or the intermediate rate differs from the target (e.g., 44.1 kHz or DSD64 to PCM 384 kHz, or 96 kHz to DSD256).
- **Half-Band Cascade:** Multi-stage upsampling cascade.
- **Modulator:** CRFB delta-sigma modulator for DSD output. *Modulator is not applied for PCM output.*

The two tables below evaluate the lightest and heaviest combinations of the pipeline:
- Each cell shows **p99.9 budget used / throughput (x real-time)**.
- A trailing **✗** marks a cell that misses the mono goal (>= 4x, <= 35%).

### Linear Phase, Apodization Off, (DSD output) 5th-Order Modulator

| Input | Block Budget (us) | PCM 384 kHz | DSD256 | DSD512 |
|---|---:|---:|---:|---:|
| PCM 44.1 kHz | 5805.0 | **0.5% / 600.97x** | **10.1% / 12.07x** | **19.8% / 6.03x** |
| PCM 96 kHz | 2666.7 | **0.2% / 1140.11x** | **11.4% / 11.68x** | **21.2% / 5.95x** |
| DSD64 | 1451.2 | **2.1% / 108.62x** | **11.7% / 11.38x** | **21.4% / 5.88x** |

### Minimum Phase, Apodization 20kHz, (DSD output) 7th-Order Modulator

| Input | Block Budget (us) | PCM 384 kHz | DSD256 | DSD512 |
|---|---:|---:|---:|---:|
| PCM 44.1 kHz | 5805.0 | **0.6% / 400.21x** | **10.9% / 11.41x** | **23.8% / 5.73x** |
| PCM 96 kHz | 2666.7 | **0.4% / 492.17x** | **11.8% / 10.85x** | **23.4% / 5.61x** |
| DSD64 | 1451.2 | **2.5% / 84.82x** | **13.1% / 10.47x** | **22.9% / 5.51x** |
