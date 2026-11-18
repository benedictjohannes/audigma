# PipeWire Backend

PipeWire is audigma's live input (the virtual sink applications play into) and one of its two live outputs (`[output] backend = "pipewire"`, PCM only). Platform-neutral behavior is in [Design.md](../../Design.md).

## 1. Virtual Sink (input)

- **Node:** `media.class = "Audio/Sink"`, named by `[input] node_name`, or `audigma_sink_<instance>` when unset or empty. It carries the node property `audigma.sink = "true"` so probes (including `audigma probe --backend pipewire`) can skip it.
- **Channels:** `audio.channels` and `audio.position` are the resolved `channel_positions` (Design.md §3.4).
- **Clock: the sink is its own driver** (`node.driver = "true"`). A graph cycle is triggered only when the sink's queue drops below the preroll cushion (`[input] buffer_ms`), so the producer is paced by audigma's consumption and follows the output device's clock. The sink is therefore a *demand-driven* input (Design.md §1.5): no ASRC is applied to it, and `drift_correction = "on"` only observes.
- **Cycle size:** `[input] quantum_frames` (512 or 1024 frames at 44.1/48 kHz, scaled with the rate) sets the graph quantum requested by the sink (`node.latency`).
- **Rates (`[input] rate_mode = "dynamic"`):** the sink advertises `audio.allowed-rates` from `[input] allowed_rates` (e.g. `[ 44100 48000 88200 96000 176400 192000 352800 384000 ]`) and offers every allowed rate in its format enumeration. When an application starts playback at another rate, PipeWire renegotiates the format and the daemon switches rate without restarting (Design.md §3.2).
- **DSD input (`[input] allow_dsd = true`):** the sink also offers SPA `audio/dsd`, whose rate is in bytes per second per channel (Design.md §1.1). Native DSD streams have no volume properties.

## 2. Volume Synchronisation

audigma owns the user volume (Design.md §1.8); the desktop slider and mute of the virtual sink are synchronised both ways with it.

- **Into PipeWire:** the daemon seeds the stream's `channelVolumes` (PipeWire's cubic curve; master `volume` pinned to 1.0) and `mute` when the sink is created, and pushes later changes made through IPC or the tray.
- **Handshake:** every streaming run (first start, rate switch, resume, sink re-registration) begins with a handshake: PipeWire reports are discarded until one confirms the seeded state, or 1 s passes. This keeps the stream's default properties (100 %, unmuted) and any value restored by the session manager from overwriting a restored volume.
- **From PipeWire:** afterwards, a report equal (within 0.25 % on the stream's curve) to the stream's known state or to one of audigma's in-flight writes is an echo and ignored, so a volume set in dB never flips to percent. Only a genuinely different report updates the volume (as percent, like any desktop slider).
- A stream that rejects the write (e.g. native DSD, which has no volume properties) keeps the push queued until it is accepted.
- With `volume_control = "none"` nothing is pushed or ingested.

## 3. Output Stream (`[output] backend = "pipewire"`)

- **Node:** `node.name = "audigma_pcm_out_<instance>"`. The stream always carries unquantized `f32`; `target.sample_format` is ignored (with a warning when set).
- **Graph rate:** the stream requests the graph run at the target rate (`node.rate = 1/<rate>`). PipeWire honours this only if the rate is in `clock.allowed-rates` and no other stream holds the graph at a different rate; otherwise its adapter resamples the stream to the graph rate. The output is rate-matched by the graph, so no ASRC is applied.
- **Graph quantum:** `[output] quantum_frames` (512 or 1024 at 44.1/48 kHz, scaled with the rate).
- **Target pinning:** with `output.target_node` set the stream carries `target.object = <target_node>`, `node.dont-fallback = true` and `node.dont-move = true`, so it neither falls back to the default sink when the target is missing nor follows later default-sink changes.
- **No output loop:** an unpinned (or unmatched) output stream is linked by WirePlumber to the default sink, which can be audigma's own virtual sink: audio then recirculates inside audigma and nothing reaches a device (silent playback; level grows at about unity gain). `audigma run` warns at startup when `target_node` is unset, and while running the daemon checks the graph links about every 5 s, logging `output_loop` (ERROR) when the output stream is linked into audigma's own sink and `output_loop_cleared` when it clears.

## 4. Device Wait for a PipeWire Output

With `output.target_node` set, that sink is the output device for `device_wait_ms`, `device_wait_expiry` and `keep_audio_sink_ms` (Design.md §1.7, §4.5). Its arrival and removal are tracked in the PipeWire registry by `node.name`: removal fails a blocked output write at once and the daemon enters `WaitingForDevice`; the sink's reappearance ends the wait. A PipeWire output without `target_node` plays into the default sink and has nothing to wait for. Device reservation does not apply to PipeWire outputs.
