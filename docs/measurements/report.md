# audigma — DSP Quality Report

> **Authoritative specification:** `Design.md §4` and `§8`.  
> **Generated:** `2026-11-08 11:39:19Z` on `AMD Ryzen AI 7 350 w/ Radeon 860M (x86_64)` running `CachyOS (Linux 7.3.1-1-cachyos)`  
> **Rust compiler:** `rustc 1.98.0 (88d9e12ae 2026-08-18)`  
> **Hardware FTZ/DAZ:** `Active (hardware-verified)`  
> **Benchmark:** last run `2026-11-08 10:57:08Z`, placement `cpu [0], highest_perf 208`  
> **Harness:** `crates/audigma-measurements` (`measure_harness`) analysed by `tools/measure/measure_spectral.py`.

This index lists the per-family reports and the invariant check. Filter and resampler quality figures are reported
**without pass/fail thresholds**; thresholds are derived from stated requirements in the DSP quality audit's threshold step.

## Reports

| Report | Contents | Headline |
|---|---|---|
| [Modulator](modulator.md) | CRFB modulators: direct-sine verification against the synthesis oracle, stability envelope. | Stable in all stress cases; DSD64 in-band SNR 115 dB (5th) / 134 dB (7th); 0 failing check(s) |
| [Cascade](cascade.md) | Half-band interpolation cascade as implemented (impulse response, tones) and cascade + modulator end to end. | Worst image rejection 145.8 dB over 60 filter/stage combinations; 0 failing check(s) |
| [Resampler](resampler.md) | Polyphase rational SRC (44.1k <-> 48k families) and ASRC (clock-drift tracking). | Worst spur on 1-18 kHz tones: 44.1k->48k -146 dBc, 48k->44.1k -157 dBc, ASRC at +50 ppm -146 dBc |
| [Signal](signal.md) | Production DSD chain (gain L - M, cascade, dithered modulator): level sweep, dither, IMD, ultrasonic noise, rate families. | Production chain: DSD64 DR (A) 118 dB (5th) / 137 dB (7th); 0 failing check(s) |
| [Benchmark](benchmark.md) | CPU throughput and real-time deadline budget (pinned like a runtime DSP worker). | 3.3x to 27.4x real-time; p99.9 up to 39.9% of budget (last run 2026-11-08 10:57:08Z) |

## Invariants & Compliance Audit

| Requirement / Invariant | Canonical Reference | Measured / Verified Value | Status |
|---|---|---|:---:|
| **Baseband OSR Calculation** | `Design.md §4.1` | 44.1k: 70.56, 141.12, 282.24, 564.48 | 48k: 76.80, 153.60, 307.20, 614.40 (f_bit / 40 kHz) | Compliant |
| **Cascade Image Rejection (implemented, all settings, all K)** | `Design.md §4.2` | 145.8 dB worst over all phases, apodizations and K (goal >= 140 dB); passband deviation with `off` 2.0e-06 dB (fixed by the rejection, no separate goal) | Compliant |
| **Modulator Stability Envelope** | `Design.md §4.3` | Bounded states across silence, DC (+0.5), multitone, step overload recovery | Compliant |
| **DSD Silence Byte Invariant** | `Design.md §1.3` | 0x69 idle pattern confirmed (2,278 clamp bytes); 0 rail-lock bytes (0x00/0xFF) across 16,777,216 silence bits | Compliant |
| **Hardware FTZ/DAZ** | `Design.md §4.1` | Active (hardware-verified) | Compliant |
| **Real-Time Throughput** | `Design.md §8.5` | >= 2.0x sustained real-time throughput (Measured: 3.27x to 27.43x) | Compliant |
| **p99.9 Deadline Budget** | `Design.md §8.5` | <= 70% of audio block duration (Measured peak: 39.9%) | Compliant |

## Conclusion

7 of 7 invariants verified on the compiled code. Method checks (cascade reproduces the design response, tones agree with the impulse response, resampler tables equal the generator output) are part of the family reports and listed below if any failed.

Filter-cascade and resampler *quality* (flatness, image/alias levels, ASRC spurs) is reported in [cascade.md](cascade.md) and [resampler.md](resampler.md) without thresholds; whether it is good enough is decided in audit step 5.
