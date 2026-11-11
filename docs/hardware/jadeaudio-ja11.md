# JadeAudio JA11 — Reference DAC Hardware Profile

> **Workspace Reference:** Referenced by [`Design.md`](../../Design.md).

## 1. Device Identification
- **Vendor / Model:** JadeAudio (FiiO) JA11 USB-C Dongle DAC
- **USB Vendor / Product ID:** `2972:0102`
- **ALSA Card Name:** `JA11` (`hw:CARD=JA11,DEV=0`)
- **PipeWire Target Node:** `alsa_output.usb-FIIO_JadeAudio_JA11-00.analog-stereo`

## 2. Supported ALSA Formats & Transports
- **Pure PCM Endpoints (No Native `DSD_U32` Altsetting):**
  - **Formats:** `S16_LE`, `S24_3LE` (packed 3-byte 24-bit), `S32_LE`
  - **Sample Rates:** `44.1k`, `48k`, `88.2k`, `96k`, `176.4k`, `192k`, `352.8k`, `384k Hz`
- **DSD over PCM (DoP v1.1) Capabilities:**
  - **DoP64:** `2,822,400 Hz` DSD bit clock carried over `176,400 Hz` stereo PCM frames (`S24_3LE` only)
  - **DoP128:** `5,644,800 Hz` DSD bit clock carried over `352,800 Hz` stereo PCM frames (`S24_3LE` only)
  - **No DoP over `S32_LE`:** the `S32_LE` altset (altset 3) is true 32-bit PCM (`Bits: 32` in `/proc/asound/card*/stream0`, against `Bits: 24` for `S24_3LE` on altset 2). DoP sent there is not detected and plays as PCM noise (verified 2026-11-11; the same `s32le` DoP stream plays correctly on the FiiO K17). `dop_sample_format = "auto"` picks `S24_3LE` here.

## 3. Operational Notes & Invariants
- Ideal secondary hardware test target for packed 24-bit DoP (`s24_3le`) when `FiiO K17` is busy playing music via MPD. Not a target for 32-bit DoP (`s32le`); use the K17 for that.
- Requires continuous `0x05` / `0xFA` marker alternation and `0x69` balanced DSD silence during start/stop transitions to avoid analog pops.
