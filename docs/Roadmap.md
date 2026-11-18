# Roadmap

Planned behavior that is not implemented yet. `Design.md` describes only what audigma does today; an item moves there when it ships. Scope of each item is to be re-agreed before it is built.

## 1. `audigma doctor [--fix] [--json]`

Inspects the host audio stack and diagnoses why audigma cannot run as configured. Diagnostics sit behind a platform abstraction, so OS-specific checks stay out of the portable crates. Planned Linux checks:

1. **ALSA device lockouts:** finds the holders of `/dev/snd/pcmC*D*p`. If PipeWire/WirePlumber holds the device, `--fix` can switch the card profile off (`wpctl set-profile <id> 0`) or write a persistent WirePlumber drop-in under `~/.config/wireplumber/`. If another user or daemon holds it (such as MPD), `--fix` asks before terminating the process, escalating with `sudo` when needed, or prints the exact command.
2. **PipeWire multi-rate configuration:** with a PipeWire virtual sink and `rate_family = "source"`, warns when `default.clock.allowed-rates` pins the graph to a single rate (e.g. `48000`) and offers to install a multi-rate drop-in.
3. **Real-time scheduling limits:** checks `RLIMIT_RTPRIO` and RTKit availability for the loop, PipeWire and DSP threads. (The daemon already reports the outcome per thread as `rt_priority` events at startup.)
