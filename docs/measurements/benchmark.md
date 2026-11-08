# audigma — Benchmark Measurements

> **Generated:** `2026-11-08 10:57:08Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-08 10:57:08Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 52.9 | 81.9 | **5.6%** | **27.43x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 62.5 | 90.2 | **6.2%** | **23.21x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 101.9 | 133.9 | **9.2%** | **14.24x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 111.4 | 148.7 | **10.2%** | **13.03x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 198.4 | 265.7 | **18.3%** | **7.31x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 207.5 | 261.4 | **18.0%** | **6.99x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 194.8 | 241.7 | **33.3%** | **3.72x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 199.4 | 243.0 | **33.5%** | **3.64x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 58.4 | 83.8 | **5.8%** | **24.82x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 67.8 | 96.8 | **6.7%** | **21.38x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 113.5 | 156.5 | **10.8%** | **12.78x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 122.5 | 164.3 | **11.3%** | **11.85x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 220.4 | 265.3 | **18.3%** | **6.58x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 229.7 | 278.8 | **19.2%** | **6.32x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 217.0 | 273.8 | **37.7%** | **3.34x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 222.0 | 289.8 | **39.9%** | **3.27x** | **PASS** |
