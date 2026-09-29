# audigma — Technical Specification & Architecture (v2.2)

`audigma` is an open-source Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD). Inspired by foobar2000's `foo_dsd_processor`, it integrates with PipeWire and ALSA so Linux desktop applications and dedicated music players (such as MPD) can output high-rate PCM (back into PipeWire or directly to ALSA) or 1-bit DSD (Native or DoP via exclusive ALSA) without manual device switching.

---

## 1. Architectural Decisions & Core Invariants

In `audigma`, architectural decisions are enforced directly by the Rust type system and pipeline boundaries:

1. **Strongly Typed Rate Domains (`audigma-dsp`):**
   - Never pass raw `u32` integers for rates across module boundaries.
   - Use distinct types: `PcmRateHz` (e.g., `44_100`), `DsdBitRateHz` (e.g., `11_289_600` for DSD256), and `TransportFrameRateHz` (e.g., `176_400` for DoP64 frames or `88_200` for ALSA `DSD_U32` frames).
   - **SPA DSD API Boundary:** PipeWire's SPA `audio/dsd` format expresses rate in **bytes per second per channel** ($f_{\text{bit}} / 8$, e.g., `1_411_200` for DSD256), with separate bit-order and signed byte-interleave fields [1]. Always convert via an explicit method on `DsdBitRateHz` at the SPA boundary; never pass the bit clock directly.
   - Keep 44.1 kHz-family DSD (`DSD64 = 2.8224 MHz`) and 48 kHz-family DSD (`DSD64/48 = 3.072 MHz`) explicit; never assume a DAC that supports conventional DSD256 also accepts 48 kHz-family DSD rates.

2. **Never Touch Packed DSD/DoP as PCM:**
   - All volume adjustments, `preamp_db` attenuation, headroom scaling, channel mixing, and fade ramps must occur in floating-point PCM **before** the 1-bit modulator.
   - Once modulated into 1-bit DSD or packed into DoP frames, samples must never pass through PCM gain, crossfaders, limiters, or sample-rate converters.

3. **DSD Silence is Never `0x00` (`audigma-dsp` / `audigma-format`):**
   - In 1-bit DSD, all-zero bytes (`0x00`) represent full-scale negative DC, which causes loud analog pops on start/stop.
   - Planned transitions (stop, mute, rate change) must ramp PCM to zero before modulation, wait for `filter_group_delay` to drain, and then emit a balanced DSD idle pattern (`0x69` / verified zero-input modulator bitstream) while keeping DoP `0x05`/`0xFA` markers alternating.

4. **Output Backend Separation: PipeWire PCM vs. Exclusive ALSA (`audigma-io` / `audigma-engine`):**
   - **High-Rate PCM Output (`[target] format = "pcm"`)** can either return directly into the **PipeWire graph** (`[output] backend = "pipewire"`, keeping the DAC managed by PipeWire under a single shared clock) or drive an **exclusive ALSA endpoint** (`[output] backend = "alsa"`, e.g., `705.6/768 kHz` `s32le` with TPDF dither for NOS modes).
   - **DSD Output (`[target] format = "dsd"`)** always drives an **exclusive ALSA endpoint** (`[output] backend = "alsa"`, `transport = "native" | "dop"`). Normal PipeWire PCM links cannot carry DoP safely because software volume or resampling corrupts the `0x05`/`0xFA` marker bits.
   - **D-Bus Device Reservation (`org.freedesktop.ReserveDevice1`):** To avoid `Device or resource busy` lockouts when taking exclusive control of an ALSA DAC without requiring permanent configuration changes, `audigma-io` acquires the DAC dynamically via the standard D-Bus device reservation protocol (`[output] reserve_device = true`, enabled by default). WirePlumber automatically suspends its ALSA PCM monitor while `audigma` runs, and restores normal desktop routing when `audigma` stops. For headless setups without desktop sound servers, `reserve_device = false` can be configured.

5. **Clock Domains & Low-Rate Baseband ASRC (`audigma-dsp` / `audigma-engine`):**
   - **Shared Clock (`clock.mode = "shared-graph"`):** When both `[input]` and `[output]` use `backend = "pipewire"`, both sides are paced by the same PipeWire graph clock; no asynchronous drift controller is needed.
   - **Independent Clocks (`clock.mode = "adaptive-input"`):** When bridging a software input clock (PipeWire sink or ALSA Loopback) to a physical ALSA hardware DAC (`[output] backend = "alsa"`), the hardware DAC is the master clock. Clock drift is absorbed by factoring the total conversion ratio as:
     $$R_{\text{total}}(t) = R_{\text{asrc}}(t) \times R_{\text{rational}} \times 2^K$$
   - The continuously variable fractional-phase stage ($R_{\text{asrc}}(t)$) **always executes at the low baseband/intermediate PCM rate ($1\times$ or $2\times$)** *before* the fixed-coefficient $2\times$ half-band FIR cascade—never at the final MHz DSD bit rate, and never by dropping/duplicating DSD bits.
   - **ASRC Drift Bounds & Recovery:** The drift loop operates within a bounded correction range (e.g. `max_correction_ppm = 500`). When measured drift or queue fill exceeds safety thresholds, the engine invokes a controlled fault/recovery sequence rather than silently dropping or duplicating samples.

6. **DSD Passthrough Invariant (`audigma-engine` / `audigma-dsp`):**
   - Bit-exact DSD passthrough is a dedicated, typed signal path with zero intermediate float conversions.
   - DSD passthrough is only valid across shared-clock or backpressured links where input and output rates match. It **cannot** reconcile independent live clock domains (which require PCM ASRC) without losing bit-exactness.
   - Any volume change, EQ, or sample-rate conversion on an incoming DSD stream requires explicit remodulation ($\text{DSD} \to \text{PCM} \to \text{DSM}$), which must be reported as a lossy conversion path.

---

## 2. Crate Architecture
 
The Cargo workspace is organized as a strictly layered 8-crate DAG with zero circular dependencies:
 
```text
crates/
  audigma-dsp/          # Pure math: rate types, DspConfiguration, presets.rs, FIR/ASRC, 5th-order CRFB DSM, dither, FTZ/DAZ (zero OS I/O)
  audigma-format/       # Containers (WAV/FLAC/DSF) & exact bit-packers (DSD_U32_BE, DoP s32le/s24le/s24_3le)
  audigma-settings/     # TOML config Schema v1 (`Settings`), [[rules]] resolver, config check & explain
  audigma-engine/       # Pure Rust pipeline planner, SPSC queues, ASRC drift controller & I/O traits (zero C FFI)
  audigma-io/           # Feature-gated concrete backends: ALSA, PipeWire, D-Bus ReserveDevice1, file/stdio & prober
  audigma-configurator/ # Interactive TUI & one-shot Web setup wizard (uses settings + engine/io prober & benchmark)
  audigma-doctor/       # Host OS diagnosis, ALSA/PipeWire lock inspection & interactive remediation (--fix)
  audigma-cli/          # Binary crate (`audigma`): CLI subcommands, Unix control socket, foreground runner
tools/
  design/                 # Offline scripts to synthesize FIR & Schreier NTF coefficient tables
  measure/                # Coherent FFT & Welch spectral analysis harness
docs/
  measurements/           # Generated SNR/SINAD/stability verification reports
  hardware/               # Probed DAC capability records (e.g., FiiO K17, JadeAudio JA11)
examples/                 # Ready-to-use TOML configs, MPD snippets, and WirePlumber rules
```
 
### Layer Responsibilities
- **`audigma-dsp`**: Pure numerical processing (`no_std`-friendly core logic, zero OS/file/network I/O). Implements domain rate/format types (`PcmRateHz`, `DsdBitRateHz`, `TransportFrameRateHz`), `DspConfiguration`, built-in verified filter and modulator coefficient tables (`presets.rs`: `PRESET_BALANCED`, `PRESET_REFERENCE`, `PRESET_LOW_LATENCY`), polyphase & half-band FIR interpolation, state-space / CRFB delta-sigma modulators, TPDF dither, overload guards, and thread-level hardware subnormal flushing (FTZ/DAZ). Portable across any operating system.
- **`audigma-format`**: Audio container decoding/encoding (`WAV`, `FLAC`, Sony `DSF`) and hardware frame bit-packers (`DSD_U32_BE`, `DoP` across `s32le`/`s24le`/`s24_3le`). Zero OS dependencies.
- **`audigma-settings`**: The `Schema v1` TOML configuration structs (`Settings`), `[[rules]]` evaluation engine, `config check` validation, and `config explain` resolution (mapping user settings and preset names like `"reference"` into concrete `audigma-dsp::DspConfiguration` and transport parameters). Zero OS dependencies.
- **`audigma-engine`**: Pure Rust orchestration connecting `dsp`, `format`, and `settings` into prepared pipelines (`prepare`, `process`, `drain`), lock-free real-time SPSC queues (`rtrb`), generation tags, the PI-controlled ASRC drift loop, lifecycle state machine, and abstract `AudioInput`/`AudioOutput` backend traits. Zero C-FFI or OS audio library dependencies, allowing instant unit testing with mock clocks/streams on any host.
- **`audigma-io`**: Feature-gated concrete implementations of the `audigma-engine` I/O traits (`alsa`, `pipewire`, `file`, `stdio`, plus future platform backends like `coreaudio` or `asio`). Houses ALSA hardware capability probing, PipeWire virtual sink (`audigma_sink`) registration, and dynamic D-Bus device reservation (`org.freedesktop.ReserveDevice1`).
- **`audigma-configurator`**: Isolates UI/terminal (`ratatui`/`crossterm`) and optional localhost one-shot Web wizard dependencies so they never bloat the core DSP or engine crates. Calls into `audigma-settings`, `audigma-engine`, and `audigma-io` to probe DACs, benchmark presets, and write `config.toml`.
- **`audigma-doctor`**: Isolates host OS-level diagnostic and repair logic behind a platform abstraction trait (`DiagnosticProvider`). On Linux, inspects PipeWire graph clock configuration (`clock.allowed-rates`), probes ALSA device locks (`/dev/snd/pcm*`), handles interactive `--fix` (calling `wpctl set-profile <id> 0`, terminating conflicting processes with optional `sudo` escalation, or generating persistent WirePlumber drop-ins). Keeps OS-specific plumbing completely segregated from the cross-platform core and configurator.
- **`audigma-cli`**: The `audigma` executable entry point, CLI subcommand router (`clap`), Unix-domain control socket server/client (`$XDG_RUNTIME_DIR/audigma/`), and foreground service runner.

---

## 3. Execution Modes, Rate Policies & Output Paths

### 3.1 Two Execution Modes

1. **Live Streaming Daemon (`audigma run`):**
   - Bridges a live `[input]` (`pipewire` virtual sink `audigma_sink` or `alsa` capture/loopback) through the prepared real-time DSP pipeline to a live `[output]` (`pipewire` node or exclusive `alsa` hardware device).
   - Allows both desktop applications (browsers, YouTube Music) and dedicated players (MPD) to play through the same pipeline without `Device or resource busy` lockouts.

2. **Offline File / Stream Converter (`audigma convert`):**
   - Reads a file (`WAV`/`FLAC`) or `stdin` (`raw`), runs synchronous DSP without hardware clock tracking, and writes `.dsf`, high-rate `.wav`, or raw DoP/PCM to `stdout`.
   - Supports deterministic seeding, two-pass true-peak headroom analysis, and exact sample-count duration trimming.

### 3.2 Two Rate Policies (`[target] rate_policy`)

Rather than an artificial "desktop vs. music mode" switch on the input, output rate switching is controlled purely by `[target] rate_policy` (or `[[rules]]`):

1. **`rate_policy = "fixed"` (Zero-Relock / Always-On Policy):**
   - Keeps the output locked at one fixed rate (e.g., always `DSD256` at `11.2896 MHz` or `PCM 192 kHz`).
   - Any input rate (`44.1 kHz` or `48 kHz`) is seamlessly converted into that single target rate via the polyphase SRC + $2\times$ FIR cascade.
   - Ideal when mixing desktop audio or switching between apps/tracks without triggering DAC hardware relay clicks or relock delays.

2. **`rate_policy = "source-family"` (Family-Preserving Policy):**
   - Dynamically switches the output rate between its `44.1 kHz`-family target (e.g., `705.6 kHz` / `DSD256-44.1`) and `48 kHz`-family target (e.g., `768 kHz` / `DSD256-48`) whenever the input stream changes rate family.
   - Avoids cross-family rational SRC so upsampling uses pure $2^K$ half-band FIR stages (plus low-rate ASRC drift tracking when driving an independent ALSA clock).
   - When used with a PipeWire virtual sink input, `audigma doctor` inspects PipeWire's `default.clock.allowed-rates` and warns if the PipeWire graph is pinned to a single rate (such as `48000`) that would prevent dynamic input rates.

### 3.3 Three Supported Live Output Paths

| Live Output Path | `[output]` Config | `[target]` Config | Clock Domain & Device Ownership |
|---|---|---|---|
| **1. PipeWire High-Rate PCM Upsampler** | `backend = "pipewire"`<br>`target_node = "alsa_output..."` | `format = "pcm"`<br>`sample_format = "f32"` | **`shared-graph` clock** (no ASRC drift loop needed); PipeWire retains ownership of the DAC. |
| **2. Direct Exclusive ALSA PCM (NOS / High-Rate)** | `backend = "alsa"`<br>`device = "hw:CARD=K17,DEV=0"` | `format = "pcm"`<br>`sample_format = "s32le"` + `dither = "tpdf"` | **`adaptive-input` ASRC** referenced to DAC master clock; `audigma` owns the ALSA DAC exclusively. |
| **3. Direct Exclusive ALSA DSD (Native / DoP)** | `backend = "alsa"`<br>`device = "hw:CARD=K17,DEV=0"`<br>`transport = "native" \| "dop"` | `format = "dsd"`<br>`dsd_bit_rate_hz = 11289600` | **`adaptive-input` ASRC** referenced to DAC master clock; `audigma` owns the ALSA DAC exclusively. |

### 3.4 Prepared Pipeline Stages

Every active pipeline executes the following ordered stages:

```text
decode / unpack
  → channel validation (stereo primary)
  → user volume + preamp_db + profile headroom gain (f64 PCM)
  → low-rate adaptive/rational polyphase SRC (R_asrc × R_rational)
  → staged 2× half-band FIR interpolation cascade (2^K)
  → modulator-input peak meter & overload guard
  → PCM quantizer (+ TPDF dither for integer PCM)  OR  1-bit Delta-Sigma Modulator (CRFB)
  → transport packer (Native DSD_U32_BE / DoP s32le|s24le|s24_3le / DSF / PCM)
  → backend output
```

- **Zero Allocation & Generation Tags in Real-Time:** `prepare(spec, config)` resolves all filter stages, coefficients, and preallocated `rtrb` SPSC ring buffers off the audio thread. `process(input, output)` never allocates on the heap, locks mutexes, or performs I/O. SPSC queue chunks carry a monotonic **generation tag** (`u64`) incremented on every pipeline/format reconfiguration so stale-format blocks in flight are immediately discarded and can never reach a newly configured audio endpoint.
- **Real-Time Thread Scheduling:** The DSP worker and ALSA hardware egress threads acquire real-time priority via standard Linux user-session facilities (`RTKit` / PipeWire RT thread properties / `RLIMIT_RTPRIO`), logging an observable warning and falling back cleanly to normal scheduling if unavailable.
- **Bounded Tile Streaming:** Upsampling to `DSD256` ($256\times$) expands 1024 input frames into 262,144 samples per channel. To preserve L1/L2 cache locality and small memory footprints, the final $2\times$ interpolation stages stream in small bounded tiles directly into the modulator rather than materializing huge MHz-rate float buffers.

---

## 4. DSP Design: Resampling, Modulation, and Transitions

### 4.1 Reference Levels, Baseband OSR & Subnormal Handling
- Internal PCM uses normalized `f64` samples where `1.0` equals `0 dBFS`.
- The 1-bit quantizer emits `-1.0` or `+1.0`, mapped to `0` and `1` bits during packing.
- **Baseband Over-Sampling Ratio ($\text{OSR}$):**
  $$\text{OSR} = \frac{f_{\text{bit}}}{2 \cdot B}$$
  where $B$ is the design audio baseband ($20\text{ kHz}$). Note that DSD's rate multiplier ($64, 128, 256$) is relative to $44.1\text{ kHz}$, **not** $2 \cdot B = 40\text{ kHz}$. Thus, conventional `DSD256` at $11.2896\text{ MHz}$ with $B = 20\text{ kHz}$ has $\text{OSR} = 282.24$ (and `DSD64` has $\text{OSR} = 70.56$). NTF synthesis scripts in `tools/design/` must always pass the exact $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ to Schreier synthesis routines.
- **Hardware FTZ/DAZ:** Inside the 1-bit DSM loop, feedback is strictly `{-1.0, +1.0}`, keeping integrators excited. However, upstream PCM FIR/IIR stages fed digital silence can decay into subnormal floats and cause severe CPU tail-latency spikes. Every DSP worker thread sets hardware **Flush-To-Zero (FTZ)** and **Denormals-Are-Zero (DAZ)** (`MXCSR` on `x86_64`, `FPCR` on `aarch64`) on thread entry.

### 4.2 Multistage Resampling & Filter Presets
1. **Polyphase Windowed-Sinc & ASRC:** Handles cross-family rational ratios (`44.1 kHz ↔ 48 kHz`) and smooth fractional-phase clock drift compensation ($R_{\text{asrc}}(t)$) at the $1\times$ or $2\times$ baseband rate.
2. **Staged $2\times$ Half-Band FIR Cascade:** Subsequent power-of-two upsampling ($2^K$) uses cascaded $2\times$ FIR filters, exploiting zero-valued every-other-tap sparsity for linear-phase stages.
3. **Filter Controls:**
   - `preset = "balanced" | "reference" | "low-latency"`
   - `phase = "linear" | "minimum"`
   - `apodization = "off" | "gentle"`

| Filter Preset | Passband Ripple Goal | Stopband / Image Rejection Goal | Target Use Case |
|---|---:|---:|---|
| `balanced` | $\le 0.01\text{ dB}$ | $\ge 120\text{ dB}$ | Default real-time playback |
| `reference` | $\le 0.001\text{ dB}$ | $\ge 140\text{ dB}$ | Offline conversion & high-margin live upsampling |
| `low-latency` | Declared per rate | $\ge 100\text{ dB}$ | Low group-delay interactive desktop audio |

### 4.3 1-Bit Delta-Sigma Modulator (CRFB)
- **Topology:** `f64` 5th-order Cascade of Resonators with Feedback (**CRFB**) as the baseline modulator for `DSD64`, `DSD128`, and `DSD256` (with 7th-order CRFB optional for higher OSRs).
- **NTF Synthesis:** Offline Schreier-style NTF synthesis (`tools/design/`) using $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ with optimized resonator zeros in the 0–20 kHz audio band and initial out-of-band gain search around $H_\infty \approx 1.5$, scaled for dynamic range and verified against maximum stable input amplitude ($L\text{ dBFS}$) across DC, coherent tones, multitone bursts, and silence.
- **Dither:** Optional TPDF dither injected prior to the 1-bit quantizer to suppress idle tones during low-level signals or silence.
- **Baseline Headroom:** Every modulator table specifies its verified input limit $L\text{ dBFS}$ and required baseline headroom in dB (e.g., `-6.0 dB` to `-3.5 dB`) so `config explain` reports the exact total attenuation (`preamp_db + volume_db + profile_headroom_db`).

### 4.4 Headroom Inequality, Two-Pass Offline Peak Analysis & Live Overload Protection
- **Required Attenuation Equation:** Given a predicted or measured post-interpolation peak of $P\text{ dBFS}$, the modulator profile's maximum stable input limit $L\text{ dBFS}$ (e.g., $-3.5\text{ dBFS}$), and a safety margin $M\text{ dB}$ (default $0.5\text{ dB}$), the total applied pre-modulator gain must satisfy:
  $$\text{gain\_db} \le L - M - P$$
- **Offline Two-Pass Gain (`peak_analysis = "two-pass"`):** Because the PCM SRC + $2\times$ FIR interpolation cascade is Linear and Time-Invariant (LTI), scaling the pre-SRC input by gain $G$ scales the reconstructed peak entering the modulator by exactly $G$. Therefore, **Pass 1 runs only the PCM interpolation cascade** (skipping the sequential 1-bit DSM loop and bit packing) to measure the true inter-sample peak $P\text{ dBFS}$, solves $\text{gain\_db} = \min(\text{configured\_gain\_db},\; L - M - P)$, and **Pass 2 runs the complete pipeline**.
- **Live Overload Guard (`overload_policy = "mute-recover"`):** Monitors the PCM signal entering the modulator and the internal integrator states. If an unexpected transient exceeds the modulator's stable envelope or drives states toward instability:
  1. Immediately bypass the filter tail and switch the output to the verified DSD idle stream at the modulator boundary.
  2. Reset and reprime the modulator state vector to zero.
  3. Resume audio with a smooth pre-SRC PCM fade-in once input levels return to normal.

### 4.5 Lifecycle & Click-Free Transitions
The engine state machine runs through:
`Stopped → Preparing → Priming → Running → Draining/Muting → Reconfiguring` (plus `Recovering` and `DeviceLost`).

- **Planned Stop / Rate Switch:** Ramp PCM gain to zero pre-SRC $\to$ run `filter_group_delay` frames until the FIR tail clears $\to$ emit the balanced DSD idle pattern (`0x69`) while holding DoP marker alternation (`0x05`/`0xFA`) during device settling.

---

## 5. Linux Transport, Bit-Packing & File Containers

### 5.1 DSD Rate & Transport Relationships (44.1 kHz Family)

| Target | DSD Bit Rate / ch (`DsdBitRateHz`) | DoP Frame Rate (`TransportFrameRateHz`) | ALSA `DSD_U32` Frame Rate (`TransportFrameRateHz`) | SPA DSD Byte Rate / ch |
|---|---:|---:|---:|---:|
| `DSD64` | `2,822,400` | `176,400` | `88,200` | `352,800` |
| `DSD128` | `5,644,800` | `352,800` | `176,400` | `705,600` |
| `DSD256` | `11,289,600` | `705,600` | `352,800` | `1,411,200` |
| `DSD512` | `22,579,200` | `1,411,200` | `705,600` | `2,822,400` |

*(For 48 kHz-family DSD, multiply `48,000` by `64/128/256/512`; e.g., `DSD256/48 = 12,288,000 Hz`).*

### 5.2 Exact Bit-Packing Formats (`audigma-format`)

1. **ALSA Native `DSD_U32_BE` / `DSD_U32_LE`:**
   - Packs 32 consecutive 1-bit DSD samples per channel into each 32-bit ALSA container word (`f_frame = f_bit / 32`), MSB-first bit ordering (`bitrev=0` unless device quirks specify LSB-first).
2. **DoP (DSD over PCM v1.1) — Three ALSA Container Alignments:**
   - Carries 16 DSD bits (2 bytes: `HI`, `LO`, oldest bit in MSB) per channel per PCM frame, with an 8-bit marker alternating `0x05` and `0xFA` on consecutive frames (synchronized across stereo channels).
   - Because USB Audio Class 2.0 DACs expose different 24/32-bit integer formats over ALSA, `audigma-format` implements and tests all three alignments:
     - **`s32le` (32-bit container, MSB-justified):** 24-bit DoP word shifted left by 8 (`0xMM_HI_LO_00`, written little-endian as `[0x00, LO, HI, MM]`).
     - **`s24le` (32-bit container, LSB-justified):** 24-bit DoP word in the lower 24 bits (`0x00_MM_HI_LO`, written little-endian as `[LO, HI, MM, 0x00]`).
     - **`s24_3le` (Packed 24-bit container, 3 bytes/sample):** Written little-endian as `[LO, HI, MM]`.
3. **Sony DSF Container (`.dsf`):**
   - Little-endian `DSD `, `fmt `, and `data` chunks; interleaves channels in fixed **4096-byte blocks per channel** (`L[4096], R[4096]`), with declared bit ordering (`1` = LSB-first per Sony DSF v1.01, `8` = MSB-first) and exact per-channel `sample_count` in bits.
   - **Final Block Padding (`dsf_pad_mode`):** Sony DSF v1.01 §4 specifies `0x00` fill for unused trailing bytes in the final 4096-byte block. However, naive players that ignore `sample_count` play those trailing `0x00` bytes as full-negative DC (~11.6 ms pop at DSD64). `audigma` supports:
     - `dsf_pad_mode = "spec-zero"` (default: standard `0x00` padding with exact `sample_count`).
     - `dsf_pad_mode = "dsd-silence"` (fills trailing pad bytes with `0x69` balanced DSD silence to prevent end-of-track pops on broken players).

### 5.3 Stdio Streaming & Container Seekability Constraints
- **Raw Streams (`container = "raw"`):** Require complete format declarations (`sample_format`, `rate_hz`, `channels` on input; `transport`, `sample_format` on output). `stdout` must carry audio payload bytes only; all logs and diagnostics go strictly to `stderr`.
- **Header-Finalized Containers (`dsf` / `wav`):** Writing valid `.dsf` or `.wav` headers requires either a **seekable output file** (to patch chunk sizes and `sample_count` at EOF) or a known-length strategy. `audigma convert -o -` must explicitly reject unsupported non-seekable `stdout` combinations (such as streaming `container = "dsf"` to a pipe) rather than emitting corrupt headers.
- **Pipe Resilience:** Handle pipe backpressure, partial frame reads/writes, clean `EOF` draining, and `EPIPE` (broken pipe) gracefully without panicking.
- **Album-Continuous Processing:** When converting multi-track albums, state (filter ring buffers, modulator integrators, dither state) must be preserved across track boundaries, splitting output files at exact rational sample boundaries to guarantee click-free gapless playback.

---

## 6. Configuration Schema (`Schema v1`) & Presets

Configuration lives at `$XDG_CONFIG_HOME/audigma/config.toml` (default `~/.config/audigma/config.toml`). Unknown keys or invalid cross-field combinations are rejected by `audigma config check`.

### 6.1 Rule-Based Selection (`[[rules]]`) vs. `rate_policy`
`[[rules]]` evaluates in first-match order based on observed input rate/format and resolves into an explicit preset and target configuration. `rate_policy = "source-family"` is convenient shorthand that lowers into the same rule resolver:

```toml
schema_version = 1

[[rules]]
when_input_rate_family = "44100"   # 44.1k, 88.2k, 176.4k, 352.8k
apply_preset = "music-reference"
set_target = { format = "dsd", dsd_bit_rate_hz = 11289600 }

[[rules]]
when_input_rate_family = "48000"   # 48k, 96k, 192k, 384k
apply_preset = "music-reference"
set_target = { format = "pcm", pcm_rate_hz = 768000 }
```

### 6.2 Reference Configuration Fixtures

#### A. PipeWire Virtual Sink $\to$ PipeWire High-Rate PCM Output (`pipewire-pcm.toml`)
```toml
schema_version = 1

[input]
backend = "pipewire"
node_name = "audigma_sink"
channels = 2

[output]
backend = "pipewire"
target_node = "alsa_output.usb-FiiO_K17-00.analog-stereo"
transport = "pcm"

[target]
format = "pcm"
rate_policy = "fixed"
pcm_rate_hz = 192000
sample_format = "f32"

[gain]
preamp_db = -3.0
overload_policy = "mute-recover"

[resampler]
preset = "balanced"
phase = "linear"
apodization = "off"

[clock]
mode = "shared-graph"

[runtime]
latency_target_ms = 40
on_device_loss = "wait"
```

#### B. PipeWire Virtual Sink $\to$ Direct Exclusive ALSA Native DSD (`pipewire-alsa-dsd.toml`)
```toml
schema_version = 1

[input]
backend = "pipewire"
node_name = "audigma_sink"
channels = 2

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "native"
reserve_device = true
buffer_ms = 40
period_ms = 5

[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 11289600

[gain]
preamp_db = 0.0
headroom = "profile"
overload_policy = "mute-recover"

[resampler]
preset = "balanced"
phase = "minimum"
apodization = "off"

[modulator]
order = 5

[clock]
mode = "adaptive-input"
max_correction_ppm = 500

[runtime]
settle_ms = 50
on_device_loss = "wait"
fallback = "none"
```

#### C. ALSA Loopback (MPD) $\to$ Direct Exclusive ALSA High-Rate PCM (`loopback-alsa-pcm.toml`)
```toml
schema_version = 1

[input]
backend = "alsa"
device = "hw:Loopback,1,0"
channels = 2

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "pcm"
reserve_device = true
buffer_ms = 40
period_ms = 5

[target]
format = "pcm"
rate_policy = "source-family"
pcm_rate_44100_hz = 705600
pcm_rate_48000_hz = 768000
sample_format = "s32le"
valid_bits = 24
dither = "tpdf"

[gain]
preamp_db = -4.0
overload_policy = "mute-recover"

[resampler]
preset = "reference"
phase = "linear"
apodization = "off"

[clock]
mode = "adaptive-input"
max_correction_ppm = 500
```

#### D. Offline WAV/FLAC $\to$ DSF (`offline-dsf.toml`)
```toml
schema_version = 1

[input]
backend = "file"

[output]
backend = "file"
container = "dsf"

[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 11289600

[gain]
preamp_db = 0.0
headroom = "profile"
peak_analysis = "two-pass"
overload_policy = "error"

[resampler]
preset = "reference"
phase = "linear"
apodization = "off"

[modulator]
order = 5

[offline]
seed = 12345
tail_policy = "trim-to-duration"
write_report = true
dsf_pad_mode = "spec-zero"
```

#### E. Raw PCM stdin $\to$ Raw DoP stdout (`stdio-dop.toml`)
```toml
schema_version = 1

[input]
backend = "stdio"
sample_format = "s16le"
rate_hz = 44100
channels = 2

[output]
backend = "stdio"
container = "raw"
transport = "dop"
sample_format = "s24_3le"

[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 5644800

[gain]
headroom = "profile"
overload_policy = "error"

[resampler]
preset = "balanced"
phase = "linear"
apodization = "off"

[modulator]
order = 5
```

---

## 7. CLI, Configurator & Service Interface

```text
audigma run [-c CONFIG]
audigma stop
audigma convert -i INPUT -o OUTPUT [-c CONFIG]
audigma benchmark [-i INPUT] [-c CONFIG] [--json]
audigma probe [--backend alsa|pipewire] [--json]
audigma configure [--tui|--web] [--out PATH]
audigma config check [-c CONFIG]
audigma config explain [-c CONFIG] [--input-rate HZ] [--json]
audigma presets list
audigma presets show NAME
audigma status [--watch] [--json]
audigma set volume DB
audigma set preset NAME
audigma reload
audigma doctor [--fix] [--json]
```

- **`audigma run` & `audigma stop`:** `run` executes the live audio pipeline in the foreground (intended for `systemd --user` service or direct terminal invocation). `audigma stop` connects to the local control socket and requests an orderly shutdown: ramping PCM gain to zero, draining filter group delays, emitting the balanced DSD idle sequence, releasing the D-Bus `ReserveDevice1` device lock so PipeWire/WirePlumber immediately reclaims the DAC, and terminating the daemon.
- **`audigma configure` (`audigma-configurator`):** Interactive TUI (`--tui`, default) or temporary localhost Web wizard (`--web`, bound to `127.0.0.1:<port>` with a one-time session token that exits cleanly on save). Walks the user through selecting an output path (PipeWire PCM upsampler vs. Exclusive ALSA PCM/DSD), choosing a rate policy (`fixed` vs. `source-family`), running a quick CPU benchmark, previewing headroom/latency, and writing `config.toml` + optional WirePlumber/systemd user snippets.
- **`audigma doctor [--fix]` (`audigma-doctor`):** Inspects the host OS audio stack and diagnoses runtime issues:
  1. *ALSA Device Lockouts:* Inspects `/dev/snd/pcmC*D*p` file descriptor holders. If held by WirePlumber/PipeWire, `--fix` can dynamically invoke `wpctl set-profile <id> 0` or offer to write a permanent WirePlumber drop-in rule in `~/.config/wireplumber/`. If held by a foreign user or daemon (such as MPD), `--fix` interactively prompts the user before terminating the process, escalating to `sudo` if permissions are insufficient, or printing the exact command required.
  2. *PipeWire Multi-Rate Configuration:* Checks `default.clock.allowed-rates` in PipeWire when `rate_policy = "source-family"` is active on a PipeWire virtual sink, warning and offering to install the required multi-rate configuration drop-in if the graph is pinned to a single rate (e.g. `48000`).
  3. *Real-Time Scheduling Limits:* Verifies `RLIMIT_RTPRIO` / RTKit availability for real-time DSP threads.
- **Control Socket & Telemetry Protocol (`audigma status`):** Operates over a local Unix domain socket at `$XDG_RUNTIME_DIR/audigma/control.sock`. Runtime telemetry exported via `status [--watch] [--json]` exposes:
  - Input negotiated format/rate and PipeWire graph rate.
  - Output bit/sample/transport rate and active physical device/endpoint.
  - Active preset, profile hash, and applied gain breakdown (`preamp_db`, user volume, profile headroom).
  - Measured true peaks and modulator integrator operating envelope.
  - Latency breakdown (graph + queue + filter group delay + backend buffer).
  - SPSC queue fill percentage and estimated ASRC clock drift ($\delta_{\text{ppm}}$).
  - Hardware XRun counters, fault/recovery event counts, and per-stage processing timing.

---

## 8. Spectral Verification & Quality Targets (`tools/measure/`)

Every filter preset and modulator order in `audigma-dsp` (`presets.rs`) is verified via automated tests in `tools/measure/`:

1. **Coherent FFT & Welch Protocol:**
   - Map 1-bit DSD output to `{-1.0, +1.0}`.
   - Use coherent test frequencies ($f_{\text{tone}} = k \cdot f_{\text{bit}} / N$, e.g., $N = 1,048,576$) with Welch segment averaging to measure in-band SNR (20 Hz–20 kHz), SINAD/THD+N, and ultrasonic noise power (20–100 kHz and 100 kHz–Nyquist).
2. **Stability & Envelope Tests:**
   - Verify bounded integrator states across DC offsets, $-6\text{ dBFS}$ coherent tones, high-crest-factor multitone bursts, digital silence, and forced overload recovery.
3. **Real-Time Throughput & Deadline Budget:**
   - Sustained average DSP throughput $\ge 2\times$ real-time on the host CPU.
   - **p99.9 worker execution time $\le 70\%$** of the audio-block duration, with zero heap allocations inside `process()`.

---

## 9. Technical References

1. [PipeWire SPA DSD structure and rate/interleave definitions](https://docs.pipewire.org/dsd_8h_source.html).
2. [PipeWire node, adapter, resampling, and ALSA properties](https://docs.pipewire.org/page_man_pipewire-props_7.html).
3. [DoP open standard v1.1](https://dsd-guide.com/dop-open-standard).
4. [Sony DSF specification v1.01](https://dsd-guide.com/sites/default/files/white-papers/DSFFileFormatSpec_E.pdf).
5. [Schreier/Caldwell NTF synthesis & delta-sigma design lectures](https://www.individual.utoronto.ca/schreier/lectures/5-6.pdf).
