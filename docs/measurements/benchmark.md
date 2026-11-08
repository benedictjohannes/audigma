# audigma — Benchmark Measurements

> **Generated:** `2026-11-08 12:08:18Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-08 12:08:18Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 55.0 | 92.6 | **6.4%** | **26.38x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 64.1 | 91.4 | **6.3%** | **22.64x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 104.0 | 147.3 | **10.2%** | **13.95x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 113.3 | 166.7 | **11.5%** | **12.81x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 198.1 | 241.8 | **16.7%** | **7.33x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 207.2 | 252.0 | **17.4%** | **7.00x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 192.3 | 239.6 | **33.0%** | **3.77x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 198.1 | 272.4 | **37.5%** | **3.66x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 59.5 | 84.1 | **5.8%** | **24.36x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 68.9 | 97.8 | **6.7%** | **21.05x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 114.0 | 150.7 | **10.4%** | **12.72x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 123.3 | 155.8 | **10.7%** | **11.77x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 220.7 | 265.6 | **18.3%** | **6.57x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 230.2 | 288.8 | **19.9%** | **6.30x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 217.3 | 264.7 | **36.5%** | **3.34x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 222.1 | 280.3 | **38.6%** | **3.27x** | **PASS** |
