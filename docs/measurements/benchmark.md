# audigma — Benchmark Measurements

> **Generated:** `2026-11-10 02:28:14Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-10 02:28:14Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 37.4 | 59.4 | **4.1%** | **38.79x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 46.5 | 69.6 | **4.8%** | **31.20x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 70.2 | 97.5 | **6.7%** | **20.66x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 79.2 | 104.4 | **7.2%** | **18.31x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 135.8 | 180.1 | **12.4%** | **10.68x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 144.9 | 184.5 | **12.7%** | **10.01x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 133.5 | 171.7 | **23.7%** | **5.43x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 138.3 | 189.4 | **26.1%** | **5.25x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 39.7 | 70.3 | **4.8%** | **36.51x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 48.8 | 76.7 | **5.3%** | **29.74x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 74.5 | 102.1 | **7.0%** | **19.46x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 83.6 | 117.4 | **8.1%** | **17.35x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 144.4 | 202.6 | **14.0%** | **10.04x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 153.5 | 200.8 | **13.8%** | **9.45x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 142.1 | 185.1 | **25.5%** | **5.11x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 146.7 | 196.2 | **27.0%** | **4.95x** | **PASS** |
