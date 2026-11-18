# Roadmap

Planned behavior that is not implemented yet. `Design.md` describes only what audigma does today; an item moves there when it ships. Scope of each item is to be re-agreed before it is built.

## 1. `audigma doctor [--fix] [--json]`

Inspects the host audio stack and diagnoses why audigma cannot run as configured. Diagnostics sit behind a platform abstraction, so OS-specific checks stay out of the portable crates. Planned Linux checks:

1. **ALSA device lockouts:** finds the holders of `/dev/snd/pcmC*D*p`. If PipeWire/WirePlumber holds the device, `--fix` can switch the card profile off (`wpctl set-profile <id> 0`) or write a persistent WirePlumber drop-in under `~/.config/wireplumber/`. If another user or daemon holds it (such as MPD), `--fix` asks before terminating the process, escalating with `sudo` when needed, or prints the exact command.
2. **PipeWire multi-rate configuration:** with a PipeWire virtual sink and `rate_family = "source"`, warns when `default.clock.allowed-rates` pins the graph to a single rate (e.g. `48000`) and offers to install a multi-rate drop-in.
3. **Real-time scheduling limits:** checks `RLIMIT_RTPRIO` and RTKit availability for the loop, PipeWire and DSP threads. (The daemon already reports the outcome per thread as `rt_priority` events at startup.)

## 2. Windows Support

Brings audigma's live daemon and audio pipeline to Windows desktop environments.

1. **Output backends:**
   - **WASAPI exclusive mode:** bit-perfect high-rate PCM and DoP v1.1 output (`s32le` / `s24_3le`), bypassing the Windows audio engine's shared-mode mixer.
   - **ASIO:** direct access to vendor ASIO drivers for native DSD (ASIO's own DSD sample formats, mapped onto audigma's native DSD transport) without DoP overhead. The ASIO SDK is not under a permissive licence (Steinberg offers it under GPLv3 or a proprietary agreement), which has to be reconciled with audigma's MIT/Apache-2.0 licensing before this ships. If it cannot be, WASAPI with DoP stays the supported path.
2. **Virtual sink (input):** capture from an already installed virtual audio device (e.g. VB-Audio Cable) so desktop applications can route into audigma. Shipping audigma's own virtual audio driver would mean writing and signing a Windows driver, which is a separate project and out of scope here.
3. **Platform integration:** a notification-area (tray) icon with volume and mute controls, and MMCSS thread scheduling for the real-time DSP workers.

## 3. macOS Support

Brings audigma's live daemon and audio pipeline to macOS.

1. **Output backend:** CoreAudio output with exclusive device access (hog mode, `kAudioDevicePropertyHogMode`) to bypass the system mixer. DSD output is DoP v1.1 (`s32le` / `s24_3le`); native DSD is added only if CoreAudio turns out to expose it for the target devices.
2. **Virtual sink (input):** capture from an installed loopback device (e.g. BlackHole) so applications can route into audigma. An audigma-owned Audio Server Plug-in is a possible later step.
3. **Platform integration:** a menu bar item with volume and mute controls, and CoreAudio real-time thread scheduling.

## 4. Web UI Distribution & Release Packaging

Automates release builds so published binaries embed the full web configurator.

1. **Web UI build:** a GitHub Actions step runs `bun install --frozen-lockfile && bun run build` in `crates/audigma-configurator/web`, producing `web/dist/index.html`.
2. **Enforced embedding:** release builds set `AUDIGMA_REQUIRE_UI=1`, so the build fails instead of silently falling back to the lightweight page when the bundle is missing.
3. **Release artifacts:** pushing a version tag builds and publishes `.tar.gz` archives for Linux (binary, man pages, shell completions and the license texts). Windows and macOS archives follow once those ports exist.

## 5. `audigma-engine` Portability

Remove the last OS-specific code from `crates/audigma-engine`, so it is portable like the other core crates (`Design.md §2`). This is the prerequisite for the Windows and macOS ports above.

1. **Worker wake-up:** put the worker pool's wait/wake barrier behind a small platform-neutral interface. Linux keeps the futex (the zero-allocation, lock-free real-time path); other platforms bring their own address-wait primitive. A portable fallback built on standard-library locks is acceptable for tests only, since it does not meet the real-time requirement of `Design.md §3.4`.
2. **Device-loss classification:** move the errno classification that decides whether an output error means device loss into the backends (`audigma-io`), so the engine only sees an abstract error kind.
