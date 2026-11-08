# audigma — Benchmark Measurements

> **Generated:** `2026-11-08 09:52:55Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-08 09:52:55Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 54.1 | 78.2 | **5.4%** | **26.82x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 63.1 | 84.9 | **5.8%** | **23.00x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 102.1 | 143.7 | **9.9%** | **14.20x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 111.1 | 143.1 | **9.9%** | **13.05x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 194.4 | 231.5 | **15.9%** | **7.46x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 203.4 | 240.9 | **16.6%** | **7.13x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 188.6 | 233.1 | **32.1%** | **3.85x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 193.2 | 238.5 | **32.9%** | **3.76x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 58.3 | 80.8 | **5.6%** | **24.88x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 67.4 | 93.0 | **6.4%** | **21.52x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 111.7 | 140.9 | **9.7%** | **12.99x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 120.8 | 155.1 | **10.7%** | **12.01x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 216.3 | 307.7 | **21.2%** | **6.71x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 225.7 | 288.9 | **19.9%** | **6.43x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 213.1 | 275.8 | **38.0%** | **3.41x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 217.2 | 258.7 | **35.6%** | **3.34x** | **PASS** |
