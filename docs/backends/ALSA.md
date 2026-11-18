# ALSA Backend

ALSA is audigma's exclusive output (`[output] backend = "alsa"`, `output.device` e.g. `hw:CARD=K17,DEV=0`) for high-rate PCM and DSD (native or DoP). There is no ALSA input backend. Platform-neutral behavior is in [Design.md](../../Design.md).

## 1. Device Reservation (`[output] reserve_device`)

To avoid `Device or resource busy` lockouts without permanent desktop configuration changes, audigma acquires the DAC through the standard D-Bus device reservation protocol (`org.freedesktop.ReserveDevice1`), enabled by default (`reserve_device = true`). WirePlumber releases the device while audigma holds it and restores normal desktop routing when audigma stops, pauses or loses the device. For headless setups without a desktop sound server, set `reserve_device = false`.

## 2. Device Loss & Arrival

- **Loss** is detected by the output write path: a write returning `ENODEV`, `EBADFD` or `EIO` moves the daemon to `WaitingForDevice` at once.
- **Arrival** is detected by an `inotify` watch on `/dev/snd` (node creation), event-driven: no directory scanning, no busy-spin. On arrival audigma re-acquires the reservation and re-opens the device (Design.md §4.5).

## 3. Hardware Volume

With `volume_control = "hardware-software-fallback"` or `"hardware"`, audigma looks for a hardware mixer element on the target card (e.g. `Master`, `PCM`, `Playback`) and applies volume there, keeping the digital stream at full scale. Without one, `hardware-software-fallback` uses software volume and `hardware` refuses digital scaling with an error/warning (Design.md §1.8).

## 4. Native DSD

Native DSD opens ALSA `DSD_U32_BE` (or `DSD_U32_LE`) at the transport frame rate $f_{\text{bit}} / 32$ (Design.md §5.1, §5.2).

## 5. DoP Word Negotiation (`output.dop_sample_format = "auto"`)

Each DoP word opens the ALSA format whose bit placement it matches: `s32le` → `S32_LE`, `s24le` → `S24_LE`, `s24_3le` → `S24_3LE` (layouts in Design.md §5.2).

- With `auto`, the words are tried in the order `s24_3le`, `s24le`, `s32le` (format, channel count and exact DoP frame rate each), and the first the device accepts is opened. DoP v1.1 is defined on 24-bit samples, and a DAC that also offers `S32_LE` may detect DoP only on its 24-bit format, so the 24-bit words lead and `s32le` is the fallback for DACs with no 24-bit format. MPD uses the same rule.
  - FiiO K17 (only `S16_LE`/`S32_LE`): DoP over `S32_LE` plays.
  - JadeAudio JA11: opens `S32_LE` as true 32-bit PCM and plays DoP in it as noise, so it needs a 24-bit word.
  - USB DACs never offer `S24_LE`, so the order between the two 24-bit words does not matter for them.
- A pinned word is tried alone and fails the open if refused (no fallback).
- Through `plughw:` or `default` every word is accepted, so `auto` opens `S24_3LE`. The plug's conversions between these words only add or drop the zero pad byte, so the DoP word stays bit-exact; whether the DAC then detects DoP depends on the format the plug opens underneath.
- The word is negotiated on every device open, and the stream is always packed in the word that was opened (Design.md §5.2).

## 6. Buffering

`[output] buffer_ms` (device buffer, protects the DAC against scheduling jitter) and `[output] period_ms` set the ALSA buffer and period sizes.
