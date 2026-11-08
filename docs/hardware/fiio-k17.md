# FiiO K17 — Reference DAC Hardware Profile

> **Workspace Reference:** Referenced by [`Design.md`](../../Design.md).

## 1. Device Identification
- **Vendor / Model:** FiiO K17 Desktop DAC & Headphone Amplifier
- **USB Vendor / Product ID:** `2972:0107`
- **ALSA Card Name:** `K17` (`hw:CARD=K17,DEV=0`)
- **PipeWire Target Node:** `alsa_output.usb-GuangZhou_FiiO_Electronics_Co._Ltd_FIIO_K17-00.analog-stereo`

## 2. Supported ALSA Formats & Transports
- **Native DSD (`DSD_U32_BE`):**
  - **Bit Order:** `bitrev = 0` (MSB-first within each byte and 32-bit word), `DOP = 0`.
  - **Supported DSD Rates (`44.1 kHz` family):**
    - `DSD64`: `2,822,400 Hz` bit clock (`88,200 Hz` `DSD_U32_BE` ALSA container frame rate)
    - `DSD128`: `5,644,800 Hz` bit clock (`176,400 Hz` `DSD_U32_BE` ALSA container frame rate)
    - `DSD256`: `11,289,600 Hz` bit clock (`352,800 Hz` `DSD_U32_BE` ALSA container frame rate)
    - `DSD512`: `22,579,200 Hz` bit clock (`705,600 Hz` `DSD_U32_BE` ALSA container frame rate)
  - **Supported DSD Rates (`48 kHz` family):**
    - Up to `768,000 Hz` `DSD_U32_BE` frame rate (`24,576,000 Hz` bit clock).
- **Multi-Bit PCM:**
  - **Formats:** `S16_LE`, `S32_LE`
  - **Sample Rates:** `44.1k`, `48k`, `88.2k`, `96k`, `176.4k`, `192k`, `352.8k`, `384k`, `705.6k`, `768k Hz`
  - **DoP Support:** Supports DoP64 (`176.4 kHz`), DoP128 (`352.8 kHz`), and DoP256 (`705.6 kHz`) encapsulated in `S32_LE` (`s32le` MSB-justified `[0x00, LO, HI, MM]`).

## 3. Operational Notes & Invariants
- Often actively used by `mpd` or PipeWire/WirePlumber on the host desktop; never assume exclusive ALSA availability without consulting the user first (see `AGENTS.md`).
- Supports dynamic D-Bus device reservation (`org.freedesktop.ReserveDevice1`) when WirePlumber manages the session.
