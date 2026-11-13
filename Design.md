# audigma — Technical Specification & Architecture (v2.3)

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
   - **High-Rate PCM Output (`[target] format = "pcm"`)** can either return directly into the **PipeWire graph** (`[output] backend = "pipewire"`, keeping the DAC managed by PipeWire under a single shared clock; the output stream requests the graph run at the target rate with `node.rate = 1/<rate>`, which PipeWire honours only if the rate is in `clock.allowed-rates` and no other stream holds the graph at a different rate, otherwise its adapter resamples the stream to the graph rate) or drive an **exclusive ALSA endpoint** (`[output] backend = "alsa"`, e.g., `705.6/768 kHz` `s32le` with TPDF dither for NOS modes).
   - **DSD Output (`[target] format = "dsd"`)** always drives an **exclusive ALSA endpoint** (`[output] backend = "alsa"`, `transport = "native" | "dop"`). Normal PipeWire PCM links cannot carry DoP safely because software volume or resampling corrupts the `0x05`/`0xFA` marker bits.
   - **D-Bus Device Reservation (`org.freedesktop.ReserveDevice1`):** To avoid `Device or resource busy` lockouts when taking exclusive control of an ALSA DAC without requiring permanent configuration changes, `audigma-io` acquires the DAC dynamically via the standard D-Bus device reservation protocol (`[output] reserve_device = true`, enabled by default). WirePlumber automatically suspends its ALSA PCM monitor while `audigma` runs, and restores normal desktop routing when `audigma` stops. For headless setups without desktop sound servers, `reserve_device = false` can be configured.

5. **Clock Domains & Low-Rate Baseband ASRC (`audigma-dsp` / `audigma-engine`):**
   - **ASRC corrects a rate difference between two independent clocks, nothing else.** Every input declares its clock (`AudioInput::input_clock()`): `DemandDriven` (the producer is paced by audigma's consumption, so it shares the output clock) or `FreeRunning` (the producer runs on its own clock). The PipeWire virtual sink is `DemandDriven`: it is always its own PipeWire driver (`NODE_DRIVER = "true"`; without it `pw-dsdplay` runs ~4x realtime or the sink is missing from KDE volume control) and a graph cycle is triggered only when its queue drops below the preroll cushion, so the producer follows the DAC by construction and the queue is held by backpressure.
   - **Mode per stream (`[resampler] drift_correction = "auto" | "on" | "off"`):** `off` → no ASRC. `auto` → `Correct` for a `FreeRunning` input with `[output] backend = "alsa"` (the DAC clock is master), otherwise off. `on` → `Correct` for a `FreeRunning` input; for a `DemandDriven` input `Observe`: the drift estimator runs and is reported, 0 ppm is applied and the audio path is the normal one. Applying an estimate to a demand-driven producer is circular (the producer echoes the applied ratio) and is never done. `on` + PipeWire output emits a validation warning (the output stream is rate-matched by the graph).
   - **Ratio factorisation (`Correct`):** the hardware DAC is the master clock and the total conversion ratio is
     $$R_{\text{total}}(t) = R_{\text{asrc}}(t) \times R_{\text{rational}} \times 2^K$$
   - The continuously variable fractional-phase stage ($R_{\text{asrc}}(t)$) **always executes at the low baseband/intermediate PCM rate ($1\times$ or $2\times$)** *before* the fixed-coefficient $2\times$ half-band FIR cascade—never at the final MHz DSD bit rate, and never by dropping/duplicating DSD bits.
   - **Drift estimator plus fill trim (`asrc.rs`, `ClockDriftController`):** the queue obeys $\dot{f} = D - c$ (fill $f$ in seconds of input, true clock offset $D$, applied correction $c$), so $D = \dot{f} + \bar{c}$ is measured feed-forward, independent of any setpoint: fill and applied correction are averaged into 0.5 s blocks (removing the per-chunk sawtooth) and a least-squares slope over the last 64 blocks (32 s) gives $\dot{f}$. Fill samples from the first 2 s after a stream start or idle exit are ignored (warm-up: the queue is still settling after a cushion refill or decoder priming, and its ramp would read as drift), so the estimate is valid 10 s in. **A `FreeRunning` input must report its fill continuously** (queued frames plus the producer progress interpolated at the nominal rate since the last chunk arrival, capped at one chunk): chunk and read tile are commensurate, so queued frames alone sit on a tile-sized lattice that steps only every `tile / (rate · D)` seconds (29 s at 200 ppm, 44.1 kHz), and no practical window resolves drift from it. A proportional trim with no integrator (5 ppm/ms beyond a ±5 ms dead band, ≤ ±100 ppm) returns the fill to the level latched at stream start. The applied correction moves towards `estimate + trim` at ≤ 10 ppm/s and is clamped to `[resampler] max_clock_drift_ppm` (default `500`). There is no integrator state to wind up.
   - **Rails & recovery:** a fill below 10 ms or above max(250 ms, 2 × setpoint) is acted on only after a 50 ms debounce (an end-of-stream drain does not move the correction); a confirmed rail counts a starvation/overrun fault, applies the full correction in the restoring direction and clears the estimator window. The rails are not checked during the 2 s warm-up: with a 20 ms input cushion the queue dips below 10 ms for a moment after a stream start or rate switch while the pipeline primes (observed about 0.3 s in, with the device buffer still above 27 ms), which is settling, not a fault. A new controller is built per stream (rate switches); on idle entry the warm-up restarts and the window, trim, setpoint and rails reset while the estimate is kept.

6. **DSD Ingestion, Automatic Passthrough & Bounded Decimation Remodulation (`audigma-engine` / `audigma-dsp`):**
   - **Automatic Opportunistic Passthrough:** Bit-exact DSD passthrough is an automatic, opportunistic signal path: whenever the input format is DSD (via SPA `audio/dsd`, Sony `.dsf` file, or auto-detected DoP), the input bit rate matches the target output DSD rate, digital software volume attenuation is at unity ($0\text{ dBFS}$ / 100% or routed to hardware UAC2 mixer / line-out), and the link is clock-locked or backpressured (file conversion, shared PipeWire graph clock, or synchronized stream), the stream bypasses intermediate float decimation and modulation completely, copying DSD bytes directly into the target transport packet with zero intermediate float conversions.
   - If any condition is violated (rate mismatch, software digital volume active, or asynchronous clock drift reconciliation required via ASRC across independent ALSA clock domains), the engine automatically remodulates:
     $$\text{DSD}_{\text{in}} \xrightarrow{\text{decimate}} \text{PCM}_{\text{intermediate}} \xrightarrow{\text{gain / ASRC / EQ}} \text{PCM}_{\text{intermediate}} \xrightarrow{2^K \text{ FIR cascade}} \text{DSM} \xrightarrow{} \text{DSD}_{\text{out}}$$
   - **Intermediate Rate Formula:**
     $$\text{Rate}_{\text{intermediate}} = \min\Big(\text{Rate}_{\text{in\_useful}},\; \text{Rate}_{\text{out\_useful}},\; \text{max\_intermediate\_rate\_hz}\Big)$$
     Defaulting to $352.8\text{ kHz}$ ($8\times 44.1\text{k}$) / $384\text{ kHz}$ ($8\times 48\text{k}$) for reference tier, and $176.4\text{ kHz}$ for DSD64 or low-power profiles.
   - **Scarlet Book Decimation Reference Normalization (`[input.dsd] gain_db`):** The Scarlet Book SACD specification defines $0\text{ dB}$ SACD reference level at $50\%$ modulation depth (amplitude $0.50$ relative to $\pm 1.0$), which DAC chips internally calibrate with $+6\text{ dB}$ gain so $0\text{ dB}$ SACD produces nominal line level ($2.0\text{ V}_{\text{rms}}$), matching $0\text{ dBFS}$ PCM. Unpacking DSD bits $\pm 1.0$ through a unity-gain half-band decimation cascade yields intermediate PCM at $-6.02\text{ dBFS}$. To align DSD input levels with PCM and direct DAC playback, Audigma applies a default $+6.0\text{ dB}$ ($2.0\times$) normalization gain post-decimation, user-configurable via `[input.dsd] gain_db` (default `6.0`). Modulator profile headroom ($-3.5\text{ dB}$ to $-6.0\text{ dB}$) and overload protection ensure stable remodulation headroom.

7. **Decoupled OS Process & PipeWire Graph Lifecycles (`audigma-engine` / `audigma-io`):**
   - USB DAC disconnects, power-cycles, or undocking must never cause the `audigma` daemon process to crash or exit abruptly (unless explicitly configured to fail fast with `device_wait_ms = 0`, and then only with `device_wait_expiry = "exit"`).
   - The engine decouples the **Process Lifecycle** (`[output] device_wait_ms`, default `-1` = wait indefinitely for device) from the **PipeWire Graph Lifecycle** (`[output] keep_audio_sink_ms`, default `5000` ms grace period).
   - **Error-Driven Loss Detection & Inotify Arrival Detection:** Device loss is detected by the output write path: an ALSA `ENODEV` / `EBADFD` / `EIO` returned from a write immediately triggers transition to `WaitingForDevice`. Re-appearance is detected by a Linux `inotify` watch on `/dev/snd` (`IN_CREATE`), which the daemon loop polls in short (200 ms) slices while waiting. Wake-up on arrival is event-driven (no directory scanning, no busy-spin); the slices only bound how often the shutdown flag is checked.
   - **Silent Bit-Bucket Pacing:** While in `WaitingForDevice` within the `keep_audio_sink_ms` window, `PipeWireVirtualSink` continues draining incoming frames into a silent bit-bucket at nominal stream clock, preventing PipeWire application stalls while the DAC settles or reconnects.
   - **PipeWire Output Target (same lifecycle):** With `output.backend = "pipewire"` and `output.target_node` set, that sink is the output device for `device_wait_ms`, `device_wait_expiry` and `keep_audio_sink_ms`; the same wait loop and bit-bucket pacing apply. Arrival and loss are detected from the PipeWire registry (`PipeWireSinkWatcher` tracks the sink by `node.name`) instead of `inotify`: a presence flag lets a blocked output write fail with `NotConnected` at once when the sink is removed, and the daemon enters `WaitingForDevice`. A PipeWire output without `target_node` plays into the default sink and has nothing to wait for. D-Bus device reservation is ALSA-only.
   - **No Output Loop on the PipeWire Path:** WirePlumber links an unpinned (or unmatched) output stream to the default sink, which can be audigma's own virtual input sink: the audio then recirculates inside audigma and nothing reaches a device (silent playback; level grows at about unity gain). With `target_node` set the output stream carries `target.object`, `node.dont-fallback = true` and `node.dont-move = true`, so it neither falls back to the default sink nor follows later default-sink changes. `audigma run` warns at startup when `target_node` is unset, and a guard thread samples the registry links (about every 5 s) and logs `output_loop` (ERROR) when the output stream is linked into audigma's own sink and `output_loop_cleared` when it clears. The output stream's `node.name` is `audigma_pcm_out_<instance>`; the virtual sink carries the node property `audigma.sink = "true"` so probes can skip it.

8. **Unified Volume Control Boundaries (`audigma-io` / `audigma-engine`):**
   - Controlled by `[output] volume_control = "hardware-software-fallback" | "software" | "hardware" | "none"` (default: `"hardware-software-fallback"`).
   - **`"hardware-software-fallback"` (Default):** Probes for an ALSA UAC2 hardware mixer element on the target DAC (e.g., `Master`, `PCM`, `Playback`). If found, applies analog volume attenuation on the DAC hardware while keeping digital stream audio at full scale ($0\text{ dBFS}$). If no mixer element is found, or if output is PipeWire/file, seamlessly falls back to pre-modulator floating-point PCM software attenuation.
   - **`"software"`:** Strictly scales linear floating-point PCM before the FIR cascade and DSM loop. Universally compatible across all DACs; preserves 1-bit DSD and DoP marker integrity.
   - **`"hardware"`:** Exclusively binds to the DAC's UAC2 hardware mixer element; logs an error/warning and refuses digital scaling if the device lacks a mixer.
   - **`"none"`:** Enforces fixed, bit-exact unity gain line-out; ignores or rejects desktop volume adjustments.
   - **Desktop Slider Interception:** `PipeWireVirtualSink` observes `param_changed` for `SPA_PARAM_Props` to capture desktop volume slider and mute adjustments in real time, routing them through the configured `volume_control` strategy.

---

## 2. Crate Architecture
 
The Cargo workspace is organized as a strictly layered 8-crate DAG with zero circular dependencies:
 
```text
crates/
  audigma-dsp/          # Pure math: rate types, DspConfiguration, defaults.rs, tables.rs, FIR/ASRC, 5th-order CRFB DSM, dither, FTZ/DAZ (zero OS I/O)
  audigma-format/       # Containers (WAV/FLAC/DSF) & exact bit-packers (DSD_U32_BE, DoP s32le/s24le/s24_3le)
  audigma-settings/     # TOML config Schema v1 (`Settings`), target resolver, config check & explain
  audigma-engine/       # Pure Rust pipeline planner, SPSC queues, ASRC drift controller & I/O traits (zero C FFI)
  audigma-io/           # Feature-gated concrete backends: ALSA, PipeWire, D-Bus ReserveDevice1, file/stdio & prober
  audigma-configurator/ # `audigma configure`: schema-driven form + guided setup, TUI and one-shot web UI, config writer (uses settings + engine/io prober & benchmark)
  audigma-cli/          # Binary crate (`audigma`): CLI subcommands, Unix control socket, foreground runner
  audigma-measurements/ # Dev-only harness (`measure_harness`): drives the production DSP types for tools/measure & tools/benchmark; not shipped
tools/
  design/                 # Offline scripts to synthesize FIR & Schreier NTF coefficient tables
  measure/                # Coherent windowed-FFT / impulse-response analysis and report generator (per-family reports)
  benchmark/              # Host CPU real-time deadline & throughput benchmark harness and report generator
docs/
  measurements/           # Generated per-family spectral verification reports (modulator, cascade, resampler, signal) + index (README.md)
  benchmarks/             # Host CPU throughput and real-time deadline budget benchmark reports (benchmark.reference.md)
  hardware/               # Probed DAC capability records (e.g., FiiO K17, JadeAudio JA11)
examples/                 # Ready-to-use TOML configs, MPD snippets, and WirePlumber rules
```
 
### Layer Responsibilities
- **`audigma-dsp`**: Pure numerical processing (`no_std`-friendly core logic, zero OS/file/network I/O). Implements domain rate/format types (`PcmRateHz`, `DsdBitRateHz`, `TransportFrameRateHz`), `DspConfiguration`, built-in verified filter and modulator coefficient tables (`tables.rs`, generated by `tools/design/export_tables.py`; defaults and DSD idle patterns in `defaults.rs`), polyphase & half-band FIR interpolation, state-space / CRFB delta-sigma modulators, TPDF dither, overload guards, and thread-level hardware subnormal flushing (FTZ/DAZ). Portable across any operating system.
- **`audigma-format`**: Audio container decoding/encoding (`WAV`, `FLAC`, Sony `DSF`; FLAC is encoded with the `rusty_flac` crate) and hardware frame bit-packers (`DSD_U32_BE`, `DoP` across `s32le`/`s24le`/`s24_3le`). Zero OS dependencies.
- **`audigma-settings`**: The `Schema v1` TOML configuration structs (`Settings`, including the `[runtime]` section and its cross-field validation), `config check` validation, and `config explain` resolution (mapping user settings into concrete `audigma-dsp::DspConfiguration` and transport parameters). Zero OS dependencies.
- **`audigma-engine`**: Pure Rust orchestration connecting `dsp`, `format`, and `settings` into prepared pipelines (`prepare`, `process`, `drain`), lock-free real-time SPSC queues (`rtrb`), generation tags, the ASRC drift estimator and trim, lifecycle state machine, abstract `AudioInput`/`AudioOutput` backend traits, the persistent channel worker pool with its timing histograms (`worker.rs`, `timing.rs`), and the pure CPU placement types and resolver (`cpu.rs`: `ThreadPlacement`, `CpuTopology`, `Placement`, `resolve`). The engine makes no scheduling syscalls. Zero C-FFI or OS audio library dependencies, allowing instant unit testing with mock clocks/streams on any host.
- **`audigma-io`**: Feature-gated concrete implementations of the `audigma-engine` I/O traits (`alsa`, `pipewire`, `file`, `stdio`, plus future platform backends like `coreaudio` or `asio`). Houses ALSA hardware capability probing, PipeWire virtual sink (`audigma_sink`) registration, and dynamic D-Bus device reservation (`org.freedesktop.ReserveDevice1`). Also houses the Linux scheduling side of runtime placement: the sysfs CPU topology reader (`cpu_topology.rs`) and `apply(&ThreadPlacement)` (`sched.rs`: `sched_setaffinity` plus `sched_setattr` with `SCHED_FLAG_RESET_ON_FORK`, falling back to RTKit with a finite `RLIMIT_RTTIME`). On non-Linux platforms topology discovery reports nothing and `apply` is a no-op, so threads stay unpinned at normal priority.
- **`audigma-configurator`**: Isolates UI/terminal (`ratatui`/`crossterm`) and the localhost one-shot web server so they never bloat the core DSP or engine crates. The core (`draft`, `guided`, `writer`, `assist`) has no UI dependency and is shared by both frontends. It calls into `audigma-settings` (the typed metadata catalog, `meta/*.toml`; see §6), `audigma-engine` and `audigma-io` to probe DACs and PipeWire sinks, benchmark candidate settings, and write `config.toml`. The Rust types are the single source of truth: committed JSON Schemas and generated TypeScript/zod types live under `crates/audigma-configurator/web/src/generated/` and a drift test keeps them in step.
- **`audigma-doctor`** *(planned, not yet created; its scope is to be re-agreed before it is built)*: Isolates host OS-level diagnostic and repair logic behind a platform abstraction trait (`DiagnosticProvider`). On Linux, inspects PipeWire graph clock configuration (`clock.allowed-rates`), probes ALSA device locks (`/dev/snd/pcm*`), handles interactive `--fix` (calling `wpctl set-profile <id> 0`, terminating conflicting processes with optional `sudo` escalation, or generating persistent WirePlumber drop-ins). Keeps OS-specific plumbing completely segregated from the cross-platform core and configurator.
- **`audigma-cli`**: The `audigma` executable entry point, CLI subcommand router (`clap`), multi-instance control server/client (instance-named Unix socket plus `flock` lockfile under `$XDG_RUNTIME_DIR/audigma/`, isolated in `ipc/platform.rs`), and foreground service runner. Wires runtime placement (`runtime.rs`): reads the topology, resolves `[runtime]`, and applies the result to the supervisor, `audigma-loop`, DSP workers and `pw-in`/`pw-out`.

---

## 3. Execution Modes, Rate Policies & Output Paths

### 3.1 Two Execution Modes

1. **Live Streaming Daemon (`audigma run`):**
   - Bridges a live `[input]` (`pipewire` virtual sink `audigma_sink`) through the prepared real-time DSP pipeline to a live `[output]` (`pipewire` node or exclusive `alsa` hardware device).
   - Allows both desktop applications (browsers, YouTube Music) and dedicated players (MPD) to play through the same pipeline without `Device or resource busy` lockouts.

2. **Offline File / Stream Converter (`audigma convert`):**
   - Reads a file (`WAV`/`FLAC`) or `stdin` (`raw`), runs synchronous DSP without hardware clock tracking, and writes `.dsf`, high-rate `.wav`, or raw DoP/PCM to `stdout`.
   - Supports deterministic seeding, two-pass true-peak headroom analysis, and exact sample-count duration trimming.

### 3.2 Clock Family Policies (`[target] rate_family`) & Dynamic Multi-Rate Negotiation

Rather than an artificial "desktop vs. music mode" switch on the input, output clock switching is controlled purely by `[target] rate_family`:

1. **`rate_family = "44100"` or `"48000"` (Zero-Relock / Pinned Clock Policy):**
   - Keeps the DAC hardware clock permanently locked at that family (e.g. `11.2896 MHz` for `dsd256` under `"44100"`, or `12.288 MHz` under `"48000"`; for PCM, locked to `pcm_rate_hz`).
   - Any input rate from the other clock family is seamlessly bridged into that single target clock via polyphase rational SRC + $2\times$ FIR cascade.
   - Ideal for general desktop listening to eliminate DAC hardware relay clicks or relock delays when switching between browser, Spotify, and videos.

2. **`rate_family = "source"` (Family-Preserving Dynamic Policy - Default):**
   - Dynamically switches the output rate between its `44.1 kHz`-family target (e.g., `705.6 kHz` / `DSD256-44.1`) and `48 kHz`-family target (e.g., `768 kHz` / `DSD256-48`) whenever the input stream changes rate family.
   - For DSD targets, symbolic `dsd_rate = "dsd256"` automatically resolves to `11.2896 MHz` for 44.1 kHz-family inputs and `12.288 MHz` for 48 kHz-family inputs.
   - For PCM targets, `pcm_rate_hz` automatically matches the oversampling tier (e.g. `768000` for 48k matches to `705600` for 44.1k via automatic tier matching).
   - Avoids cross-family rational SRC so upsampling uses pure $2^K$ half-band FIR stages (plus low-rate ASRC drift tracking when driving an independent ALSA clock).

3. **Dynamic Multi-Rate PipeWire Virtual Sink (`[input] rate_mode = "dynamic"`):**
   - When running as a live desktop daemon, `PipeWireVirtualSink` advertises `audio.allowed-rates` in its adapter node properties (e.g., `[ 44100 48000 88200 96000 176400 192000 352800 384000 ]`) and serializes an SPA `Choice::Enum` POD covering all supported input rates.
   - When an upstream application begins playback at a different rate, PipeWire notifies the sink via `param_changed` on `SPA_PARAM_EnumFormat`.
   - The engine intercepts the format transition, atomically increments the pipeline generation tag (`generation += 1`), smoothly mutes and drains active filter tails, re-prepares the pipeline (`prepare(new_spec, config)`), and resumes audio without restarting the daemon process.

### 3.3 Three Supported Live Output Paths

| Live Output Path | `[output]` Config | `[target]` Config | Clock Domain & Device Ownership |
|---|---|---|---|
| **1. PipeWire High-Rate PCM Upsampler** | `backend = "pipewire"`<br>`target_node = "alsa_output..."` | `format = "pcm"`<br>`sample_format = "f32"` | **PipeWire-rate-matched output** (no ASRC); PipeWire retains ownership of the DAC. |
| **2. Direct Exclusive ALSA PCM (NOS / High-Rate)** | `backend = "alsa"`<br>`device = "hw:CARD=K17,DEV=0"` | `format = "pcm"`<br>`sample_format = "s32le"` + `dither = "tpdf"` | DAC is master clock. With the demand-driven PipeWire sink no ASRC (`auto` → off); a free-running input gets drift-correcting ASRC (`auto` → `Correct`). `audigma` owns the ALSA DAC exclusively. |
| **3. Direct Exclusive ALSA DSD (Native / DoP)** | `backend = "alsa"`<br>`device = "hw:CARD=K17,DEV=0"`<br>`transport = "native" \| "dop"` | `format = "dsd"`<br>`dsd_rate = "dsd256"` | DAC is master clock. With the demand-driven PipeWire sink no ASRC (`auto` → off); a free-running input gets drift-correcting ASRC (`auto` → `Correct`). `audigma` owns the ALSA DAC exclusively. |

### 3.4 Prepared Pipeline Stages & Worker Threading

Every active pipeline executes the following ordered stages:

```text
decode / unpack / DSD decimation
  → channel validation & ordering (explicit channel_positions, order strictly preserved)
  → volume control (software PCM scale if active) + preamp_db + profile headroom gain (f64 PCM)
  → low-rate adaptive/rational polyphase SRC (R_asrc × R_rational)
  → DC blocker (first-order 1 Hz high-pass; DSD targets only) [Channel Worker Pool]
  → staged 2× half-band FIR interpolation cascade (2^K) [Persistent RT Channel Worker Pool]
  → modulator-input peak meter & overload guard
  → PCM quantizer (+ TPDF dither for integer PCM)  OR  1-bit Delta-Sigma Modulator (CRFB) [Channel Worker Pool]
  → transport packer (Native DSD_U32_BE / DoP s32le|s24le|s24_3le / DSF / PCM)
  → backend output (ALSA HW mixer volume if active)
```

- **Multi-Channel Topology & Order-Preserving `channel_positions`:**
  - Configured via `[input] channels = N` and `channel_positions = [ ... ]` (e.g., `[ "FL", "FR" ]`).
  - **Strict Order Preservation Invariant:** If `channel_positions` is explicitly declared, Audigma validates that `channel_positions.len() == channels as usize` and that all entries are valid channel tokens, but **never** reorders, normalizes, or overrides the declared order. This preserves intentional channel remapping (e.g., reversed stereo: `[ "FR", "FL" ]`).
  - **Default Synthesis:** When omitted, standard channel layouts are automatically synthesized:
    - `1` $\to$ `[ "MONO" ]`
    - `2` $\to$ `[ "FL", "FR" ]`
    - `4` $\to$ `[ "FL", "FR", "RL", "RR" ]`
    - `6` $\to$ `[ "FL", "FR", "FC", "LFE", "RL", "RR" ]` (5.1 surround)
    - `8` $\to$ `[ "FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR" ]` (7.1 surround)
  - **Offline file input topology:** WAV/FLAC decoding conditions input to stereo in floating-point PCM before any gain or resampling: mono is duplicated to both channels at unity gain, stereo passes through, and more than two channels are folded down per ITU-R BS.775.
  - Both PipeWire adapter node properties (`audio.channels`, `audio.position`) and SPA `AudioInfoRaw` channel arrays are constructed dynamically from the resolved `channel_positions`.

- **Persistent Real-Time Channel Worker Pool (`[runtime] dsp_threads`):**
  - The pool is created once per daemon (on the first stream, inside the `audigma-loop` thread) and survives rate switches, format changes and device reconnects. A rate or filter change builds fresh per-channel state (cascade, modulator, input processor, buffers) off the hot path and hands it to the running workers (`reconfigure`); thread count, placement and RT requests are unchanged. The channel count is fixed for the daemon's life.
  - **Worker count is decoupled from channel count.** Channel `c` runs on worker `c % workers`; a worker processes its channels sequentially. `dsp_threads = "auto"` (default) uses `min(channels, max(1, usable physical cores - 1))`, i.e. one worker per channel for stereo on 4+ cores. An integer fixes the count (capped at the channel count). `"inline"` creates no worker threads: all channels run sequentially on the `audigma-loop` thread through the same per-channel code (bit-identical output, no futex hand-off). `auto` never chooses inline or a single worker because PCM load is unmeasured. A single worker or inline needs roughly twice the time of a stereo pair of workers, so it suits small CPUs and light targets only (on the reference machine, up to about DSD256).
  - **Thread structure and priorities.** The supervisor (main) thread owns startup, IPC and shutdown. `audigma-loop` runs the feed loop; `audigma-dsp-N` are the workers; `audigma-pw-in`/`audigma-pw-out` are the PipeWire threads. With `realtime_priority = p` (default 20, range 3-99): loop `p`, `pw-in`/`pw-out` `p-1`, workers `p-2`; every other thread (IPC, rt-log, late, health, D-Bus) stays `SCHED_OTHER`. RT is `SCHED_FIFO` with `SCHED_RESET_ON_FORK`; when only RTKit can grant it, the same ladder lands as `SCHED_RR`. PipeWire's own data loop is not ours (rtkit assigns it).
  - **Affinity.** DSP workers get one exclusive core each, chosen by `core_selection` (`performance` fast cores first, `efficient` slow first, `os` no automatic pinning) and `smt_siblings` (`avoid` uses SMT siblings only after all physical cores; `never` fails at startup instead). Cores are ranked by the first metric every allowed core exposes (`highest_perf`, `cpuinfo_max_freq`, `cpu_capacity`) and clustered into performance classes; if no metric exists the workers are left unpinned. Everything else (loop, `pw-*`, IPC, logs, health) runs in the *housekeeping set*: the allowed CPUs minus the DSP cores and their SMT siblings. `loop_cpu` and `dsp_cpus` override the automatic choice. Affinity is inherited by threads a thread spawns, so the supervisor narrows itself first and threads needing a specific placement apply it on entry.
  - **Spin rule.** A worker busy-spins (up to 2048 iterations) before sleeping on a futex only when it owns its physical core and no other audigma thread shares an SMT sibling. Otherwise it sleeps immediately. The dispatcher spins at the barrier only if every worker may.
  - **Startup handshake.** Workers report their placement result to the pool constructor and the loop reports to the supervisor. With `realtime = "required"` any failure at startup aborts; with `"auto"` it logs an `rt_priority` event and continues at normal priority; `"off"` never requests RT. After startup, `pw-in`/`pw-out` re-request RT when re-created on reconnect; a failure there is never fatal (warn, `rt_priority` event, continue).
  - Synchronization between loop and workers is a lock-free, cache-line aligned generation barrier with futex fallback. Workers process non-overlapping slices of preallocated shared buffers; hot buffers are pre-touched in `prepare` so first-touch page faults do not land in the hot loop (`mlockall` is deliberately not used: under a finite `RLIMIT_MEMLOCK` it would break arena creation).
  - **Zero Copies & Zero Heap Allocation:** no heap allocation or lock contention in steady state.
  - **Cache Locality:** Filter delay lines and modulator integrator states remain strictly thread-local to the worker that owns the channel, while filter coefficient tables are read concurrently from shared static memory.
  - **Offline input stage:** `convert` feeds the same worker input stage as the live loop (`process_input_pcm`, or `process_input_dsd` for DSF input, which offers `dsd_decode_config` so the workers decode raw DSD; PCM decimation, the constant pre-modulator gain, rational SRC, then cascade and modulator), with a constant `FrameGain::Preserve` envelope and no drift correction. At end of input it flushes by pushing digital silence through that same stage, so the SRC tail is rendered. Two-pass pass 1 uses the same input stage with a command that stops after the cascade (`process_input_pcm_interpolated`).
- **Offline commands** (`convert`, `play`, `benchmark`) use the same pool and the same `dsp_threads`, `core_selection`, `smt_siblings` and `dsp_cpus` settings, resolved for two channels (offline is stereo). They never request RT, regardless of `realtime`.
  - **Telemetry.** Each worker times its own execution (excluding wake-up and barrier wait) into a fixed-bucket histogram (one relaxed atomic increment per tile); the loop times the whole dispatch (wake-up plus barrier) separately. `audigma-health` only reads these, and reports mean, p50, p99 and max per 10 s window plus cumulative p99.9 in `loop_health` and `late_slice` (`worker_exec_*`, `worker_p99_us` per worker, `dispatch_*`). In inline mode there is a single timing entry. `late_slice` also carries `headroom_min_us` and `buffer_peak_us` (device-buffer headroom over the slice): it is logged at WARN only when headroom fell below half of the peak queued audio (or is unknown), otherwise at INFO, and `late_start` is DEBUG. `[logging] rt_report_interval_ms` sets the cadence of these slices and of `loop_health`. `loop_stall` records carry a `cause` (`input_empty`, `dsp_slow`, `emit_slow`, `read_slow`, `prep_slow`, `unknown`).

- **Zero Allocation & Generation Tags in Real-Time:** `prepare(spec, config)` resolves all filter stages, coefficients, the worker pool, and preallocated `rtrb` SPSC ring buffers off the audio thread. `process(input, output)` never allocates on the heap, locks mutexes, or performs I/O. SPSC queue chunks carry a monotonic **generation tag** (`u64`) incremented on every pipeline/format reconfiguration so stale-format blocks in flight are immediately discarded and can never reach a newly configured audio endpoint.
- **Bounded Tile Streaming:** Upsampling to `DSD256` ($256\times$) and `DSD512` ($512\times$) expands baseband frames into large intermediate buffers. DSD-configured channel workers split each externally scheduled tile into internal subtiles targeting at most 16,384 expanded samples (32 baseband frames at $512\times$, 64 at $256\times$). Each subtile passes through the full FIR cascade and is immediately modulated, without additional worker synchronization or a change to the outer audio deadline. Ping-pong scratch buffers hold only the largest intermediates of one subtile. The full interpolated PCM output remains available through `channel_pcm()` for compatibility; this is subtile streaming, not a fused FIR/modulator kernel or a guarantee of L1 residency. PCM-only workers retain their existing tile sizes.

---

## 4. DSP Design: Resampling, Modulation, and Transitions

### 4.1 Reference Levels, Baseband OSR & Subnormal Handling
- Internal PCM uses normalized `f64` samples where `1.0` equals `0 dBFS`.
- The 1-bit quantizer emits `-1.0` or `+1.0`, mapped to `0` and `1` bits during packing.
- **Baseband Over-Sampling Ratio ($\text{OSR}$):**
  $$\text{OSR} = \frac{f_{\text{bit}}}{2 \cdot B}$$
  where $B$ is the design audio baseband ($20\text{ kHz}$). Note that DSD's rate multiplier ($64, 128, 256, 512$) is relative to $44.1\text{ kHz}$, **not** $2 \cdot B = 40\text{ kHz}$. Thus, conventional `DSD256` at $11.2896\text{ MHz}$ with $B = 20\text{ kHz}$ has $\text{OSR} = 282.24$, `DSD512` at $22.5792\text{ MHz}$ has $\text{OSR} = 564.48$ (and `DSD64` has $\text{OSR} = 70.56$). NTF synthesis scripts in `tools/design/` must always pass the exact $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ to Schreier synthesis routines.
- **DC Blocker (DSD targets only):** A DC offset $u$ at the modulator input produces idle tones near $u \cdot f_{\text{bit}}$ that the TPDF dither does not remove (e.g. -80 dBFS DC: 164 Hz at DSD64, 25-36 dB above the local noise; `docs/measurements/signal.md`). Every DSD-target channel worker therefore runs `audigma_dsp::DcBlocker` on the baseband PCM right before the cascade: $y[n] = g\,(x[n] - x[n-1]) + R\,y[n-1]$, $R = 1 - 2\pi f_c / f_s$, $g = (1 + R)/2$ (unity gain at Nyquist, below unity elsewhere), $f_c = 1\text{ Hz}$ at the cascade input rate, `f64`, per channel. Response: $-0.011\text{ dB}$ and $+2.9°$ at 20 Hz, $< 10^{-5}\text{ dB}$ from 1 kHz up; time constant 159 ms (a DC step at stream start decays within ~1 s). It is reset with the cascade (`reset`, `reset_output`). It runs after volume and headroom gain (it is LTI, so order does not matter for steady gain), on the live, offline and DSD-input re-modulation paths and in two-pass pass 1. PCM targets and bit-perfect / DSD passthrough paths do not run it. A high-pass can raise the peak of infrasonic-heavy material (e.g. a 0 dBFS 20 Hz square wave by about 0.7 dB, besides the cascade's own Gibbs overshoot); two-pass pass 1 measures that, live paths rely on the overload policy (§4.4). There is no setting.
- **Hardware FTZ/DAZ:** Inside the 1-bit DSM loop, feedback is strictly `{-1.0, +1.0}`, keeping integrators excited. However, upstream PCM FIR/IIR stages fed digital silence can decay into subnormal floats and cause severe CPU tail-latency spikes. Every DSP worker thread sets hardware **Flush-To-Zero (FTZ)** and **Denormals-Are-Zero (DAZ)** (`MXCSR` on `x86_64`, `FPCR` on `aarch64`) on thread entry.

### 4.2 Multistage Resampling & Filters
1. **Polyphase Windowed-Sinc & ASRC:** Handles cross-family rational ratios (`44.1 kHz ↔ 48 kHz` at any tier, and PCM targets below the matched tier such as `96 kHz → 44.1 kHz`) and smooth fractional-phase clock drift compensation ($R_{\text{asrc}}(t)$) at the baseband rate.
   - One closed-form Kaiser-windowed-sinc prototype (`tools/design/design_src.py`, parameters exported to `tables.rs`); each `PolyphaseResampler` builds its bank from it at construction (`fir::src_prototype`), and the measurement tool checks the compiled bank against the Python design.
   - Band edges are relative to the **lower** of the two rates (scale $\min(1, f_{\text{out}}/f_{\text{in}})$, taps grow as $1/\text{scale}$), so a downsampler is its own anti-alias filter: passband to 20 kHz, stopband from 24.1 kHz (at 44.1 kHz, scaled with the rate), $\ge 140\text{ dB}$ rejection, passband deviation $< 10^{-6}\text{ dB}$. Transition-band images/aliases land in 20–24.1 kHz, outside the audio band (same convention as the half-band cascade).
   - Rational mode: exact output phases (integer phase accumulator, one bank row per reduced output phase, e.g. 160 rows for 44.1k → 48k, 147 for 48k → 44.1k). ASRC: 256 rows plus guard rows, 4-point Lagrange interpolation between rows (interpolation error far below the prototype stopband).
2. **Staged $2\times$ Half-Band FIR Cascade:** Power-of-two upsampling ($2^K$) cascades $2\times$ half-band FIR stages; linear-phase stages use the folded symmetric kernel (zero even taps, 0.5 centre tap). There is no quality preset: every filter is designed for $\ge 140\text{ dB}$ image rejection (`tools/design/design_halfband.py`, exported to `tables.rs`).
   - **Construction:** exact half-band (Vaidyanathan-Nguyen): a one-band minimax filter of length $(N+1)/2$ is interleaved with the 0.5 centre tap, with DC pinned inside the minimax (a narrow, heavily weighted band at DC), so unity DC costs no rejection. A half-band has equal pass and stop ripple, so the rejection fixes the passband deviation ($\approx 10^{-6}\text{ dB}$); there is no separate ripple goal.
   - **Per-stage designs:** stage $k$ outputs at $88.2\text{ kHz} \cdot 2^k$ (44.1 kHz family; the 48 kHz family runs the same tables, so every edge scales by 48/44.1). Passband 20 kHz at every stage, stopband at the mirror $f_s/2 - 20\text{ kHz}$. Stage 0 has 199 taps (145.7 dB), stage 1 31 taps; stages 2+ use the shortest length that reaches the target (19/15/11/11/7/7/7 taps for stages 2..8). `HalfBandCascade::new` picks the filter by stage index.
   - **Decimation** (`HalfBandDecimationCascade`, DSD and PCM inputs above the target tier) runs the same designs from its output backwards: its last stage (into the output rate) is stage 0, the one before it stage 1, and so on, so each stage rejects exactly what would alias into the output passband. Decimation is always linear phase.
3. **Filter Controls (`[resampler]`):**
   - `phase = "linear" | "minimum"`: minimum phase is the homomorphic minimum-phase equivalent of each linear-phase design (same magnitude, causal energy).
   - `apodization = "off" | 18000 | 18500 | 19000 | 20000` (default `"off"`; `"gentle"` is an alias for `19000`; any other value fails validation). `"off"` uses the half-band stage 0 (stopband 24.1 kHz). A pass edge (Hz, 44.1 kHz-family sources; scaled by 48/44.1 for 48 kHz sources) replaces stage 0 with a Kaiser-windowed apodizing low-pass from that edge to a 22.05 kHz stopband (the source Nyquist, which is what makes it apodizing), $\ge 140\text{ dB}$, linear or minimum phase. Apodization affects stage 0 only.

| Apodization | Stage-0 taps | -3 dB point | Gain @ 19 kHz | Gain @ 20 kHz | Gain @ 22.05 kHz |
|---|---:|---:|---:|---:|---:|
| `off` | 199 | 21.8 kHz | 0.0 dB | 0.0 dB | -6.0 dB |
| `18000` | 209 | 19.7 kHz | -0.2 dB | -5.7 dB | $\le -140$ dB |
| `18500` | 237 | 20.0 kHz | 0.0 dB | -2.8 dB | $\le -140$ dB |
| `19000` | 277 | 20.3 kHz | 0.0 dB | -0.8 dB | $\le -140$ dB |
| `20000` | 409 | 20.9 kHz | 0.0 dB | 0.0 dB | $\le -140$ dB |

### 4.3 1-Bit Delta-Sigma Modulator (CRFB)
- **Topology:** `f64` 5th-order Cascade of Resonators with Feedback (**CRFB**) as the baseline modulator for `DSD64`, `DSD128`, `DSD256`, and `DSD512` (with 7th-order CRFB optional for higher OSRs).
- **NTF Synthesis:** Offline Schreier-style NTF synthesis (`tools/design/`) using $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ with optimized resonator zeros in the 0–20 kHz audio band (roots of the Legendre polynomial of the loop order, which minimise integrated in-band noise), Butterworth-prototype poles tuned to $H_\infty = 1.40$ for every rate and order (1.30 buys about 1 dB of MSA for about 17 dB of in-band SNR; 1.50 lowers the MSA to about -5 dBFS), unscaled (all `c = 1`; `f64` states need no dynamic-range scaling) and verified against maximum stable input amplitude ($L\text{ dBFS}$) across DC, coherent tones, multitone bursts, and silence.
- **Loop Evaluation (prefix-sum form):** The CRFB update $x' = L^{-1}(R\,x + b_1 u\,e_0 - a\,v)$ is evaluated expanded, not stage by stage. With all `c = 1`, $L^{-1}$ is a prefix sum, so $x'_k = p_k - \gamma_k v$, where $w_k = x_k - g\,x_{k+1}$ on resonator stages ($x_k$ elsewhere, $w_0 = (x_0 + b_1 u) - g_1 x_1$), $p$ are the prefix sums of $w$ computed as a depth-3 parallel prefix, and $\gamma_k = a_1 + \dots + a_k$ (exported per table as `gamma`, summed left to right; synthesis rejects any $\gamma_k < 0$). The comparator is the sign bit of the quantizer input $y$ (state plus dither): $v = \operatorname{copysign}(1, y)$ (so $y = -0.0$ decides $-1$), applied as $\operatorname{copysign}(\gamma_k, y)$. Same topology, NTF and coefficients as the stage-by-stage form; it keeps the decision off a branch and out of the integrator chain, so the loop-carried path is a few FP ops instead of two per stage (1.5–1.6× fewer ns per bit at DSD512, baseline `x86-64` build). The operation order is fixed and mirrored exactly by `tools/design/dsm_analysis.crfb_step` (`simulate_crfb`) and the MSA kernel in `design_crfb.py`; changing it in one place breaks the bit-exact check (§8). The speed depends on codegen: a branch on $v$ or a sign round trip through integer registers loses most of the gain (`docs/benchmarks/benchmark.reference.md` shows it).
- **PCM quantizer:** One shared `audigma_dsp::PcmQuantizer` serves the live loop and the offline WAV, FLAC and raw writers. Integer targets (`s16le`, `s24le`, `s24_3le`, `s32le`) are scaled to the format's positive full scale, dithered iff `target.dither = tpdf` (peak 0.5 LSB, one stream per channel seeded from `dither_tpdf_seed`, default 12345), clamped and rounded; `s24le` words carry 24-bit codes. Float targets are written verbatim. FLAC quantizes as the format's own depth: `s16le` → 16, `s24le` and `s24_3le` → 24, `s32le` → 32 bit; float formats have no FLAC representation.
- **Dither:** Optional TPDF dither (amplitude 0.005) injected prior to the 1-bit quantizer. Measured, it has no effect on in-band noise or spurs at that amplitude (`signal.md`); it is kept as harmless. DC idle tones are removed by the DC blocker (§4.1), not by the dither.
- **Input Limit & Baseline Headroom:** The input limit $L$ belongs to the modulator (order and DSD rate): every coefficient table (`tables.rs`, per order and DSD rate) carries its measured $L$ (`max_stable_input_dbfs`) and its $H_\infty$. $L$ comes from `design_crfb.measure_msa_dbfs`: DC and 50 Hz–20 kHz sines, level raised in 0.25 dB steps from -8 dBFS, $2^{21}$ samples each, without dither and with two TPDF dither sequences (production dithers), unstable when any state exceeds the overload bound (50); $L$ is one 0.25 dB guard step below the last level stable in every trial (at the edge itself stability is statistical; DC is the hardest stimulus). The modulator report runs the production modulator at exactly $L$ for the same stimuli and fails on any overload. Current values: 5th order -4.0 to -4.25 dBFS, 7th order -4.5 to -4.75 dBFS (`docs/measurements/modulator.md` lists them). For DSD targets the baseline headroom is $L - M$, so a 0 dBFS PCM peak reaches the modulator at $L - M$; `config explain` reports $L$ for the resolved target and the total attenuation (`preamp_db + volume_db + profile_headroom_db`).

### 4.4 Headroom Inequality, Two-Pass Offline Peak Analysis & Live Overload Protection
- **Required Attenuation Equation:** Given a predicted or measured post-interpolation peak of $P\text{ dBFS}$, the selected modulator's maximum stable input limit $L\text{ dBFS}$ (§4.3, e.g. $-4.0\text{ dBFS}$ for the 5th order at DSD64), and a safety margin $M\text{ dB}$ (`[gain] dsd_safety_margin_db`: exactly `0`, `0.25` or `0.5`; default `0.25`; DSD targets only: sets the profile headroom `L - M` and the two-pass gain; PCM targets use `M = 0`; `0` is proven for the measured stimuli, not arbitrary inter-sample peaks, and overloads fall back to the overload policy), the total applied pre-modulator gain must satisfy:
  $$\text{gain\_db} \le L - M - P$$
- **PCM targets:** The profile attenuation and the input limit $L$ describe the 1-bit modulator, so they apply to DSD targets only. For PCM targets the profile headroom is $0\text{ dB}$ and $L = 0\text{ dBFS}$ (full scale).
- **Offline Two-Pass Gain (`peak_analysis = "two-pass"`, `convert` only; `run` and `play` ignore it):** Because the PCM SRC + $2\times$ FIR interpolation cascade is Linear and Time-Invariant (LTI), scaling the pre-SRC input by gain $G$ scales the reconstructed peak entering the modulator by exactly $G$. Therefore, **Pass 1 runs only the PCM interpolation cascade** (with the DC blocker of §4.1 ahead of it for DSD targets, also LTI; skipping the sequential 1-bit DSM loop and bit packing) to measure the true inter-sample peak $P\text{ dBFS}$, solves $\text{gain\_db} = \min(\text{configured\_gain\_db},\; L - M - P)$, and **Pass 2 runs the complete pipeline**.
- **Live Overload Guard (`overload_policy = "mute-recover"`):** Monitors the PCM signal entering the modulator and the internal integrator states. If an unexpected transient exceeds the modulator's stable envelope or drives states toward instability:
  1. Immediately bypass the filter tail and switch the output to the verified DSD idle stream at the modulator boundary.
  2. Reset and reprime the modulator state vector to zero.
  3. Resume audio with a smooth pre-SRC PCM fade-in once input levels return to normal.

### 4.5 Lifecycle & Click-Free Transitions
The engine state machine runs through:
`Stopped → Preparing → Priming → Running → Draining/Muting → Reconfiguring` (plus `WaitingForDevice`, `Recovering`, `DeviceLost`).

- **Planned Stop / Rate Switch:** Ramp PCM gain to zero pre-SRC $\to$ run `filter_group_delay` frames until the FIR tail clears $\to$ emit the balanced DSD idle pattern (`0x69`) while holding DoP marker alternation (`0x05`/`0xFA`) during device settling.
- **Resilient Device Disconnect & Reconnect Sequence:**
  $$\text{Running} \xrightarrow{\text{ALSA ENODEV}} \text{WaitingForDevice (draining bit-bucket)} \xrightarrow{\text{inotify /dev/snd arrival}} \text{Re-reserving D-Bus} \xrightarrow{\text{re-open ALSA}} \text{Priming} \xrightarrow{\text{cosine ramp}} \text{Running}$$
  1. *ALSA Write Error:* When an active hardware device disconnects (unplug or power-off), the next ALSA write fails with `ENODEV` (or `EBADFD` / `EIO`). The engine transitions immediately to `WaitingForDevice`.
  2. *Bit-Bucket Pacing & Grace Period:* If `keep_audio_sink_ms` is positive or `-1`, `PipeWireVirtualSink` stays registered in the PipeWire graph and discards incoming audio frames at nominal stream clock rate. If `keep_audio_sink_ms` expires before the hardware returns, the sink node unregisters from PipeWire to allow desktop audio routing to fall back cleanly to secondary speakers.
  3. *Event-Driven Hotplug Arrival:* Linux `inotify` on `/dev/snd` (`IN_CREATE`), polled in 200 ms slices that also check for shutdown, wakes the daemon on DAC reconnection without busy-spinning. Audigma re-acquires the D-Bus device reservation (`org.freedesktop.ReserveDevice1`), re-opens the ALSA device, re-primes the carrier, re-registers `PipeWireVirtualSink` (if previously torn down), and fades in audio smoothly with a cosine ramp.

---

## 5. Linux Transport, Bit-Packing & File Containers

### 5.1 DSD Rate & Transport Relationships (44.1 kHz Family)

| Target | DSD Bit Rate / ch (`DsdBitRateHz`) | DoP Frame Rate (`TransportFrameRateHz`) | ALSA `DSD_U32` Frame Rate (`TransportFrameRateHz`) | SPA DSD Byte Rate / ch |
|---|---:|---:|---:|---:|
| `DSD64` | `2,822,400` | `176,400` | `88,200` | `352,800` |
| `DSD128` | `5,644,800` | `352,800` | `176,400` | `705,600` |
| `DSD256` | `11,289,600` | `705,600` | `352,800` | `1,411,200` |
| `DSD512` | `22,579,200` | `1,411,200` | `705,600` | `2,822,400` |

*(For 48 kHz-family DSD, multiply `48,000` by `64/128/256/512`; e.g., `DSD256/48 = 12,288,000 Hz`, `DSD512/48 = 24,576,000 Hz`).*

### 5.2 Exact Bit-Packing Formats (`audigma-format`)

1. **ALSA Native `DSD_U32_BE` / `DSD_U32_LE`:**
   - Packs 32 consecutive 1-bit DSD samples per channel into each 32-bit ALSA container word (`f_frame = f_bit / 32`), MSB-first bit ordering (`bitrev=0` unless device quirks specify LSB-first).
2. **DoP (DSD over PCM v1.1) — Three ALSA Container Alignments & Auto-Detection:**
   - Carries 16 DSD bits (2 bytes: `HI`, `LO`, oldest bit in MSB) per channel per PCM frame, with an 8-bit marker alternating `0x05` and `0xFA` on consecutive frames (synchronized across stereo channels).
   - Because DACs expose different 24/32-bit integer formats over ALSA, `audigma-format` implements and tests all three alignments. `output.dop_sample_format` picks one or `auto` (the default; see below), and each opens the ALSA format whose bit placement it matches, so the marker always lands in the DAC's top 24 bits:
     - **`s32le` → ALSA `S32_LE` (32-bit container, MSB-justified):** 24-bit DoP word shifted left by 8 (`0xMM_HI_LO_00`, written little-endian as `[0x00, LO, HI, MM]`). Verified on the FiiO K17 (whose only formats are `S16_LE`/`S32_LE`). Not every DAC detects DoP here: the JadeAudio JA11 opens `S32_LE` as true 32-bit PCM (`Bits: 32`) and plays DoP in it as PCM noise.
     - **`s24le` → ALSA `S24_LE` (32-bit container, LSB-justified):** 24-bit DoP word in the lower 24 bits (`0x00_MM_HI_LO`, written little-endian as `[LO, HI, MM, 0x00]`). Typical of I2S/SoC cards; MPD tries this format first for DoP.
     - **`s24_3le` → ALSA `S24_3LE` (Packed 24-bit container, 3 bytes/sample):** Written little-endian as `[LO, HI, MM]`.
   - **`output.dop_sample_format` values:** `auto` (default), `s32le`, `s24le`, `s24_3le`. DoP needs a 24-bit word, so any other sample format (`s16le`, float, native DSD words) is rejected when the settings are parsed. The setting is `audigma_settings::DopSampleFormat` (what was asked; may be undecided). Packers take `audigma_format::DopContainerFormat` (what is written; always decided), which has no `auto`. `DopSampleFormat::candidates()` / `without_device()` are the only bridge between them.
   - **Negotiation (`auto`):** `AlsaAudioOutput::open` tries the words in the order `s24_3le`, `s24le`, `s32le` on the PCM handle it is opening (fresh hw_params per word: format, channel count, exact DoP frame rate) and opens the first the device accepts. DoP v1.1 is defined on 24-bit samples, and a DAC that also offers `S32_LE` may only detect DoP on its 24-bit format (JA11), so the 24-bit words lead and `s32le` is the fallback for DACs with no 24-bit format (K17); MPD uses the same rule (`S24_LE`, `S24_3LE`, then `S32_LE`). USB DACs never offer `S24_LE`, so the order between the two 24-bit words does not matter for them. A pinned word is tried alone and fails the open if refused (no fallback). Through `plughw:`/`default` every word is accepted, so `auto` opens `S24_3LE`; the plug's conversions between these words only add or drop the zero pad byte, so the DoP word stays bit-exact (whether the DAC then detects DoP depends on the format the plug opens underneath).
   - **Invariant — one DoP word per stream:** the packed bytes must be in the word the device was opened with. The opened output reports it via `AudioOutput::dop_format()`; the live loop (`IdleCarrierFormat::for_output`, re-resolved on every stream start, so on each device reopen) and the offline writer `play` streams through both take it from there, never from the settings. Sinks with no device (files, pipes) report `None`, and the configured word applies: `auto` writes `s32le`. `-o -` needs a pinned word (§5.3).
   - **DoP Ingestion via Auto-Detection (128 Alternating Frames):** `audigma-format` automatically identifies incoming DoP streams inside PCM carriers by observing 128 consecutive alternating `0x05`/`0xFA` marker frames across all active channels ($\approx 0.72\text{ ms}$ at DoP64). This provides zero false positives on natural PCM, requires no manual transport declaration, and automatically unpacks the 16-bit DSD payload per channel.
3. **Sony DSF Container (`.dsf`):**
   - Little-endian `DSD `, `fmt `, and `data` chunks; interleaves channels in fixed **4096-byte blocks per channel** (`L[4096], R[4096]`), with declared bit ordering (`1` = LSB-first per Sony DSF v1.01, `8` = MSB-first) and exact per-channel `sample_count` in bits.
   - **Final Block Padding (`dsf_pad_mode`):** Sony DSF v1.01 §4 specifies `0x00` fill for unused trailing bytes in the final 4096-byte block. However, naive players that ignore `sample_count` play those trailing `0x00` bytes as full-negative DC (~11.6 ms pop at DSD64). `audigma` supports:
     - `dsf_pad_mode = "spec-zero"` (default: standard `0x00` padding with exact `sample_count`).
     - `dsf_pad_mode = "dsd-silence"` (fills trailing pad bytes with `0x69` balanced DSD silence to prevent end-of-track pops on broken players).

### 5.3 Stdio Streaming & Container Seekability Constraints
- **Container selection:** there is no config key. `audigma convert` takes the container from the output file extension (`.dsf`, `.wav`, `.flac`, `.raw`; case-insensitive, `.wave` is WAV); `-o -` is a raw stream. A missing or unknown extension is an error, and the target must fit the container (`.dsf` needs `target.format = "dsd"`, `.wav` / `.flac` need `"pcm"`; FLAC also rejects `f32` / `f64`). A `[output] container` key in a config is an unknown-key error. One config can therefore write several containers. The engine takes the container as an explicit argument (`OfflinePipeline::with_container`); without it, DSD targets write DSF and PCM targets write WAV.
- **Raw Streams (`.raw` or `-o -`):** Require complete format declarations (`sample_format`, `rate_hz`, `channels` on input; `transport` on output; `target.sample_format` for PCM). For DoP, a `.raw` file with `output.dop_sample_format = "auto"` (or unset) is written as `s32le`, as any DoP file without a device to negotiate with (§5.2); `-o -` requires a pinned word (`s32le`, `s24le` or `s24_3le`), since the reader at the other end of the pipe must be told the layout. `stdout` must carry audio payload bytes only; all logs and diagnostics go strictly to `stderr`.
- **Header-Finalized Containers (`dsf` / `wav`):** Writing valid `.dsf` or `.wav` headers requires either a **seekable output file** (to patch chunk sizes and `sample_count` at EOF) or a known-length strategy. `audigma convert -o -` must explicitly reject unsupported non-seekable `stdout` combinations (a `-o -` stream is always raw, so a header container cannot be requested for a pipe) rather than emitting corrupt headers.
- **Pipe Resilience:** Handle pipe backpressure, partial frame reads/writes, clean `EOF` draining, and `EPIPE` (broken pipe) gracefully without panicking.
- **Album-Continuous Processing:** When converting multi-track albums, state (filter ring buffers, modulator integrators, dither state) must be preserved across track boundaries, splitting output files at exact rational sample boundaries to guarantee click-free gapless playback.

---

## 6. Configuration Schema (`Schema v1`)

Configuration path resolution, first match wins: `-c` / `--config` (and `--out` for writes), then `$AUDIGMA_CONFIG`, then `$XDG_CONFIG_HOME/audigma/config.toml`, then `~/.config/audigma/config.toml`. Every command that reads a config (`run`, `play`, `convert`, `benchmark`, `config check`, `config explain`) uses this order, and so does `configure` for reads and writes. Unknown keys or invalid cross-field combinations are rejected by `audigma config check`.

**Live-only backends.** `[input] backend` is optional and has one value (`pipewire`); there is no ALSA input backend and no `input.device`. `[output] backend` is `pipewire` or `alsa`, is required by `run`, and `output.device` is required when it is `alsa`. Offline commands choose their input and output from `-i` / `-o`; the non-seekable-stdout rules are enforced by `convert -o -`, not by `Settings::validate()`.

**Configuration metadata.** Every key's user-facing prose lives in typed TOML files, `crates/audigma-settings/meta/<section>.toml` (one `[[field]]` per key), loaded by the `meta` feature of `audigma-settings` and served to both configurator frontends. A field carries a summary, description, default, range, structured visibility and requirement conditions (`show_if`, `required_if`, `required_for`; objects, not expression strings, with shared test vectors), per-use-case recommendations, and for enum-like keys `[[field.options]]` entries. Field prose and option prose do not repeat each other: the field text covers what holds for the key as a whole, each option carries its own detail. Assists (`meta/assists.toml`) are declared in data and implemented in the configurator; the `preset` kind offers three named quality starting points (Neutral, Apodizing, Low latency) that fill `resampler.phase`, `resampler.apodization` and `resampler.modulator_order` in the draft. The bundle name is never written to the config. Consistency tests tie the catalog to the generated JSON Schema of `Settings` in both directions.

**Config writer.** `configure` updates an existing file in place with `toml_edit` (user comments, key order and unknown keys are preserved; a timestamped backup is written first, then the file is replaced atomically) and creates a missing one. Comments are generated at one of three levels, `none`, `brief` or `full`. Every generated line starts with `##` and is regenerated on each save; a line starting with a single `#` belongs to the user and is never touched. The level is chosen in the UI only.

Validation errors are attributed to a config key (`SettingsError::Validation { path, message }`; `path` is the absolute dotted key such as `output.device`), and `Settings::warnings()` returns `FieldIssue { path, message }` values, so a UI can show them next to the field. `audigma convert` rejects an explicit `f32` or `f64` `target.sample_format` for a `.flac` output (FLAC carries integer PCM only; an unset format resolves to the default `s32le` and is written as 32 bit). A PCM target with `output.backend = "pipewire"` and an explicit `target.sample_format` warns that the format is ignored.

### 6.1 Declarative Target Resolution & `rate_family`
Target resolution evaluates based on observed input rate/format and resolves into an explicit filter and target configuration. `rate_family = "source"` dynamically pairs 44.1 kHz-family and 48 kHz-family rates:

```toml
schema_version = 1

[target]
format = "dsd"
rate_family = "source"
dsd_rate = "dsd256"

[resampler]
apodization = 19000
```

### 6.1.1 `[runtime]` Threading, Placement & Real-Time Scheduling
Optional; every key has a default and the section may be omitted. It applies to live and offline commands (offline never requests RT).

```toml
[runtime]
dsp_threads = "auto"             # "auto" | "inline" | integer >= 1
realtime = "auto"                # "auto" | "required" | "off"
realtime_priority = 20           # 3-99; loop gets this, pw-* get -1, DSP workers -2
core_selection = "performance"   # "performance" | "efficient" | "os"
smt_siblings = "avoid"           # "avoid" | "never"
# loop_cpu = 4                   # pin audigma-loop (default: housekeeping set)
# dsp_cpus = [0, 2]              # explicit pins, one per worker
systray = "off"                  # "off" | "auto" | "required"; `run` only, fixed at process start
```

- `dsp_threads`: see §3.4. `dsp_cpus` with `"auto"` sets the worker count to its length (capped at the channel count); with an integer its length must match; `dsp_cpus` cannot be combined with `"inline"`.
- `realtime`: `auto` tries RT and continues at normal priority on failure (with a warning); `required` makes any startup RT failure fatal; `off` never requests RT.
- `systray`: `off` shows no tray icon; `auto` shows one and warns and continues without it when no tray host is available; `required` makes that failure fatal at startup. Like the rest of `[runtime]`, it is read once at process start; a change picked up by `resume` only logs a warning.
- `core_selection = "os"` disables automatic pinning and spinning (explicit `dsp_cpus` still applies). `smt_siblings = "never"` fails at startup if workers exceed the allowed physical cores.
- Validated by `config check`: priority range, non-empty and duplicate-free `dsp_cpus`, count/pin agreement, `loop_cpu` not in `dsp_cpus`. Checks that need the machine (CPU exists and is allowed, pins that are SMT siblings) happen at startup and are errors or warnings as described.
- More than two channels is supported by the placement code but untested.

### 6.2 Reference Configuration Fixtures

The fixtures leave `[input] node_name` unset; the daemon names the sink `audigma_sink_<instance>` (§7).

#### A. PipeWire Virtual Sink $\to$ PipeWire High-Rate PCM Output (`pipewire-pcm.toml`)
```toml
schema_version = 1

[input]
backend = "pipewire"
channels = 2
channel_positions = [ "FL", "FR" ]
rate_mode = "dynamic"
allowed_rates = [ 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000 ]

[output]
backend = "pipewire"
target_node = "alsa_output.usb-FiiO_K17-00.analog-stereo"
transport = "pcm"
volume_control = "hardware-software-fallback"
device_wait_ms = -1
device_wait_expiry = "exit"      # "exit" | "pause"
keep_audio_sink_ms = 5000

[target]
format = "pcm"
rate_family = "48000"
pcm_rate_hz = 192000
sample_format = "f32"

[gain]
preamp_db = -3.0
overload_policy = "mute-recover"

[resampler]
phase = "linear"
apodization = "off"

```

#### B. PipeWire Virtual Sink $\to$ Direct Exclusive ALSA Native DSD (`pipewire-alsa-dsd.toml`)
```toml
schema_version = 1

[input]
backend = "pipewire"
channels = 2
channel_positions = [ "FL", "FR" ]
rate_mode = "dynamic"
allowed_rates = [ 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000 ]
allow_dsd = true
buffer_ms = 20                   # sink pull cushion; output.buffer_ms protects the DAC
quantum_frames = 512             # 512 | 1024 frames per sink cycle at 44.1/48 kHz, scaled with rate

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "native"
reserve_device = true
volume_control = "hardware-software-fallback"
device_wait_ms = -1
device_wait_expiry = "exit"      # "exit" | "pause"
keep_audio_sink_ms = 5000
buffer_ms = 20
period_ms = 5
quantum_frames = 1024            # pipewire output only: 512 | 1024 graph quantum at 44.1/48 kHz, scaled with rate

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"

[gain]
preamp_db = 0.0
overload_policy = "mute-recover"
dsd_safety_margin_db = 0.25   # 0 | 0.25 | 0.5

[resampler]
phase = "minimum"
apodization = "off"
modulator_order = 5
drift_correction = "auto"   # auto: correct a free-running input on ALSA output; "on": also observe (estimate only) a demand-driven input
max_clock_drift_ppm = 500
```

#### C. Offline WAV/FLAC $\to$ DSF (`offline-dsf.toml`)
```toml
schema_version = 1

[input]

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"
dither_tpdf_seed = 12345

[gain]
preamp_db = 0.0
peak_analysis = "two-pass"
overload_policy = "error"

[resampler]
phase = "linear"
apodization = "off"
modulator_order = 5

[offline]
tail_policy = "trim-to-duration"
write_report = true
dsf_pad_mode = "spec-zero"
```

#### D. Raw PCM stdin $\to$ Raw DoP stdout (`stdio-dop.toml`)
```toml
schema_version = 1

[input]
sample_format = "s16le"
rate_hz = 44100
channels = 2

[output]
transport = "dop"
dop_sample_format = "s24_3le"

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"

[gain]
overload_policy = "error"

[resampler]
phase = "linear"
apodization = "off"
modulator_order = 5
```

---

## 7. CLI, Configurator & Service Interface

```text
audigma run [-c CONFIG]
audigma stop
audigma play -i INPUT [-d DEVICE] [-v VOLUME] [-O KEY=VAL...] [-c CONFIG] [--json]
audigma convert -i INPUT -o OUTPUT [-c CONFIG]
audigma benchmark [-i INPUT] [-c CONFIG] [--json]
audigma probe [--backend alsa|pipewire] [--json]
audigma configure [tui|web] [-c PATH]
audigma completions SHELL        # bash|zsh|fish|elvish|powershell, script to stdout
audigma man [--out DIR]          # roff manpage to stdout, or one page per command into DIR
audigma config check [-c CONFIG]
audigma config explain [-c CONFIG] [--input-rate HZ] [--json]
audigma status [--watch] [--json]
audigma pause
audigma resume
audigma set volume DB
audigma reload
audigma doctor [--fix] [--json]
```

- **`audigma play`:** Direct file/stream playback to the configured live `[output] backend`: exclusive ALSA (default) or the PipeWire graph (`-O output.backend=pipewire`, PCM only; `transport` is fixed to `pcm`, the stream carries unquantized f32, `-d` is rejected in favour of `output.target_node`, and the tail is drained before exit). Supports immediate ergonomic overrides (`-d device`, `-v volume` in dB or percentage with the same syntax as `audigma set volume`; `-v` is applied as the user-volume component layered on top of `[gain] preamp_db`, not as a preamp replacement) and generic ad-hoc configuration overrides (`-O key=value`, e.g. `-O output.transport=dop -O output.buffer_ms=25`) which take precedence over the specified or default `config.toml`.
- **`audigma probe`:** `--backend alsa` reports cards, devices, formats, rates and DSD/DoP support. `--backend pipewire` lists the registry's `Audio/Sink` nodes (each bound once to read its full properties: `node.name`, description, `device.api`, ALSA card index, name and path), excluding audigma's own sinks. `alsa_card` equals the ALSA report's card index, which joins the two reports. Sink names are session-specific (a `.4`-style suffix can appear or disappear), so a listed name is a current value, not a stable id.
- **`audigma run` & `audigma stop`:** `run` executes the live audio pipeline in the foreground (intended for `systemd --user` service or direct terminal invocation). `audigma stop` connects to the local control socket and requests an orderly shutdown: ramping PCM gain to zero, draining filter group delays, emitting the balanced DSD idle sequence, releasing the D-Bus `ReserveDevice1` device lock so PipeWire/WirePlumber immediately reclaims the DAC, and terminating the daemon.
- **`audigma pause` & `audigma resume`:** `pause` tears the whole pipeline down exactly like `stop` (ramp to zero, drain, DSD idle pattern, close the output, release `ReserveDevice1`, drop the PipeWire sink, join the DSP workers and RT threads) but keeps the process, instance lock and control socket alive in state `Paused`; the request returns once the device is released. `resume` re-reads and re-validates the configuration file, then rebuilds the pipeline through the startup path; if the file is invalid the error is returned and the daemon stays `Paused`. `[runtime]` (threading, placement, RT) is fixed at process start: a change picked up by `resume` logs a warning and applies after a restart. User volume and mute persist across a pause. Both commands are idempotent (`pause` when paused and `resume` when running succeed); `stop` works while paused. While paused the PipeWire virtual sink does not exist, so applications fall back to another sink, as after `stop`. `status` shows only the state, uptime and volume while paused, with the reason in telemetry (`pause_reason`: `Requested` or `DeviceWaitExpired`). With `[output] device_wait_expiry = "pause"`, an expired `device_wait_ms` (including `0` with the device absent at start or at a `resume`) enters `Paused` with reason `DeviceWaitExpired` instead of exiting the process; `resume` itself reports success once the config is accepted, so a missing device surfaces as the daemon returning to `Paused`.
- **System tray (`[runtime] systray`):** with `auto` or `required` the daemon registers a StatusNotifierItem (`ksni`, blocking API, Linux only; id `audigma-<instance>`) in `audigma-cli`'s `tray/` module: `model.rs` is the platform-neutral state -> status line / menu mapping (`TrayModel`, `TrayAction`), `linux.rs` renders it. The menu is a disabled status line (e.g. `k17: Running - 96 kHz -> DSD256 - -12 dB (85%)`, `k17: Paused (device not found)`), Mute or Unmute depending on the mute state, Pause or Resume depending on state, and Stop. Middle-clicking the icon toggles mute. Scrolling the mouse wheel over the icon changes the volume by 2 % per notch (`set_volume_percent`, clamped to 0-100 %, so never above 0 dBFS; ignored when the volume mode is `none`; scrolling while muted unmutes). Scroll deltas accumulate per 120 units (the Qt/KDE notch) so touchpads and high-resolution wheels step once per notch of travel, and a direction change drops the remainder; horizontal scrolls are ignored. Hosts that report ±1 per tick instead of 120 would need many ticks per step. Menu actions run on a worker thread through the same `pause_and_wait` / `resume_and_wait` / `stop` calls as the IPC server. A refresh thread polls telemetry and pushes a change at most once per second. The tray is owned by `run_run` and outlives pipeline sessions, so it stays up while paused. `auto` registers even when no tray host is running yet (the icon appears when one starts) and only warns if the session bus is unreachable; `required` fails startup when registration fails or no tray host exists.
- **`audigma configure` (`audigma-configurator`):** Creates or edits a configuration without hand-editing TOML, exposing every key with its commentary. `audigma configure [tui|web] [-c PATH]` (`--out` is an alias of `-c`; the path follows §6, so `$AUDIGMA_CONFIG` applies to both read and write). The terminal UI is the default; `web` serves a one-shot localhost page and JSON API (`127.0.0.1:<random port>` by default, `--bind ADDR[:PORT]`, a one-time token required on every request and handed over as an `HttpOnly; SameSite=Strict` cookie, a same-origin check on writes, `--idle-timeout`, `--no-open`) and exits after a save or an explicit quit.
  - *Start:* an existing file opens the form directly. A path that does not exist offers *Guided setup* or *Edit configuration*. Guided setup asks a use case (interactive desktop, headless appliance, offline conversion), the output path and device or sink (from a fresh ALSA or PipeWire probe), the format, a quality starting point, the rate policy, optionally a benchmark sweep of the rate tiers ("discover compute capability"), the rate tier and the volume policy, then lands on the form's review page with every value editable. The offline use case asks only the minimum and drops into the form at `[offline]`.
  - *Form:* all sections in canonical order (the advanced `offline`, `runtime`, `logging` folded), each field with its description, default, the recommendation for the chosen use case (highlight only; nothing changes silently) and live validation attributed to the key. The review page can run "validate on this machine", a benchmark of exactly the configured target (an estimate: the offline gate is necessary but not sufficient for live playback).
  - *Save:* the writer of §6 (update in place, backup, `none | brief | full` comments, optional only-keys-that-differ-from-defaults), then the `audigma play -i <file> -c <path>` and `audigma run -c <path>` commands are printed. No systemd or WirePlumber snippets are written.
  - *Quality starting points* are a preset assist, not a setting (§6).
- **`audigma doctor [--fix]` (`audigma-doctor`):** Inspects the host OS audio stack and diagnoses runtime issues:
  1. *ALSA Device Lockouts:* Inspects `/dev/snd/pcmC*D*p` file descriptor holders. If held by WirePlumber/PipeWire, `--fix` can dynamically invoke `wpctl set-profile <id> 0` or offer to write a permanent WirePlumber drop-in rule in `~/.config/wireplumber/`. If held by a foreign user or daemon (such as MPD), `--fix` interactively prompts the user before terminating the process, escalating to `sudo` if permissions are insufficient, or printing the exact command required.
  2. *PipeWire Multi-Rate Configuration:* Checks `default.clock.allowed-rates` in PipeWire when `rate_family = "source"` is active on a PipeWire virtual sink, warning and offering to install the required multi-rate configuration drop-in if the graph is pinned to a single rate (e.g. `48000`).
  3. *Real-Time Scheduling Limits:* Verifies `RLIMIT_RTPRIO` / RTKit availability for the real-time loop, PipeWire and DSP threads (the daemon itself reports the outcome per thread as `rt_priority` events at startup).
- **Control Socket & Telemetry Protocol (`audigma status`):** Operates over a local Unix domain socket at `$XDG_RUNTIME_DIR/audigma/<instance>.sock` (fallback `/tmp/audigma-<uid>/`, mode 0700), guarded by an exclusive `flock` on `<instance>.lock` so a second daemon with the same instance name fails fast (`-i/--instance` or `AUDIGMA_INSTANCE`, default `default`, names `[A-Za-z0-9_-]{1,32}`). Bare `status` lists all live instances; bare `stop`/`set` act on the sole live instance and error when several are running. When `[input].node_name` is unset or empty the PipeWire sink is named `audigma_sink_<instance>`, and the PipeWire output stream is `audigma_pcm_out_<instance>`. Runtime telemetry exported via `status [--watch] [--json]` exposes:
  - Input negotiated format/rate, PipeWire graph rate, and active channel positions.
  - Output bit/sample/transport rate and active physical device/endpoint.
  - Daemon and device state (`Running`, `WaitingForDevice`, etc.) and `disconnect_duration_ms`.
  - Active volume mode (`hardware`, `software`, `none`) and current volume attenuation levels.
  - Active filter (phase and apodization), profile hash, and applied gain breakdown (`preamp_db`, user volume, profile headroom).
  - Measured true peaks and modulator integrator operating envelope.
  - Latency breakdown (graph + queue + filter group delay + backend buffer).
  - SPSC queue fill percentage and ASRC mode, applied correction and estimated clock drift ($\delta_{\text{ppm}}$).
  - Hardware XRun counters, fault/recovery event counts, and per-worker execution and dispatch timing (mean, p50, p99, max, cumulative p99.9) with the stall cause.

---

## 8. Spectral Verification & Quality Targets (`tools/measure/`)

Every filter setting and modulator order in `audigma-dsp` (`tables.rs`) is verified via automated tests in `tools/measure/`:

1. **Coherent FFT Protocol (`tools/design/dsm_analysis.py`):**
   - Map 1-bit DSD output to `{-1.0, +1.0}`.
   - Use coherent test frequencies ($f_{\text{tone}} = k \cdot f_{\text{bit}} / N$, $N = 1,048,576$) and one full-record periodic Nuttall window (4-term, continuous first derivative). A noise-shaped record is not periodic, so a rectangular window is invalid, and the window's far-sidelobe decay (not its first sidelobe level) sets the floor: 4-term Blackman-Harris (6 dB/octave) floors at ~145-150 dB. Measure in-band SNR (20 Hz–20 kHz), SINAD/THD+N, THD, worst non-harmonic spur, and ultrasonic noise power (20–100 kHz).
   - **Analysis floor:** for every modulator, analyse a synthetic non-periodic record with the same NTF-shaped out-of-band noise and a known in-band level; results within 10 dB of that floor are reported as lower bounds only.
   - **Bit-exact reference:** the direct-modulator record must equal, bit for bit, a Python run of the designed CRFB loop (`simulate_crfb`, same operation order as `modulator.rs`, §4.3) on the same input.
2. **Linear-Path Measurements (`crates/audigma-measurements`, harness modes `cascade-impulse`, `cascade-tone`, `src-tone`, `src-tables`):**
   - The harness drives the production types (`HalfBandCascade`, `PolyphaseResampler`, the exported `tables`); it never re-implements DSP. Constants the analysis needs (tap and phase counts, resampling ratio) are read back from `audigma-dsp` (`SRC_TAPS_PER_PHASE`, `SRC_RATIONAL_PHASES`, `SRC_ASRC_PHASES`, `PolyphaseResampler::{input_per_output_ratio, table_phases, taps_per_phase}`), so the analysis cannot drift from the implementation.
   - **Cascade:** the impulse response of the implemented cascade (every phase x apodization, K in {1, 2, 3, 4, 6, 9}) gives passband ripple, droop, image rejection in the bands $k \cdot f_s \pm 20	ext{ kHz}$ and group delay; it must equal, sample by sample, the cascade rebuilt from the `design_halfband.py` taps of every stage, stage 0 must reproduce the design's ripple and rejection, and coherent tones must agree with the impulse-response prediction.
   - **Resampler:** coherent tones through `new_rational` (every 44.1k/48k matched tier, both directions) and `new_asrc` (several drifts and ratios) plus multi-tier downsampling (96k → 44.1k, 88.2k → 48k) and stopband tones (alias levels) give gain, worst spur and residual; every bank a resampler builds must equal `design_src.py` to 1e-14.
   - Quality figures are gated by the requirements of item 6 (image rejection and passband deviation for the cascade, in-band error for the resampler); the method checks (design match, tone vs impulse, tables vs generator) also fail the run.
   - Reports: one file per family in `docs/measurements/` (`modulator.md`, `cascade.md`, `resampler.md`, `signal.md`) and an index `README.md`. Spectral verification is fully deterministic and machine-independent.
3. **Signal-Dependent Measurements of the Production Chain (harness mode `chain`, report `signal.md`):**
   - The chain as `audigma run` builds it with the default settings: baseband PCM -> profile gain $L - M$ -> `DcBlocker` (1 Hz, settled for 30 time constants before the record; `--dc-blocker off` removes it) -> `HalfBandCascade` (linear, apodization `off`) -> `Modulator` with TPDF dither (0.005), every 44.1k/48k DSD rate, both orders. Levels are relative to PCM full scale (0 dBFS reaches the modulator at $L - M$).
   - Stimuli: 1 kHz level sweep (0 dBFS at $M = 0$ and $M = 0.25$ down to -120 dBFS), digital silence, DC offsets (-40/-80 dBFS), the low-level set without dither, two-tone IMD (SMPTE 60 Hz + 7 kHz 4:1, CCIF 19 + 20 kHz 1:1) at 0 dBFS peak, and the DC offsets again with the DC blocker off (comparison).
   - Reported: SNR, SINAD, THD, IMD, unweighted and A-weighted in-band noise, AES17-style dynamic range (A-weighted THD+N at -60 dBFS), worst spur as *prominence* over the local noise median (pure noise gives ~8 dB), ultrasonic power 20-50/50-100/20-100 kHz, 48k minus 44.1k family deltas. The quality tiers of item 6 are checked here.
   - Checks: no overload at any stimulus (including a sine peak exactly at $L$), the DC blocker leaves at least 60 dB less DC than the offset, and tone levels up to 10 kHz within 0.01 dB of the requested PCM level (plus the noise-limited reading error at the lowest levels).
4. **Stability & Envelope Tests:**
   - Verify bounded integrator states across DC offsets, $-6\text{ dBFS}$ coherent tones, high-crest-factor multitone bursts, digital silence, and forced overload recovery.
5. **Real-Time Throughput & Deadline Budget (`tools/benchmark/`):**
   - **Offline gate (`crates/audigma-measurements` harness modes `benchmark` and `benchmark-pipeline` via `tools/benchmark/benchmark.py`, or `audigma benchmark`):** for stereo on one core, sustained average DSP throughput $\ge 2\times$ real-time and **p99.9 worker execution time $\le 70\%$** of the audio-block duration, with zero heap allocations inside `process()`. Stereo costs twice the mono time, so the single-channel benchmarks apply $\ge 4\times$ and $\le 35\%$. `benchmark` times the half-band cascade and modulator only; `benchmark-pipeline` times the worker input stage (DSD decode, SRC, cascade, modulator) inline on one core for PCM 44.1 kHz / PCM 96 kHz / DSD64 input to PCM 384 kHz / DSD256 / DSD512 output, at the lightest and heaviest filter and modulator configuration. Worker execution time is the time a worker spends processing its tile, excluding wake-up and barrier waits. Benchmark outputs and reference baselines are recorded in `docs/benchmarks/` (`benchmark.reference.md`, `benchmark.reference.json`).
   - **Live health (`audigma run`):** the live daemon additionally pays for input DSP, worker wake-up and barrier synchronisation, so its per-tile dispatch time is reported separately and is not held to the 70% gate. Live criteria: zero xruns, starvations and dropped chunks; tiles whose dispatch time exceeds the tile budget are tolerated only while the device buffer absorbs them, and are counted and logged (`late_slice` events).

6. **Quality Requirements (DSP quality audit, step 5):** thresholds are derived from two anchors, not from hardware (the DACs' own noise is not a reference). Levels are in-band (20 Hz–20 kHz) dBFS relative to a full-scale sine at the PCM input.
   - **Anchor 1, audibility:** assuming 0 dBFS reproduces at ~120 dB SPL peak, noise and every discrete product must stay below the threshold of hearing (~0 dB SPL): DR (A) (AES17, A-weighted THD+N at -60 dBFS) $\ge 120\text{ dB}$, every in-band spur $\le -120\text{ dBFS}$.
   - **Anchor 2, 24-bit transparency:** the DSP must not be the bottleneck for 24-bit sources. A TPDF-dithered 24-bit quantizer has $-141.5\text{ dBFS}$ in-band noise, so in-band noise, harmonic and IMD levels and every spur $\le -141\text{ dBFS}$ at every stimulus.
   - **Production chain tiers (`signal.md`):** tier A = DSD64 (both families), anchor 1; tier B = DSD128 and above, anchors 1 and 2. Chain flatness for tones above 10 kHz: within 0.02 dB (A) / 0.01 dB (B) of the requested level (the modulator's signal transfer function lifts 19–20 kHz by +0.011 dB (5th) / +0.015 dB (7th) at DSD64); tones up to 10 kHz within 0.01 dB (method check). **Documented exception:** DSD64 (44.1 kHz family) with the 5th-order modulator, the default order, reaches DR (A) ≈ 118 dB, about 1.6 dB short of anchor 1; it meets the spur requirement, and the 48 kHz family (≈ 122 dB) and the 7th order (≈ 137 dB) pass. The default is not changed. Anchor 2 is out of reach at DSD64 for either order (7th order in-band noise ≈ -135 dBFS).
   - **Linear stages:** these run ahead of every target, so they are held to the stricter anchor where their errors land in band. Resampler: everything unwanted in 20 Hz–20 kHz (images, aliases, interpolation error; for an input above the output Nyquist also its alias when it lands in band) $\le -141\text{ dB}$ re the input tone, and passband deviation $\le 0.01\text{ dB}$. The margin is small and accepted: the worst case, a full-scale 20 kHz tone through a near-unity ratio (ASRC, 44.1k → 48k), puts its image at the stopband edge, where it folds back to ~20 kHz at -141.4 dB (0.4 dB margin, from the slack of the 140 dB design goal); realistic content has far less energy at 20 kHz (at -6 dBFS the error is -147 dBFS). Interpolation cascade: passband deviation $\le 0.01\text{ dB}$ with apodization `off` (apodizing settings roll off by design), and image rejection $\ge 120\text{ dB}$: its images lie above 24.1 kHz and never in band, so they are held to anchor 1 as if they were audible. The 140 dB filter target of §4.2 is a design goal above that requirement.
   - **Units:** spurs are gated in absolute dBFS (a spur's audibility does not depend on the test tone, and silence and DC offsets have no tone to refer to); THD and IMD stay ratios (dBc). Values within 10 dB of the analysis floor count as that bound.
   - **Not gated:** ultrasonic noise (no DAC or amplifier limit is assumed), modulator-only and cascade + modulator records without gain staging or dither (`modulator.md`, `cascade.md`: stability and bit-exactness only). The TPDF dither (0.005) has no measurable effect on noise or spurs but is kept as harmless.

---

## 9. Technical References

1. [PipeWire SPA DSD structure and rate/interleave definitions](https://docs.pipewire.org/dsd_8h_source.html).
2. [PipeWire node, adapter, resampling, and ALSA properties](https://docs.pipewire.org/page_man_pipewire-props_7.html).
3. [DoP open standard v1.1](https://dsd-guide.com/dop-open-standard).
4. [Sony DSF specification v1.01](https://dsd-guide.com/sites/default/files/white-papers/DSFFileFormatSpec_E.pdf).
5. [Schreier/Caldwell NTF synthesis & delta-sigma design lectures](https://www.individual.utoronto.ca/schreier/lectures/5-6.pdf).
