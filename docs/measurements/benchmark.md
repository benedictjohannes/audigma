# audigma — Benchmark Measurements

> **Generated:** `2026-11-10 03:08:12Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-10 03:08:12Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 30.2 | 59.0 | **4.1%** | **47.92x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 32.2 | 52.4 | **3.6%** | **45.01x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 59.1 | 95.0 | **6.5%** | **24.53x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 61.0 | 85.5 | **5.9%** | **23.79x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 116.3 | 154.8 | **10.7%** | **12.48x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 118.4 | 152.7 | **10.5%** | **12.25x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 115.8 | 142.9 | **19.7%** | **6.27x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 116.9 | 149.6 | **20.6%** | **6.20x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 32.5 | 57.5 | **4.0%** | **44.55x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 34.4 | 59.3 | **4.1%** | **42.10x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 64.5 | 113.7 | **7.8%** | **22.48x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 65.4 | 102.0 | **7.0%** | **22.17x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 125.4 | 204.6 | **14.1%** | **11.57x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 128.5 | 199.6 | **13.8%** | **11.28x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 124.5 | 183.1 | **25.2%** | **5.83x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 125.4 | 176.5 | **24.3%** | **5.78x** | **PASS** |
