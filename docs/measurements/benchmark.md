# audigma — Benchmark Measurements

> **Generated:** `2026-11-10 00:36:46Z`  
> **Evaluated on:** `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`), analysed by `tools/measure/measure_spectral.py`.  
> **Benchmark placement:** `cpu [0], highest_perf 208` (measured `2026-11-10 00:36:46Z`)  
> **Index:** [report.md](report.md)

CPU throughput and real-time deadline budget (pinned like a runtime DSP worker).

---

## Host CPU Throughput & Real-Time Deadline Budget

Benchmark parameters: 20,000 blocks per configuration (p99.9 = 20th-worst block) of 64 baseband frames (1.451 ms audio duration at 44.1 kHz; 32 frames / 725.6 us for DSD512). Process pinned via the runtime placement resolver (`cpu [0], highest_perf 208`), normal scheduling class.

### 5th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 39.4 | 89.5 | **6.2%** | **36.80x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 50.0 | 109.9 | **7.6%** | **28.98x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 74.0 | 116.2 | **8.0%** | **19.60x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 83.1 | 127.5 | **8.8%** | **17.46x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 143.7 | 221.5 | **15.3%** | **10.09x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 152.8 | 228.2 | **15.7%** | **9.50x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 142.2 | 199.1 | **27.4%** | **5.10x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 146.5 | 222.1 | **30.6%** | **4.95x** | **PASS** |

### 7th-Order Modulator Benchmarks

| Target Rate | Filter | Audio Block Time (us) | Avg Latency (us) | p99.9 Latency (us) | p99.9 Budget Used | Throughput (x Real-Time) | Goal (>= 2x, <= 70%) |
|---|---|---:|---:|---:|---:|---:|:---:|
| `DSD64` | apodization off | 1451.2 | 41.6 | 73.5 | **5.1%** | **34.85x** | **PASS** |
| `DSD64` | apodization 20000 | 1451.2 | 50.3 | 90.5 | **6.2%** | **28.81x** | **PASS** |
| `DSD128` | apodization off | 1451.2 | 77.5 | 129.0 | **8.9%** | **18.70x** | **PASS** |
| `DSD128` | apodization 20000 | 1451.2 | 87.0 | 131.5 | **9.1%** | **16.68x** | **PASS** |
| `DSD256` | apodization off | 1451.2 | 150.7 | 215.8 | **14.9%** | **9.63x** | **PASS** |
| `DSD256` | apodization 20000 | 1451.2 | 159.6 | 242.7 | **16.7%** | **9.09x** | **PASS** |
| `DSD512` | apodization off | 725.6 | 148.7 | 223.5 | **30.8%** | **4.88x** | **PASS** |
| `DSD512` | apodization 20000 | 725.6 | 153.4 | 213.6 | **29.4%** | **4.73x** | **PASS** |
