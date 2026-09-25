# `audampler` System Architecture & Execution Plan (v1.2)

`audampler` is a modular, high-performance audio upsampler and delta-sigma modulation daemon and conversion utility written in Rust. It offloads compute-heavy digital filtering, polyphase interpolation, and high-order delta-sigma modulation (PCM $\to$ PCM, PCM $\to$ DSD) from DAC silicon to host compute.

---

## 1. Mathematical Primitives & Safety Constraints

### 1.1 Sample Rate Converter & Filter Cascade (`audampler-core::resample`)

* **Multi-stage Architecture:**
* Power-of-two stages ($2\times, 4\times, 8\times, \dots, 64\times$): Cascading polyphase half-band symmetric FIR filters.
* Rational/Arbitrary base ratios: Windowed-sinc polyphase branch interpolator.

* **Filter Specifications:**
* Stopband attenuation: $\ge 140\text{ dB}$ across all decimation/interpolation stages.
* Transition width: $0.05$ to $0.08$ normalized to input Nyquist.
* Phase transforms:
* `linear`: Symmetrical linear phase (zero phase distortion).
* `minimum`: Minimum phase derived via Hilbert transform / complex cepstrum of the linear prototype (zero pre-ringing).
* `apodizing`: Linear or minimum-phase with transition band shifted down by $2\text{ kHz} - 4\text{ kHz}$ below source Nyquist to suppress upstream ADC mastering filter ringing.

* **Vectorization & Precision Split:**
* `audampler-core::resample` (polyphase FIR): Pure `f32` — FIR convolution is linear, and single-precision (24-bit mantissa, $\approx 144-150\text{ dB}$ SNR) covers 24-bit audio with zero audible gain from `f64`. Vectorize across lanes (`wide::f32x8` / AVX2, `core::simd`) over interleaved or planar chunks.
* `audampler-core::modulate` (DSM loop): Scalar `f64` (double precision) for integrator accumulator states and feedback math. At OSR $256\times$/$512\times$, `f32` suffers catastrophic cancellation and accumulator truncation drift over millions of iterations/sec, raising the baseband floor and triggering spurious limit cycles. DSM is inherently sequential within a channel (sample $N$ depends on quantizer decision $N-1$), so SIMD cannot parallelize time anyway — channel parallelism absorbs the load.

### 1.2 Delta-Sigma Modulators (`audampler-core::modulate`)

* **Supported Topologies:**
* **CRFB:** Cascade of Resonators with Distributed Feedback.
* **CIFB:** Cascade of Integrators with Distributed Feedback.
* **CIFF:** Cascade of Integrators with Distributed FeedForward.

* **Noise Transfer Function (NTF) & Stability Constraints:**
* **Optimal Zero Placement:** NTF zeros are distributed across the entire audio baseband ($0\text{ Hz}$ to $20\text{ kHz}$ / $24\text{ kHz}$) using Chebyshev/Butterworth optimization (Schreier toolbox formulation), placing complex conjugate zero pairs along the unit circle to minimize total in-band quantization noise.
* **Lee Stability Criterion:** Maximum out-of-band NTF gain ($\Vert{}H_\infty\Vert{}$) is strictly bound:
* 5th-order: $\Vert{}H_\infty\Vert{} \le 1.5 - 1.7$
* 7th-order: $\Vert{}H_\infty\Vert{} \le 1.3 - 1.5$

* **Maximum Stable Amplitude (MSA):** Modulator profiles explicitly declare their theoretical and verified MSA:
* 5th-order (`crfb5-notched`): MSA $\approx -2.5\text{ dBFS}$ (requires minimum $3.0\text{ dB}$ input attenuation).
* 7th-order (`crfb7-steep`): MSA $\approx -5.2\text{ dBFS}$ (requires minimum $6.0\text{ dB}$ input attenuation).

* **Nonlinear Protection & Conditioning:**
* **Integrator State Clamping:** Every accumulator state register is soft-clamped to a profile-defined threshold ($V_{\max} = 1.5$ to $2.5$) using branchless saturation arithmetic. Coefficients remain strictly static at all times; no dynamic retuning is performed:

$$v_{k} = \max(-V_{\max}, \min(V_{\max}, v_{k}))$$

* **Quantizer Dither Injection:** Low-amplitude TPDF dither ($\approx 0.05$ to $0.1\text{ LSB}$ equivalent) is injected at the input of the final integrator before the 1-bit comparator node to disperse limit cycles and idle tones during near-zero input signals without degrading in-band dynamic range.
* **Overload Detection & Soft-Mute Recovery (State Leaking):** If any integrator stays pinned against $V_{\max}$ for $\ge 32$ consecutive clock cycles, the overload flag trips:
1. Force modulator input to zero and hold loop coefficients unchanged.
2. Over the next $64$ samples, apply exponential state decay (leak factor $\lambda = 0.90$ per clock, $0.9^{64} \approx 0.001$) to bleed stored energy out of the integrator registers. This $64$-clock leak ($\approx 5.6\,\mu\text{s}$ at DSD256) executes inside the output mute window below.
3. Concurrently initiate a $2\text{ ms}$ cosine-windowed mute on the output ring buffer, then ramp back up only once all states fall below $0.5 \times V_{\max}$ (hysteresis) and input is below MSA. This eliminates DC thumps and protects downstream gear without altering loop transfer functions or introducing zipper noise.

---

## 2. CLI Interface & Manpage Specification

```man
AUDAMPLER(1)                   User Commands                  AUDAMPLER(1)

NAME
     audampler - high-performance audio upsampler and delta-sigma modulator

SYNOPSIS
     audampler --daemon [OPTIONS]
     audampler --convert -i <FILE> -o <FILE> [OPTIONS]
     audampler --benchmark -i <FILE> [OPTIONS]
     audampler --configure [--web | --tui] [--out <PATH>]

DESCRIPTION
     audampler executes real-time and offline audio upsampling, noise
     shaping, and delta-sigma modulation.

OPTIONS
     -c, --config <PATH>
          Path to configuration TOML file. Defaults to
          $XDG_CONFIG_HOME/audampler/config.toml.

     -d, --daemon
          Run as a persistent real-time daemon processing audio from
          ALSA loopback, PipeWire, or named pipe to an audio sink.

     --convert
          Batch offline file conversion mode. Converts multi-bit PCM to
          high-rate PCM (.wav) or native DSD (.dsf). Bypasses audio clocks.

     -i, --input <PATH>
          Input audio path for conversion or benchmark mode. Accepts '-'
          for raw PCM ingestion via standard input.

     -o, --output <PATH>
          Output audio path for conversion mode (.wav, .dsf). Accepts '-'
          for standard output.

     --benchmark
          Executes the complete DSP pipeline on the input file as fast as
          possible. Outputs processing multiplier (x realtime), stage CPU
          ratios (resampler vs. modulator vs. packaging), and peak RAM.

     --configure
          Run interactive hardware probe and setup. Defaults to TUI.

     --web
          When paired with --configure, launches an ephemeral HTTP server
          on a dynamic localhost port, displays the configuration URL,
          and terminates automatically upon saving.

     --out <PATH>
          Target path when saving configuration via --configure.

```

---

## 3. Configuration Specification & Concrete Examples

The configuration file is validated against an explicit schema containing six declared root tables:

* `[audio.io]`
* `[audio.routing]`
* `[audio.stdio_params]` (optional, active when `backend = "stdio"`)
* `[gain]`
* `[resampler]`
* `[modulator]` (optional, active when `target.format = "dsd"`)
* `[target]`

### 3.1 Headless MPD Server via ALSA Loopback (`daemon-alsa.toml`)

*MPD outputs to `hw:Loopback,0,0`. `audampler` captures from `hw:Loopback,1,0` and feeds a USB DAC on `hw:Audio,0`.*

```toml
[audio.io]
backend = "alsa"
input_device = "hw:Loopback,1,0"
output_device = "hw:Audio,0"

# Buffer units explicitly defined at hardware output rate:
# At DSD256 (11.2896 MHz bit clock = 352,800 32-bit words/sec/ch):
# 65536 hardware frames = ~185 ms latency buffer
buffer_frames_hardware = 65536
period_frames_hardware = 8192

[audio.routing]
# Automatic passthrough for streams already in DSD format
dsd_passthrough = true
# Mute period (in milliseconds) when switching sample rates to eliminate DAC relay/transient clicks
rate_change_mute_ms = 40

[gain]
# Enforce -3.0 dBFS input scaling to remain strictly below the MSA for CRFB-5
attenuation_db = -3.0

[resampler]
family = "polyphase-fir"
phase = "minimum"              # "linear" | "minimum" | "apodizing"
stopband_attenuation_db = 140.0
transition_width = 0.08

[modulator]
topology = "crfb"
order = 5
profile = "crfb5-notched"
quantizer_bits = 1
dither = "tpdf-internal"
dither_amplitude_lsb = 0.08
integrator_clamp = 2.0

[target]
format = "dsd"
# Rate resolves adaptively according to input family:
# 44.1 kHz family (44.1k, 88.2k, 176.4k) -> 256 * 44.1k = 11.2896 MHz
# 48.0 kHz family (48k, 96k, 192k)       -> 256 * 48.0k = 12.2880 MHz
rate_family = "DSD256"
framing = "native"             # "native" (DSD_U32_BE) | "dop" (0x05/0xFA in PCM)

```

---

### 3.2 System-Wide PipeWire DSP Filter Node (`daemon-pipewire.toml`)

*Registers as a virtual sink in the PipeWire graph (`audampler-sink`). Audio sent here is upsampled and modulated directly before egress to the downstream DAC sink node.*

```toml
[audio.io]
backend = "pipewire"
# Name of the virtual sink node created in PipeWire graph
pw_node_name = "audampler_dsp"
# Downstream target sink (empty string = follow default PipeWire audio sink)
output_target_node = "alsa_output.usb-DAC_Audio-00.analog-stereo"

[audio.routing]
dsd_passthrough = true
rate_change_mute_ms = 30

[gain]
attenuation_db = -3.5

[resampler]
family = "polyphase-fir"
phase = "apodizing"
stopband_attenuation_db = 140.0
transition_width = 0.07

[modulator]
topology = "crfb"
order = 5
profile = "crfb5-notched"
quantizer_bits = 1
dither = "tpdf-internal"
dither_amplitude_lsb = 0.08
integrator_clamp = 2.0

[target]
format = "dsd"
rate_family = "DSD128"
# PipeWire graph links pass DoP frames transparently inside high-rate PCM
framing = "dop"

```

---

### 3.3 High-Rate PCM-to-PCM Upsampling (`pcm-upsample.toml`)

*No delta-sigma modulator engaged; upsamples low-rate PCM to high-rate 768 kHz PCM with linear-phase noise-shaped dither for R-2R ladder or NOS DACs.*

```toml
[audio.io]
backend = "alsa"
input_device = "hw:Loopback,1,0"
output_device = "hw:DAC,0"
buffer_frames_hardware = 16384
period_frames_hardware = 2048

[audio.routing]
dsd_passthrough = true
rate_change_mute_ms = 20

[gain]
# -2.0 dBFS headroom: modern 0 dBFS masters exhibit inter-sample peaks of
# +1.5 to +3.0 dBFS after reconstruction. Attenuation is applied *prior* to
# interpolation so FIR arithmetic never clips the output container.
attenuation_db = -2.0

[resampler]
family = "polyphase-fir"
phase = "linear"
stopband_attenuation_db = 150.0
transition_width = 0.05

# Modulator block omitted: target format is PCM
[target]
format = "pcm"
# Target sample rate multiplier relative to source base clock:
# 44.1k family -> 16x = 705.6 kHz | 48k family -> 16x = 768.0 kHz
pcm_rate_multiplier = 16
bit_depth = 32
dither = "tpdf-triangular"

```

*Note: in all daemon and conversion pipelines, the `[gain]` attenuation stage runs **prior** to interpolation so upsampling reconstruction (inter-sample peaks) never clips intermediate FIR arithmetic or the output container.*

---

### 3.4 Batch File Conversion: FLAC to DSF (`conversion-dsf.toml`)

```toml
[audio.io]
backend = "file"

[audio.routing]
dsd_passthrough = true

[gain]
# 7th-order CRFB requires steep attenuation to avoid exceeding its lower MSA
attenuation_db = -6.0

[resampler]
family = "polyphase-fir"
phase = "apodizing"
stopband_attenuation_db = 150.0
transition_width = 0.06

[modulator]
topology = "crfb"
order = 7
profile = "crfb7-steep"
quantizer_bits = 1
dither = "tpdf-internal"
dither_amplitude_lsb = 0.05
integrator_clamp = 1.8

[target]
format = "dsd"
rate_family = "DSD512"
framing = "native"

```

---

### 3.5 UNIX Pipe Streaming (`stdio-pipe.toml`)

*Headless CI / hardware-free testing: `cat track.raw | audampler -c stdio-pipe.toml -i - -o - | aplay`. Raw PCM in via stdin, modulated DoP out via stdout — no soundcard or `snd-aloop` required.*

```toml
[audio.io]
backend = "stdio"
input_device = "stdin"
output_device = "stdout"

[audio.stdio_params]
input_sample_rate = 44100
input_channels = 2
input_format = "S16_LE"

[audio.routing]
dsd_passthrough = true
rate_change_mute_ms = 20

[gain]
attenuation_db = -3.0

[resampler]
family = "polyphase-fir"
phase = "linear"
stopband_attenuation_db = 130.0
transition_width = 0.10

[modulator]
topology = "crfb"
order = 5
profile = "crfb5-notched"
quantizer_bits = 1
dither = "tpdf-internal"
dither_amplitude_lsb = 0.08
integrator_clamp = 2.0

[target]
format = "dsd"
rate_family = "DSD128"
framing = "dop"
```

---

## 4. Hardware Interrogation (`--configure`)

When running `audampler --configure`:

1. **ALSA Device Interrogation:**

* Query `snd_pcm_hw_params_test_format`:
* Native DSD formats: `SND_PCM_FORMAT_DSD_U32_BE`, `DSD_U32_LE`, `DSD_U8`.
* Multi-bit PCM formats: `S32_LE`, `S24_LE`, `S16_LE`.

* Query supported rates via `snd_pcm_hw_params_test_rate`:
* Scan 44.1k family: 44.1k, 88.2k, 176.4k, 352.8k, 705.6k, 1.4112M.
* Scan 48k family: 48k, 96k, 192k, 384k, 768k, 1.536M.

2. **DoP Compatibility Evaluation:**

* If native DSD is unsupported, verify whether PCM 176.4 kHz (DSD64), 352.8 kHz (DSD128), or 705.6 kHz (DSD256) are supported for DoP encapsulation.

3. **Live UI Enforcement:**

* In TUI/Web configuration, invalid options are disabled. If a DAC maxes out at 384 kHz PCM without native DSD, `DSD256` and `DSD512` options are greyed out, and framing locks to `dop`.

---

## 5. Threading Model, Channel Parallelism & Click Prevention

1. **Worker Threading Strategy:**

* The DSP processing pipeline operates across a thread pool using **channel-level parallelism**:
* Left channel and right channel FIR interpolations and modulator loops run independently on dedicated core threads. FIR stages use SIMD lanes (`wide::f32x8` / AVX2 via `core::simd`); modulator loops use scalar `f64` accumulators.
* The single-sample dependency in the delta-sigma modulator loop is sequential *within* a channel, but channels execute concurrently across CPU cores without lock contention.

2. **DoP Detection & Format Switching Safety:**

* Input streams are inspected frame-by-frame for DoP v1.1 header patterns (`0x05` / `0xFA` in bits 23:16).
* When switching between PCM and DSD/DoP, or when upstream rates change:
* A zero-signal fade-out (3–5 ms cosine window) executes on the output ring buffer.
* Hardware parameters re-negotiate.
* The output un-mutes after the configured `rate_change_mute_ms` to suppress relay clicks or DC-thumps.

---

## 6. Cargo Workspace Structure & Crate Responsibilities

```
crates/
├── audampler-core/       # Pure DSP algorithms (No IO, no ALSA/PW dependencies)
│   ├── src/resample/     # Polyphase FIR, cascading half-band filters, window generators
│   ├── src/modulate/     # CRFB/CIFB/CIFF difference loops, clamps, dither generators
│   └── src/dop/          # DoP v1.1 framer and parser
├── audampler-format/     # Audio format encoding/decoding
│   ├── src/wav.rs        # PCM WAV I/O (via hound)
│   └── src/dsf.rs        # Custom DSF reader & writer (ID3/DSD chunks, 4096-byte blocks)
├── audampler-io/         # System I/O and hardware interfaces
│   ├── src/alsa/         # ALSA hw_params query, loopback ingestion, direct egress
│   ├── src/pipewire/     # PipeWire SPA filter node implementation via pipewire-rs
│   └── src/ring_buffer/  # Real-time lock-free ring buffers (via rtrb)
├── audampler-config/     # TOML schema parsing, validation rules, Axum web/TUI configurator
└── audampler-cli/        # Binary entry point, CLI flags (clap), daemon and benchmark runners

```

---

## 7. Execution Milestones & Test Gates

### Milestone 1: Pure DSP Math Engine (`audampler-core`)

* **Deliverables:**
* Implement polyphase half-band filters and multi-stage upsamplers.
* Implement CRFB 5th-order (`crfb5-notched`) and 7th-order (`crfb7-steep`) difference equations with accumulator clamps and comparator dither.

* **Automated Test Gate:**
* **Stability & MSA Test:** Inject a 1 kHz sine wave sweeping amplitude from $-60\text{ dBFS}$ to $+2.0\text{ dBFS}$.
* Must remain numerically stable with no accumulators exceeding $V_{\max}$ and zero `NaN`/`Inf` occurrences up to the designated MSA ($-2.5\text{ dBFS}$ for CRFB-5, $-5.2\text{ dBFS}$ for CRFB-7).
* Above MSA, verify that integrator clamps bound the states and that the loop recovers cleanly when input returns below MSA.

* **Audio Band Dynamic Range Test (two-part protocol):** At $f_s = 11.2896\text{ MHz}$ (DSD256) a $65{,}536$-point FFT gives $\Delta f \approx 172.3\text{ Hz}$, collapsing $20\text{ Hz} - 100\text{ Hz}$ into less than one bin — statistically meaningless for SNR. Use instead:
1. **Direct High-Resolution Spectrum:** $1{,}048{,}576$-point FFT ($1\text{M}$ points, $\approx 93\text{ ms}$ at DSD256) via `rustfft`/`realfft` with a 7-term Blackman-Harris window and Welch averaging over 4 overlapping blocks. Yields $\Delta f \approx 10.76\text{ Hz}$ for resolution down to $20\text{ Hz}$ while displaying the ultrasonic noise-shaping shelf up to $5.64\text{ MHz}$. Use a coherent test tone (integer cycles per window, e.g. 1 kHz) to avoid leakage.
2. **Standard Reference Decimation (pass/fail gate):** Pass the modulated 1-bit stream through a high-precision digital decimation low-pass filter down to $48\text{ kHz}$ / $32$-bit float PCM, then compute standard AES17 THD+N / dynamic range across $20\text{ Hz} - 20\text{ kHz}$; verify SNR $> 120\text{ dB}$ for inputs at $-6\text{ dBFS}$.

### Milestone 2: Formats & Offline Engine (`audampler-format` & CLI)

* **Deliverables:**
* Hand-rolled DSF file parser and writer (handling 4096-byte blocks, DSD header chunks, channel bit-interleaving).
* WAV file I/O via `hound`.
* Implement `audampler --convert -i input.wav -o output.dsf`.
* Implement `audampler --benchmark -i input.wav`.

* **Automated Test Gate:**
* Verify conversion of standard 16/44.1 kHz WAV to DSD64/DSD128/DSD256 DSF. Validate header structure using `mediainfo` or `ffprobe`.
* Verify that `--benchmark` reports real-time factor, stage compute ratios, and zero runtime heap allocations in the hot loop.
* Verify headless UNIX pipe: feed raw `S16_LE` via `audampler -c stdio-pipe.toml -i - -o -` in CI without soundcard; assert exit 0, expected output byte count, and valid DoP `0x05`/`0xFA` markers.

### Milestone 3: Hardware Interrogation & Configurator (`audampler-config`)

* **Deliverables:**
* ALSA `hw_params` introspection module querying available cards, formats, and sample rates.
* Axum-based ephemeral web configurator binding to `127.0.0.1:0`, serving embedded UI assets, validating TOML configurations against probed hardware limits, and shutting down on save.

* **Automated Test Gate:**
* Unit test config validation: reject configurations requesting rates or DSD modes unsupported by the simulated hardware profile.

### Milestone 4: ALSA Real-time Daemon (`audampler-io::alsa`)

* **Deliverables:**
* Ingestion from ALSA loopback (`snd-aloop`) with dynamic rate tracking.
* Direct hardware egress via `alsa-rs` with lock-free decoupled ring buffers (`rtrb`).
* Click-free format/rate switching logic with soft mute windows.

* **Automated Test Gate:**
* Run headless daemon loop; verify zero XRuns across 1 hour of simulated multi-rate playback.

### Milestone 5: PipeWire Filter Node (`audampler-io::pipewire`)

* **Deliverables:**
* Implement PipeWire SPA node wrapper using `pipewire-rs`.
* Expose virtual sink (`audampler_dsp`) capable of receiving PCM and streaming modulated DoP/PCM to downstream PipeWire sink nodes.

* **Automated Test Gate:**
* Launch PipeWire test session; confirm node registration, format parameter negotiation, and audio flow via `pw-link`.
