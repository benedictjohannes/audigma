# audigma — Benchmark Measurements

> **Generated:** `2026-11-10 03:43:49Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-10 03:43:49Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 30.0 | 69.4 | **4.8%** | **48.31x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 30.6 | 61.3 | **4.2%** | **47.39x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 59.1 | 111.1 | **7.7%** | **24.54x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 59.8 | 108.1 | **7.5%** | **24.26x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 117.7 | 181.1 | **12.5%** | **12.33x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 118.5 | 187.3 | **12.9%** | **12.24x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 117.5 | 182.4 | **25.1%** | **6.17x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 117.9 | 193.1 | **26.6%** | **6.15x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 32.4 | 77.8 | **5.4%** | **44.73x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 33.7 | 89.0 | **6.1%** | **42.97x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 63.8 | 121.1 | **8.3%** | **22.74x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 64.6 | 124.2 | **8.6%** | **22.45x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 127.0 | 200.6 | **13.8%** | **11.43x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 127.2 | 191.9 | **13.2%** | **11.40x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 125.7 | 186.2 | **25.7%** | **5.77x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 127.0 | 200.7 | **27.7%** | **5.71x** | **PASS** |
