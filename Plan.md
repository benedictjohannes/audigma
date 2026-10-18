# audigma — Multi-Session Execution Plan

> **Authoritative Reference:** This execution plan governs how we build `audigma` step-by-step across sessions. It works alongside `Design.md`, which serves as the canonical technical specification for architectural decisions, DSP math, bit-packing semantics, transport invariants, and configuration contracts.

---

## 1. Execution Rules & The "STOP" Protocol

### Mandatory Execution Rules
1. **No Deviations:** Never write code that deviates from this plan or violates the invariants in `Design.md` (e.g., never apply PCM gain/fades to packed DSD/DoP, never send `0x00` bytes as audible DSD silence, never place variable-ratio ASRC at MHz bit rates).
2. **Proactive Updates:** If an architectural flaw, crate limitation, or hardware quirk is discovered during a session, **STOP immediately** and discuss it with the user along with a concrete proposal to update `Plan.md` and/or `Design.md`.
3. **History Preservation:** When a phase is completed, mark its header as `[Completed]` (e.g., `### Phase 0: ... [Completed]`). Do not delete completed phases.
4. **Proof is Mandatory:** Every phase must end with reproducible proof (`cargo test`, generated spectral report, or `[User-Led]` hardware confirmation) satisfying all Success Criteria before the phase can be marked `[Completed]`.
5. **Hardware Collaboration Rule:**
   - Known reference hardware profiles discovered for this workspace:
     - **FiiO K17** (`USB ID 2972:0107`, ALSA Card `K17`): Native DSD (`DSD_U32_BE`, `DOP=0, bitrev=0`) up to DSD512 (705.6k / 768k frame rates) and PCM (`S16_LE`, `S32_LE`) up to 768 kHz.
     - **JadeAudio JA11** (`USB ID 2972:0102`, ALSA Card `JA11`): Pure PCM endpoints (`S16_LE`, `S24_3LE`, `S32_LE`) up to 384 kHz (including 176.4 kHz and 352.8 kHz for **DoP64** and **DoP128**).
   - **Never assume both (or either) DACs are currently connected or idle** (e.g., `K17` may be actively in use by MPD, or `JA11` may be unplugged).
   - Whenever a task touches ALSA, PipeWire, or hardware probing/playback, the agent **MUST** check/discuss with the user first and rely on `[User-Led]` collaboration for playback and physical state changes.
6. **Rust Pedagogical Alignment:** Keep code idiomatic, modular, and well-commented so each phase clarifies Rust ownership, traits, zero-allocation real-time boundaries, and DSP data structures without overwhelming blast radius.
7. **Handoff Brief Maintenance:** At the end of every execution session, update the `# Handoff Brief` section at the bottom of `Plan.md`.

### The "STOP" Protocol
- Each execution session works on **strictly one active phase** (or an explicitly agreed sub-slice if a phase is paused for discussion).
- Once the current phase's Success Criteria are proven and the `# Handoff Brief` is updated, the agent **MUST STOP**.
- The next phase **MUST** be initiated in a fresh session to prevent context pollution.

---

## 2. Objective & Target Architecture

Build `audigma` from a clean Cargo workspace into a verified offline converter and real-time Linux PipeWire/ALSA DSD and high-rate PCM audio engine, following the 8-crate layered architecture in `Design.md §2`:

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
  design/                 # Offline scripts to synthesize/export FIR & Schreier NTF coefficient tables
  measure/                # Spectral analysis (FFT coherence, Welch averaging, SNR/SINAD verification)
docs/
  measurements/           # Generated SNR/SINAD/stability verification reports
  hardware/               # Probed DAC capability records (FiiO K17, JadeAudio JA11)
examples/                 # Ready-to-use TOML configs, MPD snippets, and WirePlumber rules
```

---

## 3. Phased Breakdown

### Phase 0: Workspace Scaffolding, Domain Types, Bit-Packing & Schema v1 [Completed]
**Maps to:** `Design.md §1`, `§2`, `§5.1–§5.3`, and `§6`.

- **Goal:** Initialize the Cargo workspace (`audigma-dsp`, `audigma-format`, `audigma-settings`), implement strongly typed domain units (`PcmRateHz`, `DsdBitRateHz`, `TransportFrameRateHz`, plus explicit `to_spa_dsd_byte_rate()` $= f_{\text{bit}} / 8$ conversion and $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ calculation), define `DspConfiguration` and `presets.rs` (`PRESET_BALANCED`, `PRESET_REFERENCE`, `PRESET_LOW_LATENCY`) in `audigma-dsp`, build verified bit/byte packers (`audigma-format`) for `DSD_U32_BE`, DSF blocks, and the three ALSA DoP containers (`s32le`, `s24le`, `s24_3le`), and implement `Schema v1` config parsing (`Settings`) in `audigma-settings`.
- **Scope & Tasks:**
  1. Initialize Cargo workspace with the initial three crates: `audigma-dsp`, `audigma-format`, and `audigma-settings`.
  2. In `audigma-dsp`, define strongly typed rate/format units (`PcmRateHz`, `DsdBitRateHz`, `TransportFrameRateHz`), SPA DSD byte-rate conversion ($f_{\text{bit}} / 8$), baseband $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ helper (`282.24` for DSD256 at $B = 20\text{ kHz}$), `DspConfiguration`, `presets.rs` (`PRESET_BALANCED`, `PRESET_REFERENCE`, `PRESET_LOW_LATENCY`), and the `StreamSpec` contract (`Design.md §1`, `§3.4`, `§4.1`, `§5.1`).
  3. In `audigma-format`, implement exact bit-packing and unpacking with independent golden vector unit tests for:
     - ALSA Native `DSD_U32_BE` (`f_frame = f_bit / 32`, MSB-first byte/bit order matching K17 `bitrev=0`).
     - Stereo DoP (`0x05`/`0xFA` marker alternation) across all three ALSA containers: `s32le` (MSB-justified `<< 8`), `s24le` (LSB-justified in 32-bit), and `s24_3le` (packed 3-byte) (`Design.md §5.2`).
     - Sony DSF 4096-byte per-channel blocks (`bits_per_sample = 1` LSB-first default per Sony DSF v1.01, plus MSB-first option) with both `dsf_pad_mode = "spec-zero"` and `"dsd-silence"` (`0x69` idle fill) (`Design.md §5.2`).
  4. In `audigma-settings`, implement `Schema v1` TOML parsing (`Settings`), preset resolution (mapping `"reference"` $\to$ `&audigma_dsp::presets::PRESET_REFERENCE`), and validation against the reference config fixtures in `Design.md §6.1–§6.2`.
  5. Record discovered hardware profiles for `FiiO K17` and `JadeAudio JA11` in `docs/hardware/`.
- **`[User-Led]` Actions:**
  - Review the Cargo workspace layout and run `cargo test` in IDE/terminal if desired.
- **Success Criteria & Proof:**
  - `cargo test --workspace` passes 100% of unit tests, including rate/OSR/SPA conversion tests, golden vector tests for `DSD_U32_BE`, all three DoP alignments (`s32le`, `s24le`, `s24_3le`), DSF block padding, and all TOML config fixtures from `Design.md §6`.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 1: Reference DSP Harness, Filter Tables & 5th-Order DSM Loop [Completed]
**Maps to:** `Design.md §4.1–§4.5` and `§8`.

- **Goal:** Build the reproducible filter/NTF coefficient generator (`tools/design/`), embed verified coefficients in `audigma-dsp` (`presets.rs`), implement the pure-Rust `f64` multistage interpolation cascade and 5th-order CRFB Delta-Sigma Modulator in `audigma-dsp`, and verify spectral performance with an offline measurement harness (`tools/measure/`).
- **Scope & Tasks:**
  1. Create Python (`scipy`/`numpy`) synthesis/verification tooling in `tools/design/` using the exact baseband oversampling ratio $\text{OSR} = f_{\text{bit}} / (2 \cdot B)$ (`282.24` for DSD256) as an independent cross-language mathematical oracle to export static `f64` coefficients, maximum stable input limit $L\text{ dBFS}$, and baseline headroom metadata into `audigma-dsp` (`presets.rs`):
     - Half-band $2\times$ FIR cascade coefficients (`balanced` ≥ 120 dB rejection, `reference` ≥ 140 dB rejection) and polyphase rational SRC coefficients (`Design.md §4.2`).
     - 5th-order CRFB modulator coefficients for DSD64, DSD128, and DSD256 (`Design.md §4.3`).
  2. In `audigma-dsp`, implement:
     - Per-thread hardware FTZ/DAZ helper (`x86_64` MXCSR / `aarch64` FPCR) (`Design.md §4.1`).
     - Staged $2\times$ FIR interpolator and fractional/rational polyphase SRC (`Design.md §4.2`).
     - 5th-order CRFB modulator (`f64`), $\pm 1$ quantizer, TPDF dither injection, headroom equation helper ($\text{gain\_db} \le L - M - P$), overload detector, verified zero-input DSD idle stream, and emergency modulator-boundary fault/recovery (`Design.md §4.3–§4.5`).
  3. Build `tools/measure/` implementing the coherent FFT and Welch spectral protocol (`Design.md §8`) to verify in-band SNR, SINAD, ultrasonic noise bands, and stability envelope across DC, $-6\text{ dBFS}$ coherent 1 kHz sine (`f_tone = k * f_bit / N`), silence, and overload recovery.
- **`[User-Led]` Actions:**
  - Inspect generated spectral verification reports in `docs/measurements/` and confirm the trade-offs (headroom, SNR, CPU throughput) make sense.
- **Success Criteria & Proof:**
  - `cargo test --workspace` and the spectral verification tool produce a report in `docs/measurements/` proving stable modulation (no state explosion or clipping within envelope) and meeting the `Design.md §4.2` / `§8` targets for DSD64/128/256.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 2: Offline Audio Converter (`audigma convert` & `benchmark`) [Completed]
**Maps to:** `Design.md §3.1`, `§4.4`, `§5.2–§5.3`, `§6.2.D–E`, and `§7`.

- **Goal:** Deliver the first playable vertical slice: introduce `audigma-engine` (pure Rust pipeline & I/O traits), `audigma-io` (file & stdio stream drivers), and `audigma-cli` to convert WAV/FLAC files (and raw stdio streams) into `.dsf`, high-rate PCM `.wav`, lossless `.flac`, or raw DoP, complete with two-pass true-peak analysis, channel topology adaptation, non-seekable `stdout` validation, and deterministic seeding.
- **Scope & Three-Step Breakdown:**
  - **Step 1: Container Decoders/Encoders & Schema Updates (`audigma-format`, `audigma-settings`):**
    - In `audigma-settings`, add `OutputContainer::Flac` (`container = "flac"`) and define offline configuration resolution rules (auto-inferring target format and container from output extension `.dsf`, `.wav`, or `.flac`, with fallback to built-in offline reference defaults).
    - In `audigma-format`, implement pure-Rust audio decoding for WAV and FLAC via `symphonia` with input channel topology handling:
      - Mono (1 channel) $\to$ duplicate to stereo ($L = C, R = C$) with unity gain.
      - Stereo (2 channels) $\to$ passthrough ($L = L, R = R$).
      - Multi-channel ($> 2$ channels) $\to$ ITU-R BS.775 stereo fold-down downmixing in floating-point PCM before any gain or resampling stages.
    - In `audigma-format`, implement container writers:
      - High-rate PCM `.wav` writer via `hound` (supporting 16, 24, 32-bit int and 32-bit float up to 768 kHz).
      - Lossless `.flac` encoder via `flacenc` (pure Rust, supporting 16 and 24-bit PCM up to 384 kHz).
      - Sony `.dsf` file writer utilizing `audigma-format::dsf` with seekable chunk header size and sample count finalization and selectable `DsfPadMode` (`spec-zero` vs. `dsd-silence`).
    - Add roundtrip golden-vector unit tests for all container formats.
  - **Step 2: Pure-Rust Engine & Prepared Offline Pipeline (`audigma-engine`):**
    - Define abstract `AudioInput` and `AudioOutput` stream traits (pure Rust, zero C-FFI / zero OS audio library dependencies).
    - Implement the offline pipeline runner chaining:
      - PCM input decoding and channel conditioning.
      - Gain staging (preamp, profile headroom, user volume).
      - Multistage linear/minimum-phase FIR interpolation cascade ($2^K$) and polyphase rational SRC (if rate family conversion is required).
      - `peak_analysis = "two-pass"`: Pass 1 evaluates only the LTI PCM interpolation cascade to measure true inter-sample peak $P\text{ dBFS}$ and solves $\text{gain\_db} = \min(\text{configured\_gain\_db},\; L - M - P)$ per `Design.md §4.4`; Pass 2 executes the complete pipeline with calibrated gain.
      - Output quantizer/modulator: 5th-order CRFB DSM for DSD or TPDF dither for integer PCM.
      - Transport bit packing and container egress.
    - Implement tail policies (`tail_policy = "trim-to-duration" | "preserve-tail"`).
    - Comprehensive in-memory pipeline tests verifying bit-exactness, gain scaling, and tail truncation.
  - **Step 3: Concrete File/Stdio I/O, CLI Commands & Quality Proof (`audigma-io`, `audigma-cli`):**
    - In `audigma-io`, implement concrete `AudioInput`/`AudioOutput` drivers for files and stdio streams:
      - Strictly reject non-seekable `stdout` combinations (`container = "dsf"` or streaming `wav` to `-`) before processing (`Design.md §5.3`).
      - Handle pipe backpressure, partial frames, EOF, and `EPIPE` without panicking.
    - In `audigma-cli`, implement subcommands:
      - `audigma convert -i INPUT -o OUTPUT [-c CONFIG]` (with output extension container inference and offline defaults).
      - `audigma benchmark [-i INPUT] [-c CONFIG] [--json]` (using a deterministic 10-second synthetic stereo test signal when `-i` is omitted, reporting DSP-only vs. end-to-end throughput and p99.9 block time budget).
      - `audigma presets list` & `audigma presets show NAME`.
      - `audigma config check` & `audigma config explain`.
- **`[User-Led]` Actions (Hardware Collaboration Required):**
  - Convert a real music track (e.g. WAV/FLAC) to `.dsf` (DSD128 or DSD256), `.wav`, or `.flac` using `audigma convert`.
  - Only when directed by the user, test playback of generated `.dsf` on the **FiiO K17** and/or raw DoP conversion on the **JadeAudio JA11**, confirming clean playback, zero DC pops, and proper DAC indicator status.
- **Success Criteria & Proof:**
  - Independent validation of generated `.dsf`, `.wav`, and `.flac` files via `ffprobe` and unit/integration tests (including non-seekable stdout rejection tests).
  - `audigma benchmark` reports $\ge 2\times$ real-time DSP throughput and p99.9 worker execution time $\le 70\%$ of audio-block duration on the host CPU.
  - `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` pass 100% cleanly.
  - `[User-Led]` listening test confirms clean DSD playback on whichever hardware DAC is confirmed available.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 3: Direct ALSA Hardware Egress (Native DSD & DoP Probe/Playback) [Completed]
**Maps to:** `Design.md §1`, `§4.5`, `§5.1–§5.2`, and `§7`.

- **Goal:** Implement hardware device discovery (`audigma probe`) and the direct ALSA backend in `audigma-io` supporting both Native DSD (`DSD_U32_BE` for **FiiO K17**) and verified DoP (`S24_3LE` / `S32_LE` at 176.4k/352.8k for **JadeAudio JA11** or K17), coupled with clean silence/transition state machines in `audigma-engine` (`Design.md §4.5`).
- **Scope & Tasks:**
  1. In `audigma-io`, implement ALSA device enumeration and capability probing (`feature = "alsa"`).
  2. Implement `audigma probe [--backend alsa] [--json]` (reporting exact format/rate combinations supported by connected DACs).
  3. Implement the ALSA output driver in `audigma-io` and the transition lifecycle in `audigma-engine` (`Stopped → Preparing → Priming → Running → Draining/Muting → Reconfiguring`, `Design.md §4.5`):
     - Pre-SRC PCM fade + `filter_group_delay` drain before sending the verified DSD idle stream during stop/rate transitions.
     - Preserve DoP marker alternation (`0x05`/`0xFA`) continuously during idle and active frames.
  4. Add a direct file-to-ALSA playback mode (`audigma play`) to verify sustained hardware streaming without PipeWire in the middle yet.
- **`[User-Led]` Actions (Hardware Collaboration Required):**
  - **Before any hardware test:** Agent checks with the user which DAC (`FiiO K17`, `JadeAudio JA11`, or neither) is currently plugged in and available for exclusive ALSA access.
  - Run `audigma probe` and `User-Led` short hardware playback test on the available DAC(s) to verify click-free start/stop transitions and zero XRuns.
- **Success Criteria & Proof:**
  - `audigma probe` accurately identifies the connected DAC's exact ALSA formats/rates (verified on host with 5 detected cards including K17 and JA11).
  - `[User-Led]` hardware playback test confirms stable Native `DSD_U32_BE` (on `K17` up to DSD256/DSD512) and verified DoP (on `JA11` across both `s32le` and `s24_3le` at DoP64 / DoP128) with zero XRuns and clean start/stop transitions.
  - Recorded findings in `docs/hardware/`.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 4: PipeWire Virtual Sink, Shared-Graph PCM, D-Bus Device Reservation & Adaptive Clock Bridge [Completed]
**Maps to:** `Design.md §1`, `§3.1–§3.4`, `§6.2.A–C`, and `§7`.

- **Goal:** Build the real-time PipeWire integration in `audigma-io` and clock-bridging in `audigma-engine` so applications (browsers, YouTube Music, MPD) can route into `audigma_sink` and play out continuously either as **PipeWire High-Rate PCM** (`backend = "pipewire"`, `clock.mode = "shared-graph"`) or **Direct Exclusive ALSA DSD / High-Rate PCM** (`backend = "alsa"`, `clock.mode = "adaptive-input"`), acquiring exclusive ALSA access seamlessly via standard D-Bus device reservation (`org.freedesktop.ReserveDevice1`).
- **Scope & Tasks:**
  1. In `audigma-engine`, implement the real-time lock-free SPSC queues (`rtrb`), preallocated scratch buffers, monotonic **generation tags** (`u64`, discarding stale-format buffers across transitions), **RTKit / `RLIMIT_RTPRIO`** real-time thread scheduling (with observable fallback), and both clocking modes (`Design.md §3.4`):
     - `clock.mode = "shared-graph"` for `PipeWire in → PipeWire PCM out` (single clock domain, no ASRC drift loop).
     - `clock.mode = "adaptive-input"` for `PipeWire/ALSA in → Direct ALSA out` (adjusting `R_asrc(t)` in the low-rate PCM stage before the $2\times$ half-band cascade per `Design.md §1`, `§3.4`).
  2. Implement D-Bus device reservation (`org.freedesktop.ReserveDevice1`) in `audigma-io` when `reserve_device = true` (default), dynamically requesting WirePlumber to yield the ALSA DAC when `audigma` runs, and releasing it when `audigma` stops.
  3. In `audigma-io`, implement the PipeWire input backend (`audigma_sink` virtual sink node) and PipeWire PCM output backend (`target_node`) (`feature = "pipewire"`).
  4. Implement `audigma run [-c CONFIG]`, `audigma stop`, `audigma status [--watch] [--json]`, and the local Unix control socket in `audigma-cli` (`$XDG_RUNTIME_DIR/audigma/control.sock`) for live volume (`audigma set volume DB`), preset switching, status telemetry, and orderly shutdown.
- **`[User-Led]` Actions (Hardware Collaboration Required):**
  - Coordinate with the agent to test `audigma run` in a desktop session across both live output paths (`PipeWire in → PipeWire PCM out` and `PipeWire in → Direct ALSA DSD/PCM out`), routing MPD and browser audio through `audigma_sink`.
  - Verify smooth volume control, dynamic D-Bus device reservation yielding the DAC from WirePlumber, zero device-lockout when switching between MPD and browser, clean shutdown via `audigma stop`, and bounded queue occupancy / drift ppm in `audigma status --watch`.
- **Success Criteria & Proof:**
  - Simulated clock-drift integration tests ($\pm 200\text{ ppm}$ drift) in `audigma-engine` (running purely with mock streams and zero C-FFI) pass with bounded queue occupancy, valid generation-tag filtering, p99.9 worker time $\le 70\%$ of block duration, and zero allocation in the audio callback.
  - `[User-Led]` live session confirms seamless playback from both MPD and browser through `audigma_sink` to PipeWire PCM and hardware ALSA DSD/PCM outputs, with D-Bus device reservation yielding and restoring the DAC cleanly:
    - **FiiO K17:** Verified Native DSD (`DSD_U32_BE`) live playback from PipeWire at DSD256 with zero XRuns and click-free start/stop.
    - **JadeAudio JA11:** Verified DoP (`S24_3LE`) live playback from PipeWire at DoP64 with zero XRuns and click-free start/stop.
    - **Software Volume:** Verified pre-modulator digital volume attenuation (-6 dB, -10 dB) via Unix control socket preserving DSD bitstream/marker integrity.
  - `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo fmt --check` pass 100% cleanly.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 5: Interactive Configurator (`audigma-configurator`) & WirePlumber Templates
**Maps to:** `Design.md §2`, `§6.1`, and `§7`.

- **Goal:** Implement `audigma-configurator` to provide an interactive TUI (`audigma configure --tui`) and optional one-shot localhost Web wizard (`audigma configure --web`), complete rule-based selection (`[[rules]]`), and ship the `systemd --user` service and WirePlumber templates.
- **Scope & Tasks:**
  1. In `audigma-configurator`, implement `audigma configure --tui` (and optional `--web` on `127.0.0.1:<random_port>` with session token that terminates on save, per `Design.md §7`):
      - Guided flow: select Output Path (`PipeWire High-Rate PCM` vs. `Exclusive ALSA PCM` vs. `Exclusive ALSA DSD`) $\to$ probe DAC capabilities (`audigma-io`) $\to$ choose Rate Policy (`fixed` zero-relock vs. `source-family`) $\to$ run quick CPU benchmark (`audigma-engine`) $\to$ preview headroom/latency (`audigma-settings` / `audigma-dsp`) $\to$ write `~/.config/audigma/config.toml`.
  2. Complete full `[[rules]]` evaluation and `audigma config explain` output (`Design.md §6.1`).
  3. Package the `systemd --user` unit file, WirePlumber templates, shell completions, and manpage (`examples/`).
- **`[User-Led]` Actions:**
  - Run `audigma configure`, generate a personalized config for `FiiO K17` and/or `JadeAudio JA11`, and optionally enable the `systemd --user` service.
- **Success Criteria & Proof:**
  - `audigma configure` produces a valid, passing configuration from scratch without hand-editing TOML.
  - Exported WirePlumber templates and systemd user services parse and validate cleanly.
- **STOP:** Update `# Handoff Brief` and terminate session.

---

### Phase 6: Host Audio Doctor (`audigma-doctor`), Interactive Remediation (`--fix`) & Polish
**Maps to:** `Design.md §2` and `§7`.

- **Goal:** Implement the dedicated `audigma-doctor` crate to inspect the host audio environment, verify PipeWire multi-rate setup and real-time scheduling limits, diagnose ALSA device lockouts, and provide interactive remediation (`audigma doctor --fix`), keeping platform-specific OS plumbing cleanly isolated.
- **Scope & Tasks:**
  1. Implement `audigma-doctor` with a platform diagnostic trait (`DiagnosticProvider`):
     - Linux ALSA device lock inspection (`/dev/snd/pcm*` file descriptor queries).
     - PipeWire multi-rate clock inspection (`default.clock.allowed-rates`) when `rate_policy = "source-family"` is active.
     - Linux `RLIMIT_RTPRIO` / RTKit real-time priority diagnostics.
  2. Implement interactive remediation (`audigma doctor --fix`):
     - If PipeWire/WirePlumber holds an ALSA device needed for exclusive DSD: dynamically call `wpctl set-profile <id> 0` or offer to write a permanent WirePlumber drop-in rule in `~/.config/wireplumber/`.
     - If a foreign daemon (e.g. MPD) holds the ALSA device: interactively prompt the user before terminating the process, escalate with `sudo` if required, or print the exact fallback command.
     - If PipeWire is pinned to a single rate: offer to generate the multi-rate PipeWire configuration snippet.
  3. Run final workspace-wide regression tests, clippy lints, and formatting checks across all 8 crates.
- **`[User-Led]` Actions:**
  - Run `audigma doctor` and `audigma doctor --fix` in both healthy and simulated lockout states (e.g. with MPD actively holding `K17` or WirePlumber monitoring it), confirming accurate diagnosis and clean interactive resolution.
- **Success Criteria & Proof:**
  - `audigma doctor` accurately detects all device lockouts and rate misconfigurations.
  - Interactive `audigma doctor --fix` successfully resolves lockouts with clear prompts and zero crashes.
  - `cargo test --workspace`, `cargo clippy --workspace --all-targets`, and `cargo fmt --check` pass 100% cleanly across all 8 crates.
- **STOP:** Update `# Handoff Brief` and mark plan complete.

---

# Handoff Brief

- **Current Phase:** Phase 4 (PipeWire Virtual Sink, Shared-Graph PCM, D-Bus Device Reservation & Adaptive Clock Bridge) — [Completed]
- **Next Phase:** Phase 5 (Interactive Configurator `audigma-configurator` & WirePlumber Templates)
- **What was done in this session:**
  - **PipeWire Format Negotiation Resolution:**
    - Diagnosed that `build_format_pod` was passing `id: 1` instead of `libspa::param::ParamType::EnumFormat.as_raw()` (3), causing PipeWire to reject the format enumeration and fall back to single-port generic DSP format (`F32P`).
    - Fixed the POD object serialization in `PipeWireVirtualSink` using standard `libspa` types, and added `STREAM_CAPTURE_SINK => "true"` with `MEDIA_ROLE => "Music"`.
    - Confirmed PipeWire automatically creates discrete stereo ports (`input_FL`, `input_FR`) and auto-links them to the sink monitor (`audigma_k17_sink:monitor_FL/FR`).
  - **Chunk Slicing & Frame Preservation:**
    - Discovered that `read_stereo` was popping full 1024-frame chunks from the SPSC ring buffer, copying only the requested 256-frame tile, and discarding the remaining 768 frames (dropping 75% of incoming audio and causing the garbled "musical beat without notes").
    - Added persistent cursor buffering (`current_chunk: Option<TaggedChunk>`, `read_offset: usize`) to `PipeWireVirtualSink` so unread frames are seamlessly preserved and consumed on consecutive calls.
  - **Hardware Listening Tests Verified:**
    - **FiiO K17:** Verified clean, musical Native DSD256 playback from desktop applications (Edge/KDE) with zero XRuns and zero overloads.
    - **JadeAudio JA11:** Verified clean, musical DoP64 playback in `S24_3LE` container format with zero XRuns.
    - **Software Digital Volume Control:** Verified real-time software attenuation via Unix control socket (`audigma set volume -6`, `-10`, `0`) operating cleanly on floating-point PCM before modulation, preserving DSD marker and bitstream integrity.
  - **Workspace Quality Gate:**
    - `cargo test --workspace` passed 100% (all unit and integration tests across all crates).
    - `cargo clippy --workspace --all-targets -- -D warnings` passed with 0 warnings.
    - `cargo fmt --check` passed cleanly.
- **Action Items for Next Session:**
  1. Initiate Phase 5: Scaffold `audigma-configurator` crate with interactive TUI (`ratatui` / `crossterm`) and one-shot Web wizard.
  2. Implement guided configuration workflow, full `[[rules]]` evaluation, and `audigma config explain`.
  3. Package systemd user services, WirePlumber configuration templates, shell completions, and manpages.


