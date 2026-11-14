# JadeAudio JA11

## Device Identification
- **Vendor / Model:** JadeAudio (FiiO) JA11 USB-C Dongle DAC
- **USB Vendor / Product ID:** `2972:0102`
- **ALSA Card Name:** `JA11` (`hw:CARD=JA11,DEV=0`)
- **PipeWire Target Node:** `alsa_output.usb-FIIO_JadeAudio_JA11-00.analog-stereo`

## Supported ALSA Formats & Transports
- **Pure PCM Endpoints (No Native `DSD_U32` Altsetting):**
  - **Formats:** `S16_LE`, `S24_3LE` (packed 3-byte 24-bit), `S32_LE`
  - **Sample Rates:** `44.1k`, `48k`, `88.2k`, `96k`, `176.4k`, `192k`, `352.8k`, `384k Hz`
- **DSD over PCM (DoP v1.1) Capabilities:**
  - **DoP64:** `2,822,400 Hz` DSD bit clock carried over `176,400 Hz` stereo PCM frames (`S24_3LE` only)
  - **DoP128:** `5,644,800 Hz` DSD bit clock carried over `352,800 Hz` stereo PCM frames (`S24_3LE` only)

## Known Issue

DoP over `S32_LE` altset 3 is true 32-bit PCM (`Bits: 32`) in `/proc/asound/card*/stream0`. DoP sent there is not detected and plays as PCM noise. The same `s32le` DoP stream plays correctly on the FiiO K17. Audigma decided to have `dop_sample_format = "auto"` prioritize `S24_3LE` before `S32_LE` when device reports support for both, analogous to `mpd`.
