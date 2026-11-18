# audigma

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-2024%20(1.85%2B)-orange.svg)](https://www.rust-lang.org/)

**audigma** is an open-source Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and real-time PCM-to-1-bit Delta-Sigma Modulation (DSD). 

Inspired by foobar2000's `foo_dsd_processor`, audigma integrates with PipeWire and ALSA so Linux desktop applications (browsers, music players, streaming apps) and dedicated music players (such as MPD) can output high-rate PCM or 1-bit DSD (Native or DoP) directly to external DACs without manual device switching, rate lockouts, or audio pops.

> [!WARNING]
> **Active Development Status:** Audigma is in early active development and is not yet published on crates.io. Internal APIs, command-line arguments, and configuration schemas may evolve without backward compatibility guarantees.

---

## Why audigma?

Playing audiophile streams on a Linux desktop usually forces compromises:
- **Audio Server Rate Fighting:** Desktop sound servers (like PipeWire) often pin the DAC to a single clock rate (e.g. 48 kHz). Upsampling or DSD playback typically requires taking exclusive ALSA control of the DAC, causing `Device or resource busy` lockouts for all other desktop audio.
- **Hardware Relay Clicks & Pops:** Switching between audio sample rates or starting/stopping 1-bit DSD playback often sends all-zero PCM or DC thumps to DACs, causing loud analog relay clicks and acoustic pops.
- **DSD Software Volume Corruption:** Attempting software volume scaling or DSP on 1-bit DSD or DoP (DSD over PCM) bitstreams destroys the bitstream structure and emits deafening high-frequency noise.

**Audigma solves these challenges:**
1. **Dynamic PipeWire Virtual Sink:** Applications play into a virtual sink that automatically renegotiates rates and seamlessly bridges audio to exclusive hardware endpoints.
2. **D-Bus Device Reservation:** Audigma takes exclusive ownership of the target ALSA DAC when active via `org.freedesktop.ReserveDevice1` and releases it back to PipeWire/WirePlumber cleanly on stop or pause.
3. **Zero Pops & Safe Transitions:** Planned transitions ramp floating-point PCM to zero before modulation, wait for filter group delays to clear, and emit balanced DSD idle silence (`0x69`) while maintaining DoP marker alternation.
4. **Pre-Modulator Unified Volume:** Volume adjustments occur in high-precision `f64` PCM *before* modulation and FIR interpolation, or synchronize directly with the DAC's analog/hardware mixer. Packed DSD bits are never touched as PCM.
5. **Measured, Not Assumed:** Multistage polyphase sinc and half-band FIR cascades are designed for $\ge 140\text{ dB}$ image rejection, and every filter and modulator setting is verified by an automated, deterministic spectral suite against thresholds derived from audibility and 24-bit transparency. The thresholds, and the one documented exception (DSD64 with the default 5th-order modulator), are in [Design.md §8](Design.md); the reports are in [docs/measurements/](docs/measurements/).

---

## Key Features

- **Live Streaming Daemon (`audigma run`):** Background real-time audio service bridging PipeWire virtual sinks to high-rate PCM or exclusive ALSA DSD (Native `DSD_U32` or DoP v1.1 up to DSD512).
- **Offline File Converter (`audigma convert`):** High-throughput offline batch converter (WAV, FLAC, DSF $\to$ DSF, high-rate PCM, or raw DoP) with two-pass true-peak headroom analysis and album-continuous gapless processing.
- **Direct Playback (`audigma play`):** Plays audio files through the configured DSP pipeline, straight to an exclusive ALSA device or back into the PipeWire graph.
- **Interactive Configurator (`audigma configure`):** Guided configuration editor available as both a terminal TUI (`audigma configure tui`) and a localhost Web UI (`audigma configure web`).
- **Real-Time Telemetry & Health (`audigma status`):** Live monitoring of buffer fills, clock drift compensation (ASRC $\delta_{\text{ppm}}$), modulator operating envelopes, and per-worker real-time execution budgets.
- **System Tray Integration (Linux):** Optional desktop StatusNotifierItem for volume control, muting, and daemon state inspection.

---

## Building & Installation

### Prerequisites (Linux)
Building Audigma requires a Rust toolchain (1.85+), `pkg-config`, the ALSA, PipeWire and SPA development headers, and `libclang` (the PipeWire bindings are generated at build time). If something is missing, Cargo's build errors name the library it could not find on your host.

On Debian (tested on a clean `debian:stable`), that is:

```bash
sudo apt install build-essential pkg-config libclang-dev libasound2-dev libpipewire-0.3-dev libspa-0.2-dev
```

Other distributions package the same pieces under their own names.

### Compiling
Audigma compiles with Cargo out of the box:

```bash
git clone https://github.com/benedictjohannes/audigma.git
cd audigma
cargo build --release
```

The resulting binary is located at `./target/release/audigma`.

> [!TIP]
> **No Node or Bun Required for Base Builds:** `cargo build` compiles the full audio engine, daemon, CLI, and the complete terminal TUI configurator (`audigma configure tui`) without any JavaScript tooling. [Bun](https://bun.sh/) is only required if you wish to rebuild the optional Web UI frontend (`crates/audigma-configurator/web`).

---

## Quick Start

### 1. Configure Your Setup
Launch the interactive Terminal UI configurator:
```bash
./target/release/audigma configure
```
The configurator will probe your audio hardware, recommend configurations based on your use case (desktop listening, headless appliance, or offline conversion), and write a validated `config.toml`.

### 2. Run the Daemon
Start the live streaming service:
```bash
./target/release/audigma run
```
Audigma creates a PipeWire virtual sink (e.g. `audigma_sink_default`). Route your desktop audio or music player (e.g. MPD) into this sink.

### 3. Check Live Status
Inspect live stream rates, volume, and DSP health:
```bash
./target/release/audigma status
```

---

## Documentation

- **[Design.md](Design.md):** The authoritative technical specification describing audio math, formulas, filter cascades, CRFB modulator topologies, clock domains, and configuration schema.
- **[docs/backends/PipeWire.md](docs/backends/PipeWire.md):** Virtual sink details, quantum scaling, rate negotiation, and volume synchronization.
- **[docs/backends/ALSA.md](docs/backends/ALSA.md):** Exclusive hardware output, D-Bus device reservation, DoP word negotiation, and hardware mixer control.
- **[docs/Telemetry.md](docs/Telemetry.md):** Real-time health monitoring fields, loop timing metrics, and log event contracts.
- **[docs/measurements/](docs/measurements/):** Automated spectral verification reports across modulators, filter cascades, resamplers, and production signal chains.
- **[docs/Roadmap.md](docs/Roadmap.md):** Planned features that are not implemented yet.
- **[CONTRIBUTING.md](CONTRIBUTING.md):** Development guidelines, verification standards, and pull request workflows.

---

## License

Audigma is dual-licensed under either:
- **MIT License** ([LICENSE-MIT](LICENSE-MIT))
- **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE))

at your option.
