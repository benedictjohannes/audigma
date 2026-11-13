//! ALSA hardware capability probing and device discovery (`Design.md §7`).
//!
//! Provides device enumeration, hardware parameter testing (`snd_pcm_hw_params_test_*`),
//! procfs stream descriptor inspection (`/proc/asound`), and detection of Native DSD
//! and DoP capability tiers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// Overall report of ALSA hardware probing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsaProbeReport {
    /// Discovered sound cards.
    pub cards: Vec<AlsaCardReport>,
}

/// Discovered sound card report.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsaCardReport {
    /// Card index (e.g. 0, 1, 2).
    pub index: i32,
    /// Card string ID (e.g. "K17", "Generic").
    pub id: String,
    /// Driver or hardware short name.
    pub name: String,
    /// Card description / longname.
    pub longname: String,
    /// USB vendor & product ID if applicable (e.g. "2972:0107").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usb_id: Option<String>,
    /// PCM playback devices found on this card.
    pub devices: Vec<AlsaDeviceReport>,
    /// Mixer elements with playback volume or switch (candidates for `output.mixer_control`).
    #[serde(default)]
    pub mixer_controls: Vec<MixerControlReport>,
}

/// A playback mixer element of a card.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MixerControlReport {
    /// Element name (e.g. "PCM", "Master"), the value for `output.mixer_control`.
    pub name: String,
    /// Element index (usually 0).
    pub index: u32,
    /// Has a playback volume.
    pub has_volume: bool,
    /// Has a playback switch (mute).
    pub has_switch: bool,
}

/// Lists the playback mixer elements of a card; empty when the mixer cannot be opened.
#[cfg(feature = "alsa")]
fn probe_mixer_controls(card_idx: i32) -> Vec<MixerControlReport> {
    use alsa::mixer::{Mixer, Selem};
    let Ok(mixer) = Mixer::new(&format!("hw:{card_idx}"), false) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for elem in mixer.iter() {
        let Some(selem) = Selem::new(elem) else {
            continue;
        };
        let (has_volume, has_switch) = (selem.has_playback_volume(), selem.has_playback_switch());
        if !has_volume && !has_switch {
            continue;
        }
        let id = selem.get_id();
        if let Ok(name) = id.get_name() {
            out.push(MixerControlReport {
                name: name.to_string(),
                index: id.get_index(),
                has_volume,
                has_switch,
            });
        }
    }
    out
}

/// Discovered PCM device report.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlsaDeviceReport {
    /// Card index.
    pub card_index: i32,
    /// PCM device index on this card (e.g. 0, 3, 7).
    pub device_index: i32,
    /// Device identifier string.
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// ALSA device string (e.g. "hw:CARD=K17,DEV=0" or "hw:2,0").
    pub device_path: String,
    /// True if device is actively opened / busy by another process.
    pub is_busy: bool,
    /// Active playback stream status if currently running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_status: Option<ActiveStreamStatus>,
    /// Probed hardware capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<DeviceCapabilities>,
    /// Error encountered during probe if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Real-time status of an active ALSA stream (from `/proc/asound/cardX/pcmYp/sub0/status`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveStreamStatus {
    /// Stream state (e.g. "RUNNING", "PREPARED", "PAUSED").
    pub state: String,
    /// Active ALSA format (e.g. "DSD_U32_BE", "S32_LE").
    pub format: String,
    /// Sample or frame rate in Hz.
    pub rate_hz: u32,
    /// Channel count.
    pub channels: u32,
    /// Period size in frames.
    pub period_size: u64,
    /// Buffer size in frames.
    pub buffer_size: u64,
}

/// Probed hardware capabilities of an ALSA PCM playback device.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceCapabilities {
    /// Supported audio formats (e.g. ["S16_LE", "S32_LE", "DSD_U32_BE"]).
    pub formats: Vec<String>,
    /// Supported sample / transport frame rates in Hz.
    pub rates_hz: Vec<u32>,
    /// Minimum channels supported.
    pub min_channels: u32,
    /// Maximum channels supported.
    pub max_channels: u32,
    /// Native DSD support summary.
    pub native_dsd: NativeDsdSupport,
    /// DoP (DSD over PCM) support summary.
    pub dop: DopSupport,
    /// Detailed USB altsets if USB audio class device.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub usb_altsets: Vec<UsbAltsetInfo>,
}

/// Native DSD hardware capability details.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NativeDsdSupport {
    /// Whether Native DSD (DSD_U32_BE/LE, etc.) is supported.
    pub supported: bool,
    /// Native DSD formats supported (e.g. ["DSD_U32_BE"]).
    pub formats: Vec<String>,
    /// Maximum container frame rate in Hz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_frame_rate_hz: Option<u32>,
    /// Maximum DSD tier (e.g. "DSD256", "DSD512").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dsd_tier: Option<String>,
    /// USB DSD raw descriptor if present (e.g. "DOP=0, bitrev=0").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsd_raw: Option<String>,
}

/// DoP (DSD over PCM v1.1) capability details.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DopSupport {
    /// Whether DoP is supported (based on 176.4k+ and 24/32-bit PCM availability).
    pub supported: bool,
    /// Highest DoP tier supported (e.g. "DoP64", "DoP128", "DoP256").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_dop_tier: Option<String>,
    /// ALSA containers capable of carrying DoP (e.g. ["s32le", "s24_3le"]).
    pub supported_containers: Vec<String>,
}

/// USB Audio Class altset description from `/proc/asound/cardX/stream0`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsbAltsetInfo {
    /// USB Interface altsetting number.
    pub altset: u32,
    /// Format declared by USB descriptor.
    pub format: String,
    /// Channels declared.
    pub channels: u32,
    /// Sample rates declared.
    pub rates_hz: Vec<u32>,
    /// DSD raw flag if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsd_raw: Option<String>,
}

/// Candidate sample / frame rates to probe on ALSA devices.
pub const CANDIDATE_RATES: &[u32] = &[
    44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000, 705_600, 768_000,
];

/// Computes the DSD tier label from an ALSA `DSD_U32` container frame rate.
pub fn dsd_u32_frame_rate_to_tier(rate_hz: u32) -> Option<&'static str> {
    match rate_hz {
        88_200 => Some("DSD64"),
        96_000 => Some("DSD64 (48k)"),
        176_400 => Some("DSD128"),
        192_000 => Some("DSD128 (48k)"),
        352_800 => Some("DSD256"),
        384_000 => Some("DSD256 (48k)"),
        705_600 => Some("DSD512"),
        768_000 => Some("DSD512 (48k)"),
        _ => None,
    }
}

/// Computes the DoP tier label from a PCM sample rate.
pub fn dop_pcm_rate_to_tier(rate_hz: u32) -> Option<&'static str> {
    match rate_hz {
        176_400 => Some("DoP64"),
        192_000 => Some("DoP64 (48k)"),
        352_800 => Some("DoP128"),
        384_000 => Some("DoP128 (48k)"),
        705_600 => Some("DoP256"),
        768_000 => Some("DoP256 (48k)"),
        _ => None,
    }
}

/// Analyzes a set of formats and rates to synthesize `NativeDsdSupport` and `DopSupport`.
pub fn analyze_capabilities(
    formats: &[String],
    rates_hz: &[u32],
    dsd_raw: Option<String>,
) -> (NativeDsdSupport, DopSupport) {
    let mut native_formats = Vec::new();
    let mut supports_native = false;
    let mut max_dsd_rate: Option<u32> = None;
    let mut max_dsd_tier: Option<String> = None;

    for fmt in formats {
        if fmt.starts_with("DSD_") {
            supports_native = true;
            native_formats.push(fmt.clone());
        }
    }

    if supports_native {
        // Find highest matching DSD frame rate
        for &r in rates_hz.iter().rev() {
            if let Some(tier) = dsd_u32_frame_rate_to_tier(r) {
                max_dsd_rate = Some(r);
                max_dsd_tier = Some(tier.to_string());
                break;
            }
        }
    }

    let native = NativeDsdSupport {
        supported: supports_native,
        formats: native_formats,
        max_frame_rate_hz: max_dsd_rate,
        max_dsd_tier,
        dsd_raw,
    };

    // DoP analysis: requires 24-bit or 32-bit container and at least 176.4 kHz
    let mut dop_containers = Vec::new();
    for fmt in formats {
        match fmt.as_str() {
            "S32_LE" => dop_containers.push("s32le".to_string()),
            "S24_LE" => dop_containers.push("s24le".to_string()),
            "S24_3LE" => dop_containers.push("s24_3le".to_string()),
            _ => {}
        }
    }

    let mut dop_supported = false;
    let mut max_dop_tier: Option<String> = None;

    if !dop_containers.is_empty() {
        for &r in rates_hz.iter().rev() {
            if let Some(tier) = dop_pcm_rate_to_tier(r) {
                dop_supported = true;
                max_dop_tier = Some(tier.to_string());
                break;
            }
        }
    }

    let dop = DopSupport {
        supported: dop_supported,
        max_dop_tier,
        supported_containers: dop_containers,
    };

    (native, dop)
}

/// Parses USB Audio Class stream0 text (from `/proc/asound/cardX/stream0`).
pub fn parse_stream0(content: &str) -> Vec<UsbAltsetInfo> {
    let mut altsets = Vec::new();
    let mut current_altset: Option<u32> = None;
    let mut current_format: Option<String> = None;
    let mut current_channels: Option<u32> = None;
    let mut current_rates: Vec<u32> = Vec::new();
    let mut current_dsd_raw: Option<String> = None;

    let flush_current = |altsets: &mut Vec<UsbAltsetInfo>,
                         current_altset: &mut Option<u32>,
                         current_format: &mut Option<String>,
                         current_channels: &mut Option<u32>,
                         current_rates: &mut Vec<u32>,
                         current_dsd_raw: &mut Option<String>| {
        if let (Some(alt), Some(fmt), Some(ch)) =
            (*current_altset, current_format.take(), *current_channels)
        {
            let mut rates = std::mem::take(current_rates);
            rates.sort_unstable();
            rates.dedup();
            altsets.push(UsbAltsetInfo {
                altset: alt,
                format: fmt,
                channels: ch,
                rates_hz: rates,
                dsd_raw: current_dsd_raw.take(),
            });
        }
        *current_altset = None;
        *current_channels = None;
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Altset ") {
            flush_current(
                &mut altsets,
                &mut current_altset,
                &mut current_format,
                &mut current_channels,
                &mut current_rates,
                &mut current_dsd_raw,
            );
            if let Some(num_str) = trimmed.strip_prefix("Altset ") {
                if let Ok(num) = num_str.trim().parse::<u32>() {
                    current_altset = Some(num);
                }
            }
        } else if let Some(fmt_val) = trimmed.strip_prefix("Format: ") {
            current_format = Some(fmt_val.trim().to_string());
        } else if let Some(ch_val) = trimmed.strip_prefix("Channels: ") {
            if let Ok(ch) = ch_val.trim().parse::<u32>() {
                current_channels = Some(ch);
            }
        } else if let Some(rates_val) = trimmed.strip_prefix("Rates: ") {
            for part in rates_val.split(',') {
                if let Ok(r) = part.trim().parse::<u32>() {
                    current_rates.push(r);
                }
            }
        } else if let Some(dsd_val) = trimmed.strip_prefix("DSD raw: ") {
            current_dsd_raw = Some(dsd_val.trim().to_string());
        }
    }

    flush_current(
        &mut altsets,
        &mut current_altset,
        &mut current_format,
        &mut current_channels,
        &mut current_rates,
        &mut current_dsd_raw,
    );

    altsets
}

/// Reads active stream status from `/proc/asound/cardX/pcmYp/sub0/status` and `hw_params`.
pub fn read_proc_active_status<P: AsRef<Path>>(
    status_path: P,
    hw_params_path: P,
) -> Option<ActiveStreamStatus> {
    let status_str = fs::read_to_string(status_path).ok()?;
    let trimmed = status_str.trim();
    if trimmed.is_empty() || trimmed.contains("closed") {
        return None;
    }

    let mut state = "UNKNOWN".to_string();
    for line in status_str.lines() {
        let l = line.trim();
        if let Some(st) = l.strip_prefix("state: ") {
            state = st.trim().to_string();
            break;
        }
    }

    let hw_str = fs::read_to_string(hw_params_path).ok()?;
    let mut format = "UNKNOWN".to_string();
    let mut rate_hz = 0u32;
    let mut channels = 0u32;
    let mut period_size = 0u64;
    let mut buffer_size = 0u64;

    for line in hw_str.lines() {
        let l = line.trim();
        if let Some(f) = l.strip_prefix("format: ") {
            format = f.trim().to_string();
        } else if let Some(r) = l.strip_prefix("rate: ") {
            // e.g. "88200 (88200/1)"
            let first = r.split_whitespace().next().unwrap_or("");
            if let Ok(val) = first.parse::<u32>() {
                rate_hz = val;
            }
        } else if let Some(c) = l.strip_prefix("channels: ") {
            if let Ok(val) = c.trim().parse::<u32>() {
                channels = val;
            }
        } else if let Some(p) = l.strip_prefix("period_size: ") {
            if let Ok(val) = p.trim().parse::<u64>() {
                period_size = val;
            }
        } else if let Some(b) = l.strip_prefix("buffer_size: ") {
            if let Ok(val) = b.trim().parse::<u64>() {
                buffer_size = val;
            }
        }
    }

    Some(ActiveStreamStatus {
        state,
        format,
        rate_hz,
        channels,
        period_size,
        buffer_size,
    })
}

/// Probes all ALSA hardware playback devices on the host.
pub fn probe_alsa_devices() -> Result<AlsaProbeReport, String> {
    #[cfg(feature = "alsa")]
    {
        probe_alsa_devices_native()
    }
    #[cfg(not(feature = "alsa"))]
    {
        probe_alsa_devices_procfs_only()
    }
}

/// Pure procfs-based probe fallback (inspects `/proc/asound`).
pub fn probe_alsa_devices_procfs_only() -> Result<AlsaProbeReport, String> {
    let cards_content = fs::read_to_string("/proc/asound/cards")
        .map_err(|e| format!("cannot read /proc/asound/cards: {e}"))?;

    let mut cards = Vec::new();
    // Parse /proc/asound/cards lines:
    //  0 [Generic        ]: HDA-Intel - HD-Audio Generic
    //  2 [K17            ]: USB-Audio - FIIO K17
    let mut lines = cards_content.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(first_space) = trimmed.find(' ') {
            if let Ok(index) = trimmed[..first_space].parse::<i32>() {
                let rest = trimmed[first_space..].trim();
                if let (Some(start_id), Some(end_id)) = (rest.find('['), rest.find(']')) {
                    let card_id = rest[start_id + 1..end_id].trim().to_string();
                    let colon_idx = rest.find(':');
                    let name = colon_idx
                        .map(|c| rest[c + 1..].trim().to_string())
                        .unwrap_or_else(|| card_id.clone());

                    // The next line often contains long description
                    let longname = if let Some(next_l) = lines.peek() {
                        if next_l.starts_with("  ") || next_l.starts_with('\t') {
                            lines.next().unwrap_or("").trim().to_string()
                        } else {
                            name.clone()
                        }
                    } else {
                        name.clone()
                    };

                    let usb_id_path = format!("/proc/asound/card{index}/usbid");
                    let usb_id = fs::read_to_string(&usb_id_path)
                        .ok()
                        .map(|s| s.trim().to_string());

                    let stream0_path = format!("/proc/asound/card{index}/stream0");
                    let usb_altsets = fs::read_to_string(&stream0_path)
                        .ok()
                        .map(|s| parse_stream0(&s))
                        .unwrap_or_default();

                    let status_path = format!("/proc/asound/card{index}/pcm0p/sub0/status");
                    let hw_params_path = format!("/proc/asound/card{index}/pcm0p/sub0/hw_params");
                    let active_status = read_proc_active_status(&status_path, &hw_params_path);
                    let is_busy = active_status.is_some();

                    let mut formats_set = BTreeSet::new();
                    let mut rates_set = BTreeSet::new();
                    let mut dsd_raw = None;

                    for alt in &usb_altsets {
                        formats_set.insert(alt.format.clone());
                        for &r in &alt.rates_hz {
                            rates_set.insert(r);
                        }
                        if alt.dsd_raw.is_some() && dsd_raw.is_none() {
                            dsd_raw = alt.dsd_raw.clone();
                        }
                    }

                    let formats: Vec<String> = formats_set.into_iter().collect();
                    let rates_hz: Vec<u32> = rates_set.into_iter().collect();
                    let (native_dsd, dop) =
                        analyze_capabilities(&formats, &rates_hz, dsd_raw.clone());

                    let capabilities = if !formats.is_empty() || !rates_hz.is_empty() {
                        Some(DeviceCapabilities {
                            formats,
                            rates_hz,
                            min_channels: 2,
                            max_channels: 2,
                            native_dsd,
                            dop,
                            usb_altsets,
                        })
                    } else {
                        None
                    };

                    let dev_report = AlsaDeviceReport {
                        card_index: index,
                        device_index: 0,
                        id: card_id.clone(),
                        name: name.clone(),
                        device_path: format!("hw:CARD={card_id},DEV=0"),
                        is_busy,
                        active_status,
                        capabilities,
                        error: None,
                    };

                    cards.push(AlsaCardReport {
                        index,
                        id: card_id,
                        name,
                        longname,
                        usb_id,
                        devices: vec![dev_report],
                        mixer_controls: Vec::new(),
                    });
                }
            }
        }
    }

    Ok(AlsaProbeReport { cards })
}

#[cfg(feature = "alsa")]
fn probe_alsa_devices_native() -> Result<AlsaProbeReport, String> {
    use alsa::Direction;
    use alsa::pcm::{Access, Format, HwParams, PCM};

    let mut report_cards = Vec::new();

    for card in alsa::card::Iter::new() {
        let card = match card {
            Ok(c) => c,
            Err(e) => return Err(format!("failed to iterate ALSA cards: {e}")),
        };

        let card_idx = card.get_index();
        let card_id_path = format!("/proc/asound/card{card_idx}/id");
        let card_id = fs::read_to_string(&card_id_path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| format!("card{card_idx}"));
        let card_name = card
            .get_name()
            .unwrap_or_else(|_| format!("Card {card_idx}"));
        let card_longname = card.get_longname().unwrap_or_else(|_| card_name.clone());

        let usb_id_path = format!("/proc/asound/card{card_idx}/usbid");
        let usb_id = fs::read_to_string(&usb_id_path)
            .ok()
            .map(|s| s.trim().to_string());

        let stream0_path = format!("/proc/asound/card{card_idx}/stream0");
        let usb_altsets = fs::read_to_string(&stream0_path)
            .ok()
            .map(|s| parse_stream0(&s))
            .unwrap_or_default();

        let ctl = match alsa::ctl::Ctl::from_card(&card, false) {
            Ok(c) => c,
            Err(e) => {
                report_cards.push(AlsaCardReport {
                    index: card_idx,
                    id: card_id.clone(),
                    name: card_name.clone(),
                    longname: card_longname.clone(),
                    usb_id,
                    devices: vec![AlsaDeviceReport {
                        card_index: card_idx,
                        device_index: 0,
                        id: card_id.clone(),
                        name: card_name.clone(),
                        device_path: format!("hw:{card_idx},0"),
                        is_busy: false,
                        active_status: None,
                        capabilities: None,
                        error: Some(format!("failed to open ALSA ctl: {e}")),
                    }],
                    mixer_controls: probe_mixer_controls(card_idx),
                });
                continue;
            }
        };

        let mut devices = Vec::new();

        for dev_idx in alsa::ctl::DeviceIter::new(&ctl) {
            let dev_path = format!("hw:CARD={card_id},DEV={dev_idx}");
            let fallback_path = format!("hw:{card_idx},{dev_idx}");

            let status_path = format!("/proc/asound/card{card_idx}/pcm{dev_idx}p/sub0/status");
            let hw_params_path =
                format!("/proc/asound/card{card_idx}/pcm{dev_idx}p/sub0/hw_params");
            let active_status = read_proc_active_status(&status_path, &hw_params_path);
            let is_busy = active_status.is_some();

            // First check if we can probe capabilities via alsa-lib PCM handle
            let mut probed_formats = Vec::new();
            let mut probed_rates = Vec::new();
            let mut min_channels = 2;
            let mut max_channels = 2;
            let mut probe_err = None;

            // Open non-blocking so we don't block if another process holds it
            match PCM::new(&dev_path, Direction::Playback, true)
                .or_else(|_| PCM::new(&fallback_path, Direction::Playback, true))
            {
                Ok(pcm) => {
                    if let Ok(hwp) = HwParams::any(&pcm) {
                        // Test access modes
                        let _ = hwp.test_access(Access::MMapInterleaved);
                        let _ = hwp.test_access(Access::RWInterleaved);

                        // Test channels
                        if hwp.test_channels(1).is_ok() {
                            min_channels = 1;
                        }
                        if hwp.test_channels(2).is_ok() {
                            max_channels = 2;
                        }

                        // Test formats
                        const FORMAT_TESTS: &[(Format, &str)] = &[
                            (Format::S16LE, "S16_LE"),
                            (Format::S24LE, "S24_LE"),
                            (Format::S243LE, "S24_3LE"),
                            (Format::S32LE, "S32_LE"),
                            (Format::FloatLE, "FLOAT_LE"),
                            (Format::DSDU8, "DSD_U8"),
                            (Format::DSDU16LE, "DSD_U16_LE"),
                            (Format::DSDU16BE, "DSD_U16_BE"),
                            (Format::DSDU32LE, "DSD_U32_LE"),
                            (Format::DSDU32BE, "DSD_U32_BE"),
                        ];

                        for &(fmt, name) in FORMAT_TESTS {
                            if hwp.test_format(fmt).is_ok() {
                                probed_formats.push(name.to_string());
                            }
                        }

                        // Test rates
                        for &r in CANDIDATE_RATES {
                            if hwp.test_rate(r).is_ok() {
                                probed_rates.push(r);
                            }
                        }
                    }
                }
                Err(e) => {
                    // Device is likely busy (EBUSY) or capture-only
                    if e.errno() == 16 {
                        // EBUSY: Fall back to stream0 descriptors if available
                    } else {
                        probe_err = Some(format!("{e}"));
                    }
                }
            }

            // Merge with stream0 descriptors if present
            if !usb_altsets.is_empty() {
                for alt in &usb_altsets {
                    if !probed_formats.contains(&alt.format) {
                        probed_formats.push(alt.format.clone());
                    }
                    for &r in &alt.rates_hz {
                        if !probed_rates.contains(&r) {
                            probed_rates.push(r);
                        }
                    }
                }
                probed_rates.sort_unstable();
            }

            let dsd_raw = usb_altsets.iter().find_map(|a| a.dsd_raw.clone());
            let (native_dsd, dop) = analyze_capabilities(&probed_formats, &probed_rates, dsd_raw);

            let capabilities = if !probed_formats.is_empty() || !probed_rates.is_empty() {
                Some(DeviceCapabilities {
                    formats: probed_formats,
                    rates_hz: probed_rates,
                    min_channels,
                    max_channels,
                    native_dsd,
                    dop,
                    usb_altsets: usb_altsets.clone(),
                })
            } else {
                None
            };

            devices.push(AlsaDeviceReport {
                card_index: card_idx,
                device_index: dev_idx,
                id: format!("pcm{dev_idx}"),
                name: format!("{card_name} (Device {dev_idx})"),
                device_path: dev_path,
                is_busy: is_busy || probe_err.as_ref().is_some_and(|e| e.contains("busy")),
                active_status,
                capabilities,
                error: probe_err,
            });
        }

        report_cards.push(AlsaCardReport {
            index: card_idx,
            id: card_id,
            name: card_name,
            longname: card_longname,
            usb_id,
            devices,
            mixer_controls: probe_mixer_controls(card_idx),
        });
    }

    Ok(AlsaProbeReport {
        cards: report_cards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const K17_STREAM0_FIXTURE: &str = r#"
GuangZhou FiiO Electronics Co.,Ltd FIIO K17 at usb-0000:07:00.4-1.4, high speed : USB Audio

Playback:
  Status: Running
    Interface = 1
    Altset = 3
    Packet Size = 136
    Momentary freq = 88200 Hz (0xb.0668)
    Feedback Format = 16.16
  Interface 1
    Altset 1
    Format: S32_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000
    Data packet interval: 125 us
    Bits: 32
    Channel map: FL FR
    Sync Endpoint: 0x81 (1 IN)
    Sync EP Interface: 1
    Sync EP Altset: 1
    Implicit Feedback Mode: No
  Interface 1
    Altset 2
    Format: S16_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000
    Data packet interval: 125 us
    Bits: 16
    Channel map: FL FR
    Sync Endpoint: 0x81 (1 IN)
    Sync EP Interface: 1
    Sync EP Altset: 2
    Implicit Feedback Mode: No
  Interface 1
    Altset 3
    Format: DSD_U32_BE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000, 705600, 768000
    Data packet interval: 125 us
    Bits: 32
    DSD raw: DOP=0, bitrev=0
    Channel map: FL FR
    Sync Endpoint: 0x81 (1 IN)
    Sync EP Interface: 1
    Sync EP Altset: 3
    Implicit Feedback Mode: No
"#;

    const JA11_STREAM0_FIXTURE: &str = r#"
GuangZhou FiiO JadeAudio JA11 at usb-0000:07:00.4-1.2, high speed : USB Audio

Playback:
  Status: Stop
  Interface 1
    Altset 1
    Format: S32_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000
  Interface 1
    Altset 2
    Format: S24_3LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000
  Interface 1
    Altset 3
    Format: S16_LE
    Channels: 2
    Endpoint: 0x01 (1 OUT) (ASYNC)
    Rates: 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000
"#;

    #[test]
    fn test_parse_k17_stream0() {
        let altsets = parse_stream0(K17_STREAM0_FIXTURE);
        assert_eq!(altsets.len(), 3);

        assert_eq!(altsets[0].altset, 1);
        assert_eq!(altsets[0].format, "S32_LE");
        assert_eq!(altsets[0].channels, 2);
        assert_eq!(altsets[0].rates_hz.len(), 10);
        assert_eq!(altsets[0].rates_hz.last().copied(), Some(768_000));

        assert_eq!(altsets[1].altset, 2);
        assert_eq!(altsets[1].format, "S16_LE");

        assert_eq!(altsets[2].altset, 3);
        assert_eq!(altsets[2].format, "DSD_U32_BE");
        assert_eq!(altsets[2].dsd_raw.as_deref(), Some("DOP=0, bitrev=0"));

        let formats: Vec<String> = altsets.iter().map(|a| a.format.clone()).collect();
        let rates: Vec<u32> = altsets[0].rates_hz.clone();
        let (native, dop) = analyze_capabilities(&formats, &rates, altsets[2].dsd_raw.clone());

        assert!(native.supported);
        assert_eq!(native.formats, vec!["DSD_U32_BE"]);
        assert_eq!(native.max_dsd_tier.as_deref(), Some("DSD512 (48k)"));
        assert_eq!(native.dsd_raw.as_deref(), Some("DOP=0, bitrev=0"));

        assert!(dop.supported);
        assert_eq!(dop.max_dop_tier.as_deref(), Some("DoP256 (48k)"));
        assert_eq!(dop.supported_containers, vec!["s32le"]);
    }

    #[test]
    fn test_parse_ja11_stream0() {
        let altsets = parse_stream0(JA11_STREAM0_FIXTURE);
        assert_eq!(altsets.len(), 3);

        let formats: Vec<String> = altsets.iter().map(|a| a.format.clone()).collect();
        let rates: Vec<u32> = altsets[0].rates_hz.clone();
        let (native, dop) = analyze_capabilities(&formats, &rates, None);

        assert!(!native.supported);
        assert!(dop.supported);
        assert_eq!(dop.max_dop_tier.as_deref(), Some("DoP128 (48k)"));
        assert!(dop.supported_containers.contains(&"s32le".to_string()));
        assert!(dop.supported_containers.contains(&"s24_3le".to_string()));
    }

    #[test]
    fn test_rate_to_tier_conversions() {
        assert_eq!(dsd_u32_frame_rate_to_tier(88_200), Some("DSD64"));
        assert_eq!(dsd_u32_frame_rate_to_tier(176_400), Some("DSD128"));
        assert_eq!(dsd_u32_frame_rate_to_tier(352_800), Some("DSD256"));
        assert_eq!(dsd_u32_frame_rate_to_tier(705_600), Some("DSD512"));

        assert_eq!(dop_pcm_rate_to_tier(176_400), Some("DoP64"));
        assert_eq!(dop_pcm_rate_to_tier(352_800), Some("DoP128"));
        assert_eq!(dop_pcm_rate_to_tier(705_600), Some("DoP256"));
    }
}
