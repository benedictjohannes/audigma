# Telemetry & Log Events

What a running daemon reports through `audigma status` and which named log events it emits. Event names are an interface: scripts and users grep for them, so a rename is a breaking change to this document.

## 1. `audigma status [--watch] [--json]`

Bare `status` lists all live instances (Design.md §7). Per instance it reports:

- Input negotiated format/rate, PipeWire graph rate, and active channel positions.
- Output bit/sample/transport rate and active physical device/endpoint.
- Daemon and device state (`Running`, `WaitingForDevice`, `Paused`, ...) and `disconnect_duration_ms`. While paused only the state, uptime and volume are shown, with `pause_reason` (`Requested` or `DeviceWaitExpired`).
- Active volume mode (`hardware`, `software`, `none`) and current volume attenuation levels.
- Active filter (phase and apodization), profile hash, and configured gain breakdown (`preamp_db`, user volume, profile headroom).
- Measured true peaks and modulator integrator operating envelope.
- Latency breakdown (graph + queue + filter group delay + backend buffer).
- Input queue fill percentage, ASRC mode, applied correction and estimated clock drift ($\delta_{\text{ppm}}$).
- Hardware xrun counters, fault/recovery event counts, and per-worker execution and dispatch timing (mean, p50, p99, max, cumulative p99.9) with the stall cause.

## 2. Real-Time Health Events

The feed loop and DSP workers are timed continuously. *Worker execution* is the time a worker spends processing its tile (excluding wake-up and barrier wait); *dispatch* is the loop's view of the whole tile (wake-up plus barrier). In `dsp_threads = "inline"` mode there is a single timing entry. `[logging] rt_report_interval_ms` sets the cadence of the windowed events below.

- **`loop_health`:** per window (10 s by default), mean, p50, p99 and max per worker (`worker_exec_*`, `worker_p99_us`) and for dispatch (`dispatch_*`), plus cumulative p99.9.
- **`late_start` / `late_slice` / `late_end`:** a run of tiles whose dispatch exceeded the tile budget. `late_slice` carries the same timing fields plus `headroom_min_us` and `buffer_peak_us` (device-buffer headroom over the slice). It is logged at WARN only when headroom fell below half of the peak queued audio (or is unknown), otherwise at INFO; `late_start` is DEBUG.
- **`loop_stall`:** a stall of the feed loop, with `cause` one of `input_empty`, `dsp_slow`, `emit_slow`, `read_slow`, `prep_slow`, `unknown`.

## 3. Other Events

- **`rt_priority`:** outcome of a real-time scheduling request per thread at startup, and failures when PipeWire threads re-request RT on reconnect (Design.md §3.4).
- **`output_loop` / `output_loop_cleared`:** the PipeWire output stream is (no longer) linked into audigma's own virtual sink ([PipeWire.md](backends/PipeWire.md) §3).
- **`volume_persist`:** the volume state file could not be read, parsed or written (Design.md §6.3).

This is not yet a complete catalog: other events exist in the code (daemon lifecycle, device, volume, rate switch, tray) without documented fields.
