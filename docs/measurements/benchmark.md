# audigma — Benchmark Measurements

> **Generated:** `2026-11-08 06:54:48Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-08 05:53:39Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Preset | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `Balanced` | 1451.2 | 57.1 | 85.2 | **5.9%** | **25.40x** | **PASS** |
| `DSD64` | `Reference` | 1451.2 | 59.3 | 80.7 | **5.6%** | **24.46x** | **PASS** |
| `DSD128` | `Balanced` | 1451.2 | 113.7 | 142.2 | **9.8%** | **12.76x** | **PASS** |
| `DSD128` | `Reference` | 1451.2 | 118.0 | 154.0 | **10.6%** | **12.30x** | **PASS** |
| `DSD256` | `Balanced` | 1451.2 | 225.5 | 268.1 | **18.5%** | **6.43x** | **PASS** |
| `DSD256` | `Reference` | 1451.2 | 233.6 | 274.0 | **18.9%** | **6.21x** | **PASS** |
| `DSD512` | `Balanced` | 725.6 | 224.9 | 290.6 | **40.0%** | **3.23x** | **PASS** |
| `DSD512` | `Reference` | 725.6 | 232.6 | 280.4 | **38.6%** | **3.12x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Preset | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | `Balanced` | 1451.2 | 62.6 | 90.0 | **6.2%** | **23.16x** | **PASS** |
| `DSD64` | `Reference` | 1451.2 | 64.9 | 100.2 | **6.9%** | **22.36x** | **PASS** |
| `DSD128` | `Balanced` | 1451.2 | 125.0 | 159.6 | **11.0%** | **11.61x** | **PASS** |
| `DSD128` | `Reference` | 1451.2 | 128.9 | 156.1 | **10.8%** | **11.25x** | **PASS** |
| `DSD256` | `Balanced` | 1451.2 | 247.8 | 291.6 | **20.1%** | **5.86x** | **PASS** |
| `DSD256` | `Reference` | 1451.2 | 256.4 | 315.2 | **21.7%** | **5.66x** | **PASS** |
| `DSD512` | `Balanced` | 725.6 | 247.2 | 298.2 | **41.1%** | **2.93x** | **PASS** |
| `DSD512` | `Reference` | 725.6 | 255.9 | 331.9 | **45.7%** | **2.84x** | **PASS** |
