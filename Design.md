# audampler — Product, Architecture, and Delivery Plan (v2.1)

**Status:** proposed implementation plan; no implementation exists yet. Numerical quality targets below are engineering goals, not measured results. Configuration examples define the intended interface and become executable acceptance fixtures as it is implemented.

## 1. Vision and release scope

`audampler` is a Rust audio conversion engine, Linux audio service, and offline converter for high-quality PCM resampling and PCM-to-1-bit delta-sigma conversion (DSD). It brings the configurable conversion workflow of foobar2000's `foo_dsd_processor` into the Linux audio ecosystem, with PipeWire integration, independently reusable modules, inspectable signal paths, and reproducible quality measurements.

Host-side interpolation and modulation provide control over processing before the DAC. They do not inherently improve every DAC, recover information absent from the source, or guarantee bypass of the DAC's internal processing. Quality claims must distinguish digital conversion performance from measured analog DAC performance.

### 1.1 What should make this better integrated and easier to use?

1. **A normal desktop audio destination:** select “audampler” in the desktop sound settings; route music players, browsers, and PulseAudio/JACK-compatible applications through PipeWire without requiring a player plugin.
2. **A dedicated music mode:** preserve the negotiated input rate where possible, choose conversion per rate/format, and use an exclusive, verified output path.
3. **Understandable configuration:** named presets first; explicit filter, gain, rate, and modulator controls for advanced users. Show the consequences in bandwidth, latency, compute cost, and headroom.
4. **Transparent operation:** show source/graph/output rates separately, actual transport, processing stages, applied gain, clock correction, and the reason a preset or fallback was selected.
5. **One engine everywhere:** the same prepared DSP pipeline serves offline conversion, benchmarking, raw streams, ALSA, and PipeWire.
6. **Evidence-backed profiles:** distribute versioned filter/modulator profiles with coefficients, provenance, measurement reports, and validated operating limits.

### 1.2 Release boundaries

| Release | Required scope |
|---|---|
| Research gates | Reference DSP harness; exact DSD transport/clocking prototypes; documented hardware capability matrix |
| Offline alpha | Stereo WAV/FLAC input; high-rate PCM WAV and DSF output; raw streaming; DSD64/128/256 candidates promoted only after verification |
| Linux beta | PipeWire PCM virtual sink; shared desktop mode; PipeWire-input/direct-ALSA-output DSD bridge; ALSA loopback input; CLI setup/status; user service |
| v1.0 | Measured release profiles; reliable reconnect/recovery; rule-based selection; TUI setup; packaged examples; sustained playback tests on named hardware |
| Later | Qualified native PipeWire DSD paths, DSD-to-PCM and DSD remodulation, DSD512+, multichannel, optional EQ/convolution, local web UI, advanced modulator research |

Native PipeWire DSD may ship earlier if its end-to-end qualification gate passes. DSD512 and higher are optional capabilities, not prerequisites for a useful release. Stereo is the first validated channel layout; channel metadata and DSP interfaces must support later expansion.

## 2. Corrections to the original draft

| Original assumption | Corrected design decision |
|---|---|
| `f32` inherently delivers 140–150 dB FIR rejection and has no possible benefit from `f64` | Arithmetic error depends on coefficients, accumulation, signal, and implementation. Start with an `f64` reference and production high-quality path; qualify faster kernels against measured error budgets. |
| A fixed stopband/transition width applies to every stage | Specify end-to-end passband, ripple, rejection, and latency; allocate stage-specific alias/image budgets. |
| Apodizing is a third phase type | Phase and apodization are independent choices. Apodization trades high-frequency bandwidth for suppression of some upstream filter effects; it cannot universally undo mastering artifacts. |
| Modulator order determines MSA and a guaranteed Lee bound | Stability is profile-, scaling-, signal-, dither-, and rate-dependent. Lee's criterion is a heuristic, not a stability proof. |
| `max(min(...))` is a soft clamp that leaves the transfer function intact | It is hard clipping. Clipping and state leaking change loop behavior and must not mask an unstable profile. |
| Small internal dither has no noise penalty | Injection point and amplitude affect noise and stability; qualify them as part of the profile. |
| Fade the DSD/DoP output ring buffer | Gain and fades operate before modulation. Packed DSD/DoP must never pass through PCM gain, mixing, limiting, or resampling. |
| A normal PipeWire PCM link transports DoP transparently | Normal adapters can alter the payload. Require an explicitly verified exact path, native DSD negotiation, or direct ALSA egress. |
| High-rate PCM support proves DoP support | It proves only a transport prerequisite. DAC DoP decoding requires separate evidence. |
| `DSD256` automatically means either clock family | Use an explicit bit rate or multiplier plus base clock; 48 kHz-family DSD requires explicit device support. |
| Fixed attenuation guarantees no reconstructed overs | It does not. Measure at the modulator input and handle excess according to an explicit policy. |
| DSF stores bit-interleaved channels | DSF uses per-channel blocks, normally 4096 bytes, and declared within-byte bit ordering. |
| 1 kHz is coherent with a 1M-point DSD256 FFT | Coherence requires `f_tone = k * f_s / N`; 1 kHz does not satisfy that example. |
| A one-hour simulated run proves zero hardware XRuns | Simulation tests scheduling logic; real devices require separate deadline and soak tests. |

## 3. Signal-path architecture

### 3.1 Three supported operating models

**A. Shared desktop sink — convenience first**

```text
Applications → PipeWire PCM mixing/graph rate → audampler virtual sink
             → PCM conditioning → SRC → [DSM] → verified output backend → DAC
```

- Accept the PCM graph's negotiated rate; do not claim access to the original file rate after upstream resampling or mixing.
- Use a fixed output rate by default to avoid repeated DAC relocking and desktop interruptions.
- Desktop volume, mute, and application mixing occur before modulation. Apply audampler's profile headroom after upstream mixing.
- PCM output can return to an ordinary PipeWire sink. DSD output uses the qualified paths below.
- A PCM monitor port supports recording, visualization, and screen sharing without exposing packed DSD as ordinary audio.

**B. Dedicated music path — conversion control first**

```text
Player / file / explicitly negotiated stream → audampler → exclusive DAC path
```

- Select a source-family target only when the actual input rate is known. On a PipeWire virtual sink this requires the graph to permit rate switching (`default.clock.allowed-rates` covering both families plus multi-rate `EnumFormat` without a rate lock); a graph pinned to a single rate (commonly 48000) silently resamples upstream and defeats music mode — see `doctor` check in §7.
- Preserve input samples up to the selected DSP stages; report any upstream conversions that can be observed.
- Support gapless continuation across compatible tracks, optional keep-open silence, and rate-change hysteresis.
- Dedicated native DSD/explicit DoP input can later bypass DSP when output is compatible. A mixed desktop PCM sink is not a DSD passthrough input. Scope: v1.0 provides bit-exact passthrough only on compatible file/ALSA inputs via the typed passthrough path (§3.3); the full DSD-input matrix with remodulation options (§6.1) follows later.

**C. Offline/pipe conversion — reproducibility first**

```text
File decoder / declared raw stream → same DSP engine → file writer / raw packer
```

- Process without audio-device clocking or adaptive drift correction.
- Preserve sample counts, define filter-tail handling, and record the resolved configuration and profile hashes.
- Support album-continuous processing separately from independent-track processing.

### 3.2 Separate input and output backends

Do not use one `backend` key for both sides. Input and output are independently selected:

- Inputs: file, stdin, PipeWire PCM, ALSA capture/loopback; explicit DSD-capable inputs later.
- Outputs: file, stdout, PipeWire PCM, ALSA PCM/native DSD/DoP; qualified PipeWire DSD later.
- Control/UI code configures a pipeline; it never implements separate audio processing.

For direct ALSA DSD output, audampler owns the DAC PCM device exclusively. Provide a documented, device-specific WirePlumber rule to release that endpoint, while leaving other desktop devices available. Restore routing on normal shutdown; avoid automatic selection of audampler's own sink as its output.

### 3.3 Prepared pipeline and data contracts

```text
decode/unpack
  → validate/channel-map
  → user volume + headroom gain + optional PCM DSP
  → rational/adaptive SRC + staged interpolation
  → final peak/overload guard
  → PCM quantization OR delta-sigma modulation
  → pack PCM/native DSD/DoP
  → output
```

- `StreamSpec` carries sample domain, exact rate, channel positions, and format/layout; use distinct types for PCM sample rates, DSD bit rates, and transport frame rates.
- `prepare(spec, config, capabilities)` resolves rates, coefficients, buffers, latency, and backend parameters off the real-time thread. The planner factors adaptive ratios per §4.2 (`R_asrc × R_rational × 2^K`) and reports the resolved per-stage chain in `config explain`.
- `process(input, output)` reports consumed/produced counts without allocating. Rational SRC must support variable counts and arbitrary caller block sizes.
- `drain()`/end-of-stream policy distinguishes filter tail from program duration. Reset, seek, discontinuity, and rate changes are explicit events. Distinguish planned PCM-domain fades (pre-SRC gain ramps that take `filter_group_delay` to reach the modulator — pipeline must clear before switching) from emergency overload faults (which bypass the filter tail and swap idle at the modulator boundary per §4.5.3).
- Retain resampler history, modulator states, packing residue, and dither RNG state across ordinary block boundaries.
- DSD passthrough is a separate typed path with representation changes only; it cannot accidentally enter PCM processing.
- Keep block sizes bounded. Stream final interpolation tiles into the modulator instead of materializing seconds of MHz-rate floating-point audio.

## 4. DSP quality and profile design

### 4.1 Definitions and reference levels

- Internal PCM uses normalized floating-point values: a sine with peak amplitude `1.0` has `0 dBFS` peak. Intermediate values above unity are representable.
- The reference 1-bit quantizer emits `-1` or `+1`; bit packing maps these to `0`/`1` in the declared order.
- Define the PCM-to-modulator gain explicitly. DSD recording/playback reference-level conventions can differ from full pulse-density scale; report the convention and calibrate rather than assuming equal analog loudness.
- `OSR = f_bit / (2 * B)`, where `B` is the design baseband. DSD's family multiplier is not automatically this OSR. DSD256 at 11.2896 MHz with `B = 20 kHz` has OSR `282.24`.

### 4.2 Resampling and interpolation

1. Implement a rational polyphase windowed-sinc SRC for ratios such as 44.1↔48 kHz and arbitrary input rates. Use a continuously adjustable fractional-phase path for clock drift. When `clock.mode = "adaptive-input"`, factor the total ratio as `R_total(t) = R_asrc(t) × R_rational × 2^K`: the continuously variable fractional-phase stage always executes at a low baseband/intermediate rate (1× or 2× input rate, e.g. 88.2/96 kHz) *prior* to the fixed 2× half-band cascade. Even when the nominal ratio is an exact power of two (e.g. 44.1 kHz → DSD256 at 256×, or 48 kHz → DSD256/48 at 256×), the actual ratio is `2^8 × (1 ± δ_ppm)`; the planner must never schedule fixed half-band stages alone with no stage left to absorb drift, and must never run variable interpolation at the final MHz rate.
2. Use multistage 2× interpolation, including sparse half-band FIRs where their symmetry and transition requirements fit. These stages are fixed-coefficient synchronous stages. A minimum-phase transformation generally loses linear-phase symmetry and half-band sparsity; budget its actual cost.
3. The planner may first reduce an unnecessarily high PCM rate when the chosen target bandwidth permits it, using verified anti-alias filtering.
4. Generate coefficients in `f64` or higher-precision design tooling. Store generation parameters, normalization, checksum, and measured response.
5. Use `f64` coefficients/accumulation for the reference and initial high-quality engine. Add mixed-precision or `f32` kernels only after measuring coefficient quantization, rounding, near-zero behavior, and complete-cascade performance.
6. Use stable Rust and runtime-dispatched scalar/SIMD kernels (for example `std::arch` or a vetted SIMD crate). Do not assume portable `std::simd`/`core::simd` is stable on the selected MSRV. Record FMA and denormal behavior. Inside the 1-bit DSM loop the quantizer feedback is strictly `{-1.0, +1.0}` (never `0.0`), so integrator states stay excited and rarely approach subnormals; subnormal risk concentrates in upstream PCM conditioning / FIR / IIR stages fed digital zero. Set hardware FTZ/DAZ per DSP-worker thread entry during `prepare`/`run` (x86 MXCSR / ARM FPCR) to eliminate tail latency spikes across all stages.

**Filter controls:**

- `phase = linear | minimum`; optional mixed-phase research later.
- Independent `apodization = off | gentle | custom` with explicit bandwidth consequences.
- Passband/stopband edges in Hz or an explicitly defined normalization, passband ripple in dB, stopband rejection in dB, and latency/CPU limits.
- Resolve generic presets into per-rate specifications; reject impossible combinations such as a 20 kHz passband for a 32 kHz source.
- Linear-phase filters have constant group delay, not zero latency. Minimum-phase filters concentrate energy earlier with nonlinear phase; they do not eliminate all ringing throughout the signal chain.

**Initial quality targets:**

| Preset family | Passband ripple goal | End-to-end image/alias rejection goal | Intent |
|---|---:|---:|---|
| `balanced` | ≤ 0.01 dB | ≥ 120 dB | Practical real-time default |
| `reference` | ≤ 0.001 dB | ≥ 140 dB | Measurement/offline and capable CPUs |
| `low-latency` | Declared per rate | Declared per rate | Shorter filters with visible tradeoffs |

These are digital SRC targets relative to a defined full-scale test signal, not guarantees of complete PCM-to-DSD SNR. For 44.1/48 kHz input, target a 20 kHz passband unless the user selects apodization. Publish transition edges, group delay, impulse/step plots, and implementation precision for each resolved design. Allocate stage budgets from the final image/alias measurements rather than requiring identical filters everywhere.

### 4.3 Delta-sigma modulators

**Initial implementation:** scalar `f64` state-space reference plus a specialized fifth-order CRFB candidate. Add seventh-order CRFB and alternative CIFB/CIFF realizations after the first profile passes validation.

- Synthesize NTFs offline using a documented Schreier-style process; specify bandwidth, OSR, zero optimization objective, peak NTF gain, and realization/scaling.
- Validate both noise transfer function (NTF) and signal transfer function (STF). An NTF alone does not establish signal gain or transient behavior.
- Use `H_inf ≈ 1.5` only as an initial search parameter for a 1-bit candidate. There is no universal order-specific guaranteed range.
- Measure maximum stable input over a defined stimulus suite and duration. Publish a verified operating envelope with margin, not a universal “theoretical MSA” inferred from order.
- Include DC, near-DC, high-frequency tones, multitone/high-crest-factor signals, bursts, silence, and overload recovery, not just a 1 kHz sine sweep.
- Sequential feedback limits ordinary time-axis SIMD; benchmark channel-lane SIMD and fixed channel workers against a single stereo worker. Threading overhead may outweigh gains.

**Every released modulator profile contains:**

| Field group | Required data |
|---|---|
| Identity | Immutable ID/version, coefficient hash, generator version, license/provenance |
| Design | Topology/order, coefficients, state scaling, STF/NTF, design bandwidth, exact supported bit rates |
| Limits | Verified input envelope, required headroom, per-state operating bounds, recovery policy |
| Dither | Injection node, distribution, amplitude normalization, seed policy, measured noise/idle-tone effects |
| Results | In-band SNR/SINAD/idle spectra, ultrasonic power bands, stress-test duration, CPU/memory/latency |

Presets may select among profiles; they must not independently override topology/order while retaining unrelated coefficients. Any changed coefficients, state scaling, or dither settings create an experimental profile that must be requalified.

### 4.4 Gain, peaks, dither, and overload

- Use `preamp_db` as a signed gain: negative attenuates. Keep user volume, loudness normalization, and profile headroom separately visible.
- Required total attenuation is based on measured reconstructed peaks and the profile's operating limit. If a predicted peak is `P dBFS`, the allowed peak is `L dBFS`, and margin is `M dB`, require `gain_db ≤ L - M - P`.
- `headroom = profile` applies the profile's documented baseline attenuation. It is not a proof that arbitrary material cannot overload. Each profile manifest must publish its baseline in dB so `config explain` shows the numeric total (`preamp_db` + volume + headroom).
- Meter the final PCM signal entering the modulator. Initial live overload policy is fault/mute/recover; optional lookahead limiting is explicitly enabled and reported. Offline two-pass peak analysis can select a fixed gain without dynamic limiting: because the SRC + interpolation cascade is linear and time-invariant, scaling the pre-SRC input by gain `G` scales the modulator-input peak by exactly `G` (given an identical SRC/DSP chain and no intermediate clipping), so Pass 1 runs only the PCM interpolation cascade or true-peak estimator and skips the sequential 1-bit DSM loop and bit packing.
- PCM integer-output dither is separate from DSM dither. Apply TPDF at the final integer quantizer when reducing precision; do not dither floating-point output or packed DSD as PCM. Noise-shaped PCM quantization is a later, rate-qualified feature, not “linear-phase dither.”
- Define DSM dither relative to the quantizer step (`2` for a ±1 quantizer), with peak/RMS conventions. Select the injection point through analysis and measurement; there is no unconditional noise-free dither amplitude.
- Nonfinite input/state triggers an immediate fault. State limits are detectors and last-resort containment, not routine clipping in a supposedly linear loop. A profile fails qualification if protection is needed inside its declared operating envelope.

### 4.5 Silence and transitions

**Never multiply or crossfade packed DSD/DoP samples, and never use all-zero DSD bytes as audible silence.** All-zero bits represent a full-polarity density, not zero signal.

1. For planned gain/rate/profile changes, fade PCM before modulation (pre-SRC ramp), allow pipeline delay (`filter_group_delay`) to clear, and emit a verified DSD idle stream while the DAC remains in DSD mode.
2. Use a tested zero-input modulator stream or qualified balanced idle sequence; verify its spectrum, bit ordering, channel synchronization, and boundary behavior.
3. For an unexpected overload, replace unsafe output with the idle stream directly at the modulator boundary (bypassing filter-tail latency); reset/reprime the loop through a tested recovery sequence and resume with a PCM-domain fade. Do not promise zero transient from every emergency boundary.
4. Preserve DoP marker alternation while sending idle data. During format/rate renegotiation, coordinate backend drain/stop/reopen/prefill and a device-specific settling interval.
5. DSD passthrough cannot have a software gain ramp while staying bit-exact. Use device mute if available or a documented stop/idle transition; changing gain requires remodulation.
6. PCM pipelines with compatible formats can crossfade after latency alignment. DSD profile changes use mute/reprime rather than blending bitstreams.

Define the lifecycle as `Stopped → Preparing → Priming → Running → Draining/Muting → Reconfiguring`, with `Recovering` and `DeviceLost` branches. Test every transition. Hardware relay clicks and analog behavior require device-specific tests; software cannot universally eliminate them.

## 5. Linux transport, rate negotiation, and clocks

### 5.1 Exact DSD rate and packing model

For conventional 44.1 kHz-family DSD:

| Label | DSD bit rate/ch | DoP PCM frame rate | ALSA DSD_U32 frame rate | SPA DSD byte rate/ch |
|---|---:|---:|---:|---:|
| DSD64 | 2,822,400 | 176,400 | 88,200 | 352,800 |
| DSD128 | 5,644,800 | 352,800 | 176,400 | 705,600 |
| DSD256 | 11,289,600 | 705,600 | 352,800 | 1,411,200 |
| DSD512 | 22,579,200 | 1,411,200 | 705,600 | 2,822,400 |

- Standard stereo DoP carries 16 DSD bits per channel per PCM frame; alternate `0x05`/`0xFA` markers in the upper byte of the 24-bit payload, synchronized across channels. Explicitly handle 24-in-32 alignment and signed container representation. USB Audio Class 2.0 DACs expose DoP over three ALSA container variants that must each have distinct golden pack/unpack vectors: `s32le` MSB-justified (24-bit DoP frame shifted left by 8, `0x05_HI_LO_00`), `s24le` LSB-justified in a 32-bit word (`0x00_05_HI_LO`), and `s24_3le` packed 24-bit (3 bytes/sample). Probe which variant each device accepts; do not assume one covers the others.
- ALSA native DSD variants include U8/U16/U32 and supported byte orders. For `w` payload bits per sample container, the ALSA frame rate is `f_bit / w`. Probe actual format/rate/channel combinations, not independent maxima.
- SPA `audio/dsd` expresses rate in bytes per second per channel, with separate bit order and signed byte-interleave fields [1]. Convert at the API boundary; never substitute the bit clock directly.
- 48 kHz-family DSD rates are `48,000 * multiplier`; show this explicitly (for example `DSD256/48 = 12.288 MHz`). Do not infer support from conventional DSD256 support.
- Human-facing buffering uses milliseconds. Display the negotiated hardware frames, frame rate, bytes, and total latency. For example 65,536 U32 DSD256 frames are approximately 185.76 ms, but the same frame count in a different transport has a different duration.

### 5.2 Output-path qualification

| Path | Policy |
|---|---|
| PipeWire PCM input → PipeWire PCM output | Standard desktop integration; report downstream SRC/format conversion where observable |
| PipeWire PCM input → direct ALSA native DSD | Primary DSD bridge after exclusive ownership and clock-drift handling pass |
| PipeWire PCM input → direct ALSA DoP | Supported only with confirmed DAC DoP capability and exact integer packing |
| PipeWire PCM input → native SPA DSD output | Early feasibility prototype; release only for tested version/device combinations |
| DoP through generic PipeWire PCM DSP/mixer | Not a supported default; requires independently proven exact transport without PCM processing |

Native SPA DSD support exists, but a type definition does not prove every adapter, session-manager policy, or device path supports it. Prototype buffer negotiation, scheduling/rate semantics, and ALSA backend support using the selected PipeWire/pipewire-rs versions before promising this path.

For DSD output, prohibit unverified fallback to a generic PCM sink, Bluetooth device, or default desktop output. A configured PCM fallback must stop DSD transmission, negotiate PCM, and restart the appropriate pipeline.

### 5.3 Clock domains and adaptive SRC

A ring buffer does not solve long-term clock mismatch. PipeWire input and a directly opened DAC may use independent clocks; ALSA loopback and a physical DAC may also drift.

- Make the hardware output clock the timing reference for the bridge.
- Track timestamps and buffer occupancy; apply a bounded, slowly varying ratio correction in the low-rate PCM ASRC stage before modulation and before the fixed half-band cascade (see §4.2). Never place the variable-ratio stage at the final MHz rate.
- Configure a maximum correction range in ppm; expose measured correction and queue fill. Fault/recover when the estimate or occupancy leaves valid limits.
- Avoid sample dropping/duplication as normal drift control. Never resample the completed DSD bitstream.
- In a verified single-clock native PipeWire path, let PipeWire provide pacing and avoid a second adaptive controller.
- DSD passthrough requires compatible pacing/backpressure or a shared clock. Independent live DSD clocks cannot be reconciled with PCM ASRC while remaining bit-exact.
- A “strict rate” mode uses a proven common-clock/backpressured source; it must not silently disable necessary drift handling on an independent-clock bridge.

### 5.4 Device discovery and policy

1. Enumerate stable device identities (serial/vendor/product/path where available), PipeWire node properties, and ALSA endpoints; avoid persisting transient numeric IDs alone.
2. Query actual format/rate/channel/period combinations. Track capability states as `unknown`, `advertised`, `open-tested`, and `playback-verified`.
3. Test exact ALSA native DSD packing/rates and PCM transport candidates. High-rate PCM acceptance alone does not prove DoP decoding.
4. Collect DAC documentation/user confirmation and optional known-pattern playback evidence for DoP. Explain unsupported options in the UI rather than treating unknown as proven.
5. Save device-specific quirks: bit order, supported rate families, open/settling delays, suspend behavior, hardware volume/mute support, and tested versions.
6. Define explicit policies for busy device, disconnect, suspend/resume, no source, unsupported rate, and CPU overload. Do not silently change quality mid-track; use an enabled fallback policy at a controlled boundary.

## 6. Product features and configuration

### 6.1 Presets, rules, and usability

**v1.0 essentials:**

- Guided setup: choose DAC → probe → choose desktop/music mode → benchmark candidate profiles → save → optionally enable user service.
- Presets such as `desktop-balanced`, `music-reference`, and `pcm-nos`, backed by measured filter/modulator profiles. Names describe tradeoffs rather than imply universally better sound.
- Ordered first-match rules by observed input format/rate/channel count and output device; rule result selects a complete preset. `explain` displays the matching rule and resolved plan.
- Source-family, fixed-rate, and explicit per-rate mappings. Mixed desktop audio uses graph-rate rules; application-specific routing is handled before mixing via WirePlumber or separate virtual sinks.
- Desktop volume/mute integration in the PCM domain, visible profile headroom, and a transport-independent user volume.
- Preset preview with response plots, estimated latency, headroom, and benchmark results. Export/import versioned configurations and device profiles.
- Gapless compatible playback, keep-open/idle timeout choices, device reconnect, and persistent stable routing.
- CLI and TUI backed by the same configuration/control API; shell completions, manpage, and clear errors with suggested valid settings.

**Later extensions aligned with the vision:**

- DSD input matrix: bit-exact passthrough when compatible; explicit DSD → high-precision PCM → DSP → DSD remodulation for volume, EQ, or rate conversion. Label the latter as a lossy processing path.
- Optional pre-modulation parametric EQ, convolution/room correction, crossfeed, channel mapping, and ReplayGain when metadata is available. Include their gain and latency in the planner.
- A/B comparison using level matching and latency alignment; compare captured PCM reconstructions for rapid testing rather than claiming seamless switching between different DAC modes.
- Multichannel with explicit layouts and hardware validation; channel remapping/mixing happens before modulation.
- Optional localhost web configurator using the same schema and control protocol. Bind loopback, use a per-session token/origin checks, and stop the setup server on save.
- Research-only higher-order, alternative topology, or lookahead/trellis modulators, each evaluated against the same quality and CPU criteria.

### 6.2 Configuration contract

- TOML with `schema_version = 1`; reject unknown keys and conflicting fields. Use backend-tagged input/output tables rather than a large set of ignored options.
- Defaults → named preset → file overrides → explicit CLI overrides. Runtime controls are temporary unless explicitly saved. Rule selection is resolved before applying explicit processing overrides; surface conflicts.
- `config check` validates structure and cross-field constraints without hardware; `probe`/`doctor` add device checks. `config explain` prints the fully resolved path and why.
- Separate signal target (`pcm`/`dsd`, exact rate policy) from transport (`native`/`dop`/`pcm`) and container (`wav`/`dsf`/`raw`). DSF is not an ALSA “native framing” mode.
- `auto` values must resolve to explicit measured profiles/rates before running. No experimental profile becomes a default solely because it uses a higher order or rate.
- Live gain changes ramp in PCM. Rate/profile/backend changes prepare off-thread and use a controlled transition; failed reloads retain the last valid configuration.
- Device records and measurement caches live under XDG state/cache paths; configuration under `$XDG_CONFIG_HOME/audampler/config.toml` (default `~/.config/audampler/config.toml`).

The following are standalone proposed examples. `profile = "auto-verified"` is a resolver policy: select an installed qualified profile for the exact target rate or fail with an explanation. It is not an unmeasured coefficient set.

**Rules vs. `rate_policy`:** `[[rules]]` (ordered first-match) selects a complete preset; after resolution `[target]` holds one explicit rate. `rate_policy = "source-family"` plus `pcm_rate_44100_hz` / `pcm_rate_48000_hz` is shorthand resolved through the same rule/family map, not an orthogonal mechanism — `config explain` must show the matched rule and the final explicit rate. Example shape (Ticket #4 turns this into fixtures):

```toml
schema_version = 1

[[rules]]
when_input_rate_family = "44100"   # 44.1k, 88.2k, 176.4k, ...
apply_preset = "music-reference"
set_target = { format = "pcm", pcm_rate_hz = 705600 }

[[rules]]
when_input_rate_family = "48000"   # 48k, 96k, 192k, ...
apply_preset = "music-reference"
set_target = { format = "pcm", pcm_rate_hz = 768000 }
```

### 6.3 Shared desktop PCM sink (`desktop-pcm.toml`)

```toml
schema_version = 1

[input]
backend = "pipewire"
node_name = "audampler_sink"
mode = "desktop"
channels = 2

[output]
backend = "pipewire"
target_node = "alsa_output.usb-EXAMPLE_DAC.analog-stereo"
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

[runtime]
latency_target_ms = 40
on_device_loss = "wait"
```

The stream format is not a promise about the DAC's hardware format. Report PipeWire's downstream negotiation; use a verified direct PCM path if exact final quantization is required.

### 6.4 PipeWire desktop input, direct native DSD output (`desktop-dsd.toml`)

```toml
schema_version = 1

[input]
backend = "pipewire"
node_name = "audampler_sink"
mode = "desktop"
channels = 2

[output]
backend = "alsa"
device = "hw:CARD=EXAMPLE,DEV=0"
transport = "native"
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
profile = "auto-verified"

[clock]
mode = "adaptive-input"
max_correction_ppm = 500

[runtime]
settle_ms = 50
on_device_loss = "wait"
fallback = "none"
```

This deliberately converts a 48 kHz desktop graph to a fixed conventional DSD256 rate when necessary (a 235.2× cross-family rational ratio, not family-preserving; prefer fixed-per-family targets when the graph rate is known). Buffer/period values are requests, rounded to supported hardware values and reported. The DAC must be released by PipeWire before direct ALSA access.

### 6.5 MPD/ALSA loopback to high-rate PCM (`mpd-pcm.toml`)

```toml
schema_version = 1

[input]
backend = "alsa"
device = "hw:Loopback,1,0"
channels = 2

[output]
backend = "alsa"
device = "hw:CARD=EXAMPLE,DEV=0"
transport = "pcm"
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

MPD writes to the paired `hw:Loopback,0,0`. Validate loopback format/rate ownership and reopen behavior rather than assuming capture can transparently follow every writer change. Unknown rate families require an explicit rule or error. The 24 valid bits must match the negotiated endpoint; a 32-bit container does not establish 32-bit DAC precision.

### 6.6 Offline WAV/FLAC → DSF (`offline-dsf.toml`)

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
profile = "auto-verified"

[offline]
seed = 12345
tail_policy = "trim-to-duration"
write_report = true
dsf_pad_mode = "spec-zero"  # "spec-zero" (default) | "dsd-silence" (see §8.3)
```

`peak_analysis = "two-pass"` runs Pass 1 on the PCM interpolation cascade only (skipping DSM/packing per §4.4) to compute the fixed gain, then Pass 2 runs the full chain.

`trim-to-duration` compensates declared pipeline delay and uses a documented output-count rounding rule; `preserve-tail` records the extended duration. Album-continuous conversion maintains state across tracks and splits output at exact rational boundaries.

### 6.7 Raw PCM → raw DoP (`stdio-dop.toml`)

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
profile = "auto-verified"
```

```sh
# Hardware-free conversion; stdout contains audio only, diagnostics use stderr.
audampler convert -c stdio-dop.toml -i - -o - < track.s16le > track.dop.s24le

# Playback requires an exclusive DAC already confirmed to decode this DoP mode.
aplay -D hw:CARD=EXAMPLE,DEV=0 -t raw -f S24_3LE -r 352800 -c 2 track.dop.s24le
```

Raw streams require complete format declarations. Header-finalized DSF/WAV output requires seekable output or a known-length strategy; reject unsupported stdout combinations rather than writing invalid headers. Handle pipe backpressure, partial frames, EOF, and broken pipes explicitly.

## 7. CLI, service, and observability

Use subcommands rather than mutually interacting mode flags:

```text
audampler run [-c CONFIG]
audampler convert -i INPUT -o OUTPUT [-c CONFIG]
audampler benchmark [-i INPUT] [-c CONFIG] [--json]
audampler probe [--backend alsa|pipewire] [--json]
audampler configure [--tui|--web] [--out PATH]
audampler config check [-c CONFIG]
audampler config explain [-c CONFIG] [--input-rate HZ] [--json]
audampler profiles list
audampler profiles show ID
audampler status [--watch] [--json]
audampler set volume DB
audampler set preset NAME
audampler reload
audampler doctor [--json]
```

- `run` stays in the foreground; ship a `systemd --user` unit rather than self-daemonizing. Startup order and reconnect logic account for PipeWire/WirePlumber availability.
- `benchmark` accepts a file or generated stimuli, excludes decoder/disk time from a separately reported DSP-only measurement, and also reports end-to-end throughput.
- `doctor` reports permissions, busy devices, routing loops, mismatched rates, unverified DoP, missing profiles, and CPU margin with actionable remedies. It inspects PipeWire `clock.allowed-rates` and upstream link resample state and warns when `mode = "music"` or `rate_policy = "source-family"` is configured on a graph locked to a single clock rate.
- Use a local Unix-domain control socket under `$XDG_RUNTIME_DIR/audampler/`; CLI/TUI/web share the same versioned request/status model.
- Report input and graph rates, output bit/sample/transport rates, profile hashes, gain components, measured peaks, latency breakdown, queue occupancy, clock ppm, XRuns, recoveries, and stage timing.
- Expose requested versus actual output path. “Bit-exact” applies only to a verified passthrough path, not to converted audio.
- Keep real-time telemetry to bounded counters/queues. Format logs, write reports, and render UI on non-audio threads.

## 8. Real-time implementation and workspace

### 8.1 Threading and memory

- PipeWire callbacks move bounded buffers and timestamps; no decoding, coefficient design, blocking I/O, logging, mutex waits, or heap allocation in the audio callback.
- Start with one fixed DSP worker and a backend output thread when clock domains require decoupling. Benchmark before adding per-channel workers; no general-purpose work-stealing pool in the real-time path.
- Preallocate SPSC queues and scratch storage. Define backpressure/overflow behavior, channel synchronization, and generation tags so old-format blocks cannot reach a newly configured device.
- Acquire supported real-time scheduling through normal user-session facilities (PipeWire/RTKit where applicable); keep an observable fallback if unavailable. Make memory locking and CPU affinity optional measured optimizations.
- Prime code/data and buffers before activation; account for page faults, denormals (per-thread FTZ/DAZ set on DSP-worker entry per §4.2), CPU frequency changes, and thermal throttling in hardware benchmarks.
- Share immutable prepared plans; acknowledge swaps at block boundaries and reclaim old buffers on the control thread.

### 8.2 Proposed workspace

```text
crates/
  audampler-core/       # PCM/DSD types, SRC, gain, quantizers, DSM; no OS I/O
  audampler-profiles/   # Versioned coefficients, manifests, verification metadata
  audampler-format/     # WAV/FLAC decoding, WAV/DSF writing, native/DoP packing
  audampler-engine/     # Pipeline planner, lifecycle, clocks, worker scheduling
  audampler-io/         # Feature-gated ALSA, PipeWire, file/stdio adapters
  audampler-config/     # Typed schema, rules, migration, validation; no UI dependency
  audampler-control/    # Local control/status protocol and service client
  audampler-cli/        # Commands, foreground service, interactive TUI
  audampler-web/        # Optional later UI
tools/
  design/              # Reproducible filter/NTF synthesis and reference scripts
  measure/             # Independent analysis and report generation
tests/
  fixtures/            # Licensed/generated audio, known packing vectors
  integration/         # Offline, PipeWire session, mock clocks, fault injection
docs/
  adr/                 # Recorded transport, numeric, and clocking decisions
  measurements/        # Versioned profile and benchmark reports
  hardware/            # Device/version compatibility records
examples/              # Validated configs and MPD/WirePlumber integration
```

Candidate dependencies: `clap`, `serde`/`toml`, `rtrb`, ALSA/PipeWire bindings, `rustfft`/`realfft`, a vetted PCM decoder such as Symphonia, and `hound` where its WAV support is sufficient. Evaluate RF64/large-file needs explicitly. Pin versions/MSRV and audit codec/DSP/tooling licenses before adopting or copying code. Keep offline/core builds usable without Linux audio libraries.

### 8.3 File/transport correctness

- DSF: little-endian chunk fields, declared bit order (`1` = LSB-first, `8` = MSB-first per specification), channel blocks, exact per-channel bit count, padding, file size, and metadata offset [4]. Padding beyond the declared sample count is never played as audio. Note the spec/hardware quirk: Sony DSF v1.01 §4 mandates `0x00` fill for unused trailing bytes of the final 4096-byte block per channel, but `0x00` is full-negative DC in DSD, so naive players that ignore `sample_count` and feed the full final block (~11.6 ms at DSD64) emit an end-of-track pop. On read, strictly truncate to `sample_count` bits. On write, keep `sample_count` exact and offer `dsf_pad_mode = "spec-zero" | "dsd-silence"`: `"spec-zero"` (default, spec-compliant `0x00` fill) versus `"dsd-silence"` (fill trailing bytes with `0x69` / zero-input modulator idle pattern for buggy players, documented as spec-deviating).
- Initially support WAV PCM and FLAC decoding; validate lengths, channel layouts, clipping/nonfinite input, and truncated files. Preserve supported metadata without claiming arbitrary tag round-trips.
- Implement independently checked pack/unpack vectors for each ALSA DSD format, DSF, and DoP; round-trip tests alone can hide matching bugs.
- DoP input detection is enabled only on an explicitly exact integer input path. Require sustained marker alternation across all channels and handle loss as a discontinuity; do not scan arbitrary mixed float audio and relabel it as DSD.

## 9. Measurement and acceptance criteria

### 9.1 Independent reference harness

- Compare the Rust implementation against an independently implemented state-space simulator and coefficient-generation output, not only against another copy of its hot loop.
- Generate DC, coherent tones, log sweeps, impulses, steps, multitone, high-crest-factor bursts, near-Nyquist input, silence, and deterministic random input.
- Cover 32/44.1/48/88.2/96/176.4/192 kHz PCM inputs, same/cross-family conversion, supported DSD rates, both channels, and varied block sizes.
- Verify block partition invariance under a fixed seed and kernel. Across SIMD/FMA/platform variants, use declared numeric/spectral tolerances; feedback sensitivity can produce different valid DSD bitstreams.
- Record input/generator/profile hashes, precision, tool versions, integration bands, windows, seeds, and durations with every report.

### 9.2 SRC tests

- Measure full-cascade passband ripple, attenuation/image rejection, DC gain, delay, phase, impulse behavior, and sample-count correctness against the resolved specification.
- Include coefficient quantization and runtime arithmetic; do not use only a theoretical frequency response.
- Test cross-family alias rejection, minimum-phase designs, and adaptive SRC under slow drift and ratio slews.
- Compare reference/faster kernels at low levels and worst-case signals; only release kernels that satisfy the preset budget.

### 9.3 Modulator tests

1. Sweep amplitude and frequency for each profile/rate with multiple seeds and initial conditions; refine around failure thresholds. Specify test durations in the manifest.
2. Require no nonfinite states, protection events, or sustained overload within the released operating envelope. Bounded states after clipping are not evidence of stability.
3. Evaluate long silence, tiny DC offsets, near-zero tones, bursts, and repeated overload/recovery for idle tones, DC shifts, and recovery time.
4. Measure ultrasonic noise power in stated bands (for example 20–100 kHz and 100 kHz–Nyquist, clipped to the actual Nyquist), plus total out-of-band power and peak NTF gain. Better in-band SNR alone does not establish a better DAC input.

**Spectral protocol:**

- Map bits to ±1. At DSD256, `N = 1,048,576` gives `Δf ≈ 10.77 Hz` and about 92.9 ms per segment. This is useful for a broad spectrum, but longer records are needed for narrow idle tones and low-frequency confidence.
- For coherent analysis use `f_tone = k * f_bit / N`; otherwise apply a specified window and account for leakage, coherent gain, equivalent noise bandwidth, and tone/harmonic bin integration.
- Use Welch averaging with documented segment count/overlap; plots of spectral density are not integrated SNR measurements.
- For baseband metrics, reconstruct through an independent high-precision low-pass/decimation chain. Prefer a same-family intermediate such as 176.4 kHz for 44.1-family DSD, then use a defined 20 Hz–20 kHz analysis band; 48 kHz conversion is not mandatory.
- Establish the analysis chain's noise/alias floor at least 10 dB below the metric being qualified and perform no final integer quantization in the measurement path.
- Report SNR (excluding harmonics), SINAD/THD+N (including them), idle noise, and dynamic range separately. State weighting and reference levels. Use “AES17-compliant” only for a separately specified and implemented compliant procedure.

**Initial aspiration:** a DSD256 reference profile exceeding 120 dB unweighted in-band SNR at a documented `-6 dBFS` input level, if that level is inside its verified envelope. This is not a blanket gate for DSD64, every preset, or dynamic range. Freeze per-profile SNR, SINAD, ultrasonic, and stability thresholds from the reference study before profile promotion.

### 9.4 Transport, clock, and real-time tests

- Golden-vector tests for bytes, bit order, markers, channel order, partial buffers, tail padding, and exact duration. Validate DSF with an independent reader and inspect metadata with `ffprobe`/`mediainfo` where supported.
- Private PipeWire test session: node registration, format negotiation, linking, mute/volume, target exclusion, graph restart, and reconnect. `pw-link` success alone does not prove audio integrity.
- Mock independent clocks: at least ±200 ppm drift, bounded slow drift changes, timestamp anomalies, producer stalls, and queue saturation; no unbounded queue growth under in-range drift.
- Instrument hot loops for allocation/blocking; test every lifecycle transition with partial writes, disconnect, underrun, profile reload, and missing capabilities.
- Proposed real-time release target on a named baseline CPU: sustained DSP throughput ≥ 2× real time and p99.9 worker time ≤ 70% of its audio-block duration, with zero missed deadlines/XRuns in a one-hour hardware run. Report maxima and thermal/load conditions; extend to an eight-hour soak before v1.0.
- Measure total latency (graph + queues + filters + backend + device), not merely requested buffer size. Native DSD, DoP, and PCM paths each get separate hardware records.

## 10. Ordered implementation milestones

Each milestone finishes with a reproducible artifact and an exit gate. Do not promote a quality profile or transport path on the basis of planned numbers.

### M0 — Establish foundations and retire transport uncertainty

**Work:**
- Create Cargo workspace, stable toolchain/MSRV, formatting/lint/test CI, license policy, and architecture decision records.
- Define typed rate/format/packing contracts and configuration schema skeleton.
- Prototype exact ALSA native DSD and DoP output using known vectors/idle streams on a named DAC.
- Prototype PipeWire PCM sink plus native SPA DSD negotiation; record actual capabilities and binding gaps.
- Decide the initial output route and timing master; record how WirePlumber releases the exclusive device.

**Exit:** checked packing vectors, documented format/rate units, hardware/version matrix, and ADR choosing the initial bridge/native route. If native PipeWire DSD fails, proceed with the PCM-input/direct-ALSA route and retain the findings.

### M1 — Reference math and profile qualification tools (depends on M0 types)

**Work:**
- Implement `f64` SRC/reference pipeline and a state-space DSM simulator.
- Reproduce a fifth-order candidate from documented design tooling; add manifest generation.
- Build independent spectral/decimation measurement scripts and stress stimuli.
- Establish per-rate filter and DSM thresholds, reference-level mapping, and recovery protocol.

**Exit:** generated coefficient artifacts and reports; at least one DSD rate/profile meets its frozen numerical and stability requirements; all M1 results reproducible from a command.

### M2 — Offline vertical slice (depends on M1)

**Work:**
- Implement WAV/FLAC input, PCM WAV/DSF output, exact packers, and raw stream mode.
- Implement `convert`, `benchmark`, profile listing, schema validation, and deterministic seed handling.
- Add seek/end-of-stream policies, duration accounting, two-pass gain analysis, metadata basics, and machine-readable reports.
- Qualify DSD64/128/256 profiles separately; expose only those that pass.

**Exit:** WAV/FLAC → DSF and PCM outputs validate independently; raw DoP byte counts/markers are correct; varied block sizes give the declared equivalent output; unsupported combinations fail clearly. Publish offline alpha.

### M3 — Real-time engine and PipeWire PCM integration (depends on M2)

**Work:**
- Implement prepared plans, bounded queues, lifecycle/reload logic, telemetry, and control socket.
- Register the desktop virtual sink and a PCM output stream; wire volume/mute and exclude routing loops.
- Implement latency reporting, device reconnect, underrun handling, and simulated clock-drift tests.
- Implement `run`, `status`, `doctor`, and user service.

**Exit:** private-session integration tests pass; one-hour PCM hardware run meets declared deadlines; restart/unplug/reload tests recover without stale-format buffers.

### M4 — DSD bridge and ALSA player integration (depends on M3 and qualified M0 transport)

**Work:**
- Connect PipeWire PCM input to direct ALSA native DSD/confirmed DoP output.
- Implement adaptive PCM clock matching, device-specific negotiation, idle stream, and verified fault recovery.
- Add ALSA loopback input and MPD examples with explicit rate-reopen behavior.
- If qualified, expose native PipeWire DSD as an additional backend path with its own tests.

**Exit:** at least one native-DSD and one confirmed-DoP hardware path documented (possibly the same DAC); drifting-clock and rate-switch tests pass; DSD hardware soak meets the declared deadline/XRun target. Publish Linux beta.

### M5 — Configuration and desktop experience (depends on M4)

**Work:**
- Implement setup TUI, stable device records, preset/rule selection, `config explain`, and guided benchmark recommendations.
- Add source-family/fixed-rate choices, controlled fallback, gapless/keep-open behavior, and routing examples for MPD/PipeWire applications.
- Publish tested schema/examples/manpage, install/uninstall steps, service integration, and device troubleshooting.

**Exit:** on a documented supported setup, a new user can select the DAC, save a passing preset, and play audio through the desktop sink without hand-editing DSP coefficients; every shipped example validates and unsupported modes explain why.

### M6 — Performance qualification and v1.0 (depends on M5)

**Work:**
- Profile hotspots; add SIMD/mixed precision/channel workers only where measurements justify them.
- Run regression spectra, fault injection, cross-platform core tests, and extended hardware soaks.
- Publish measured CPU/latency/quality tradeoffs for balanced/reference presets and a reproducible release bundle.

**Exit:** all advertised profiles and backends have matching reports, eight-hour baseline soak passes, packaging is reproducible, and no experimental path is presented as a verified default.

### M7 — Extensions after v1.0

Prioritize DSD passthrough/remodulation, optional PCM DSP, additional verified DACs/architectures, multichannel, web UI, and higher rates/topologies according to measured user value. Each new signal path reuses the same transport and profile qualification gates.

## 11. Immediate backlog and open decisions

### First implementation tickets, in order

1. **ADR-001: units and representations.** Define PCM/DSD/transport rate types, normalized amplitude mapping, channel layout, and packing reference vectors — including distinct golden vectors for DoP `s32le`/`s24le`/`s24_3le` container alignments.
2. **ADR-002: PipeWire/ALSA feasibility.** Name test DAC/CPU and Linux/PipeWire/WirePlumber versions; prove exact output paths and record capability evidence.
3. **ADR-003: clock ownership.** Diagram desktop bridge pacing, timestamps, queue feedback, and the location of adaptive SRC; simulate drift before live DSD.
4. **Schema v1.** Turn this document's TOML examples into fixtures, implement tagged types and validation, and document defaults/precedence.
5. **Reference harness.** Generate reproducible stimuli and compare independent state-space/decimation results.
6. **First qualified profile.** Generate coefficients, freeze measured limits/thresholds, and publish a manifest/report before optimizing its loop.
7. **First useful command.** Complete `audampler convert -i input.wav -o output.dsf -c offline-dsf.toml` end-to-end.

### Decisions to resolve through evidence

| Decision | Resolution artifact |
|---|---|
| Baseline CPU and supported DACs | Hardware matrix before performance promises |
| Native PipeWire DSD practicality | M0 negotiation/transport report and minimum supported versions |
| Default filter precision and threading | M1 quality baseline plus M6 measured speed/error comparison |
| Default DSD rate/profile | Qualified quality/ultrasonic/CPU/headroom tradeoff, not highest available rate |
| DAC DSD reference level and mute behavior | Device record and level/transition measurements |
| Large-file/container library choices | M2 format fixtures, RF64/seekability requirements, dependency review |

## 12. Technical references

Primary references consulted while revising this plan; record exact versions again during implementation because rolling API documentation changes.

1. [PipeWire SPA DSD structure and rate/interleave definitions](https://docs.pipewire.org/dsd_8h_source.html).
2. [PipeWire audio architecture](https://docs.pipewire.org/devel/page_audio.html) and [node, adapter, resampling, and ALSA properties](https://docs.pipewire.org/page_man_pipewire-props_7.html).
3. [DoP open standard v1.1](https://dsd-guide.com/dop-open-standard): payload, marker alternation, rate requirements, and detection behavior.
4. [Sony DSF specification v1.01](https://dsd-guide.com/sites/default/files/white-papers/DSFFileFormatSpec_E.pdf): chunks, channel blocks, bit order, sample counts, and padding.
5. [Schreier-style NTF synthesis documentation](https://pydsm.readthedocs.io/latest/generated/pydsm.NTFdesign.delsig.ntf_schreier.html) and [Schreier/Caldwell lectures on NTF optimization and Lee's criterion](https://www.individual.utoronto.ca/schreier/lectures/5-6.pdf).
6. [`foo_dsd_processor` project documentation](https://sourceforge.net/projects/sacddecoder/files/foo_dsd_processor/): product/workflow reference, not a source of assumed quality guarantees or reusable code licensing.
