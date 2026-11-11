//! Capability pre-check (`capability-check`): the configuration against an ALSA probe report.
//!
//! Pure function over data: no probing and no CPU load here, so the review page can run it on
//! every edit and tests feed it fake reports. It answers only "can this DAC take it at all";
//! whether this machine can compute it is the benchmark's job.

use audigma_dsp::SampleFormat;
use audigma_dsp::config::TargetFormat;
use audigma_dsp::rates::{PcmRateHz, RateFamily};
use audigma_io::{AlsaCardReport, AlsaDeviceReport, AlsaProbeReport};
use audigma_settings::{OutputBackend, OutputTransport, Settings, TargetRateFamily};
use serde::Serialize;

/// How serious a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The device cannot do this at all.
    Error,
    /// Works for some sources or cannot be confirmed.
    Warning,
    /// Nothing to act on; explains why a check was skipped.
    Info,
}

/// One finding, attributed to a config key like the validator's issues.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapabilityFinding {
    pub path: &'static str,
    pub severity: Severity,
    pub message: String,
}

fn finding(
    path: &'static str,
    severity: Severity,
    message: impl Into<String>,
) -> CapabilityFinding {
    CapabilityFinding {
        path,
        severity,
        message: message.into(),
    }
}

/// Finds the probed PCM device that an ALSA device string names.
///
/// Understands `hw:<card>,<dev>`, `hw:CARD=<id>[,DEV=<n>]` and an exact match of the probe's
/// `device_path`. `plughw:`, `default` and other plugin names convert formats and rates, so
/// the hardware report does not describe them and they return `None`.
pub fn find_device<'a>(
    report: &'a AlsaProbeReport,
    device: &str,
) -> Option<(&'a AlsaCardReport, &'a AlsaDeviceReport)> {
    let device = device.trim();
    for card in &report.cards {
        for dev in &card.devices {
            if dev.device_path == device {
                return Some((card, dev));
            }
        }
    }
    let rest = device.strip_prefix("hw:")?;
    let mut parts = rest.split(',');
    let card_part = parts.next()?.trim();
    let dev_part = parts.next().map(str::trim);
    if parts.next().is_some() {
        return None;
    }
    let dev_index = match dev_part {
        None => 0,
        Some(d) => d.strip_prefix("DEV=").unwrap_or(d).parse::<i32>().ok()?,
    };
    let card = if let Some(id) = card_part.strip_prefix("CARD=") {
        report
            .cards
            .iter()
            .find(|c| c.id.eq_ignore_ascii_case(id))?
    } else {
        let index = card_part.parse::<i32>().ok()?;
        report.cards.iter().find(|c| c.index == index)?
    };
    let dev = card.devices.iter().find(|d| d.device_index == dev_index)?;
    Some((card, dev))
}

fn families(f: TargetRateFamily) -> Vec<RateFamily> {
    match f {
        TargetRateFamily::Source => vec![RateFamily::Hz44100, RateFamily::Hz48000],
        TargetRateFamily::Hz44100 => vec![RateFamily::Hz44100],
        TargetRateFamily::Hz48000 => vec![RateFamily::Hz48000],
    }
}

fn family_label(f: RateFamily) -> &'static str {
    match f {
        RateFamily::Hz44100 => "44.1 kHz",
        RateFamily::Hz48000 => "48 kHz",
        RateFamily::Other => "other",
    }
}

fn alsa_format_name(f: SampleFormat) -> Option<&'static str> {
    match f {
        SampleFormat::S16Le => Some("S16_LE"),
        SampleFormat::S24Le => Some("S24_LE"),
        SampleFormat::S24_3Le => Some("S24_3LE"),
        SampleFormat::S32Le => Some("S32_LE"),
        _ => None,
    }
}

/// Reports which clock families get their `rate_hz` (per family) from the device, as findings
/// on `path`: an error when none does, a warning when only some do (`source` follows the file).
fn rate_findings(
    path: &'static str,
    what: &str,
    allowed: &[RateFamily],
    needed: impl Fn(RateFamily) -> Option<u32>,
    device_rates: &[u32],
    out: &mut Vec<CapabilityFinding>,
) {
    let mut ok = Vec::new();
    let mut missing = Vec::new();
    for &fam in allowed {
        match needed(fam) {
            Some(hz) if device_rates.contains(&hz) => ok.push(fam),
            Some(hz) => missing.push(format!("{} family needs {hz} Hz", family_label(fam))),
            None => missing.push(format!("{} family has no matching rate", family_label(fam))),
        }
    }
    if missing.is_empty() {
        return;
    }
    if ok.is_empty() {
        out.push(finding(
            path,
            Severity::Error,
            format!("The device does not offer {what}: {}.", missing.join("; ")),
        ));
    } else {
        out.push(finding(
            path,
            Severity::Warning,
            format!(
                "The device offers {what} only for the {} family ({}).",
                ok.iter()
                    .map(|f| family_label(*f))
                    .collect::<Vec<_>>()
                    .join(", "),
                missing.join("; ")
            ),
        ));
    }
}

/// Checks `settings` against the probed hardware.
///
/// Only an ALSA output with a named device is checked; PipeWire output is shaped by the sound
/// server, not by the hardware report. Findings come back in a stable order.
pub fn check(settings: &Settings, probe: &AlsaProbeReport) -> Vec<CapabilityFinding> {
    let mut out = Vec::new();
    let Some(output) = settings.output.as_ref() else {
        return out;
    };
    if output.backend != Some(OutputBackend::Alsa) {
        return out;
    }
    let Some(device) = output.device.as_deref() else {
        return out;
    };
    let Some((card, dev)) = find_device(probe, device) else {
        out.push(finding(
            "output.device",
            Severity::Info,
            format!(
                "`{device}` is not a hardware device in the probe, so its capabilities are unknown."
            ),
        ));
        return out;
    };
    let Some(caps) = dev.capabilities.as_ref() else {
        let why = dev.error.as_deref().unwrap_or("the device was busy");
        out.push(finding(
            "output.device",
            Severity::Info,
            format!(
                "{} could not be probed ({why}), so its capabilities are unknown.",
                card.id
            ),
        ));
        return out;
    };
    let Some(target) = settings.target.as_ref() else {
        return out;
    };
    let allowed = families(target.rate_family);

    match target.format {
        TargetFormat::Dsd => {
            let Some(mult) = target.dsd_rate else {
                return out;
            };
            match output.transport {
                Some(OutputTransport::Native) => {
                    if !caps.native_dsd.supported {
                        out.push(finding(
                            "output.transport",
                            Severity::Error,
                            "The device does not offer native DSD.",
                        ));
                    } else {
                        rate_findings(
                            "target.dsd_rate",
                            &format!("native {mult}"),
                            &allowed,
                            |fam| {
                                mult.to_bit_rate(fam)
                                    .map(|b| b.to_alsa_dsd_u32_frame_rate().get())
                            },
                            &caps.rates_hz,
                            &mut out,
                        );
                    }
                }
                Some(OutputTransport::Dop) => {
                    if !caps.dop.supported {
                        out.push(finding(
                            "output.transport",
                            Severity::Error,
                            "The device does not offer the PCM rates and word size DoP needs.",
                        ));
                    } else {
                        rate_findings(
                            "target.dsd_rate",
                            &format!("DoP at {mult}"),
                            &allowed,
                            |fam| mult.to_bit_rate(fam).map(|b| b.to_dop_frame_rate().get()),
                            &caps.rates_hz,
                            &mut out,
                        );
                    }
                }
                _ => {}
            }
        }
        TargetFormat::Pcm => {
            if let Some(rate) = target.pcm_rate_hz {
                rate_findings(
                    "target.pcm_rate_hz",
                    &format!("{} Hz PCM", rate.get()),
                    &allowed,
                    |fam| rate.corresponding_rate_in_family(fam).map(PcmRateHz::get),
                    &caps.rates_hz,
                    &mut out,
                );
            }
            if let Some(name) = target.sample_format.and_then(alsa_format_name) {
                if !caps.formats.iter().any(|f| f == name) {
                    out.push(finding(
                        "target.sample_format",
                        Severity::Error,
                        format!(
                            "The device does not offer {name}; it offers {}.",
                            caps.formats.join(", ")
                        ),
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_io::alsa::probe::{DeviceCapabilities, DopSupport, NativeDsdSupport};

    fn caps(formats: &[&str], rates: &[u32], native: bool, dop: bool) -> DeviceCapabilities {
        DeviceCapabilities {
            formats: formats.iter().map(|s| s.to_string()).collect(),
            rates_hz: rates.to_vec(),
            min_channels: 2,
            max_channels: 2,
            native_dsd: NativeDsdSupport {
                supported: native,
                formats: vec![],
                max_frame_rate_hz: None,
                max_dsd_tier: None,
                dsd_raw: None,
            },
            dop: DopSupport {
                supported: dop,
                max_dop_tier: None,
                supported_containers: vec![],
            },
            usb_altsets: vec![],
        }
    }

    fn report(capabilities: Option<DeviceCapabilities>) -> AlsaProbeReport {
        AlsaProbeReport {
            cards: vec![AlsaCardReport {
                index: 2,
                id: "K17".into(),
                name: "USB-Audio".into(),
                longname: "FIIO K17".into(),
                usb_id: None,
                devices: vec![AlsaDeviceReport {
                    card_index: 2,
                    device_index: 0,
                    id: "USB Audio".into(),
                    name: "USB Audio".into(),
                    device_path: "hw:CARD=K17,DEV=0".into(),
                    is_busy: false,
                    active_status: None,
                    capabilities,
                    error: None,
                }],
            }],
        }
    }

    fn settings(body: &str) -> Settings {
        Settings::from_toml_str(&format!("schema_version = 1\n{body}")).expect("settings parse")
    }

    const DSD_NATIVE: &str = r#"
[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "native"
[target]
format = "dsd"
dsd_rate = "dsd256"
"#;

    #[test]
    fn finds_devices_by_every_spelling() {
        let r = report(None);
        for d in ["hw:CARD=K17,DEV=0", "hw:CARD=k17", "hw:2,0", "hw:2"] {
            assert!(find_device(&r, d).is_some(), "{d}");
        }
        for d in ["plughw:2,0", "default", "hw:3,0", "hw:2,1", "hw:CARD=Nope"] {
            assert!(find_device(&r, d).is_none(), "{d}");
        }
    }

    #[test]
    fn native_dsd_rate_supported_in_both_families_is_clean() {
        let r = report(Some(caps(
            &["DSD_U32_BE"],
            &[352_800, 384_000],
            true,
            false,
        )));
        assert!(check(&settings(DSD_NATIVE), &r).is_empty());
    }

    #[test]
    fn native_dsd_missing_one_family_warns() {
        let r = report(Some(caps(&["DSD_U32_BE"], &[352_800], true, false)));
        let f = check(&settings(DSD_NATIVE), &r);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "target.dsd_rate");
        assert_eq!(f[0].severity, Severity::Warning);
    }

    #[test]
    fn native_dsd_rate_not_offered_is_an_error_but_pinned_family_matters() {
        let r = report(Some(caps(&["DSD_U32_BE"], &[88_200, 176_400], true, false)));
        let f = check(&settings(DSD_NATIVE), &r);
        assert_eq!(
            (f[0].path, f[0].severity),
            ("target.dsd_rate", Severity::Error)
        );
        // Pinning the 44.1 family does not change that; pinning to a family the DAC has does.
        let ok = format!("{DSD_NATIVE}rate_family = \"44100\"\ndsd_rate = \"dsd128\"\n")
            .replace("dsd_rate = \"dsd256\"\n", "");
        assert!(check(&settings(&ok), &r).is_empty());
    }

    #[test]
    fn native_transport_without_native_dsd_is_an_error_on_transport() {
        let r = report(Some(caps(&["S32_LE"], &[176_400, 352_800], false, true)));
        let f = check(&settings(DSD_NATIVE), &r);
        assert_eq!(
            (f[0].path, f[0].severity),
            ("output.transport", Severity::Error)
        );
    }

    #[test]
    fn dop_checks_carrier_rate() {
        let body = DSD_NATIVE
            .replace("native", "dop")
            .replace("dsd256", "dsd64");
        let good = report(Some(caps(&["S32_LE"], &[176_400, 192_000], false, true)));
        assert!(check(&settings(&body), &good).is_empty());
        let bad = report(Some(caps(&["S32_LE"], &[44_100], false, true)));
        assert_eq!(check(&settings(&body), &bad)[0].severity, Severity::Error);
    }

    #[test]
    fn pcm_rate_and_format() {
        let body = r#"
[output]
backend = "alsa"
device = "hw:2,0"
[target]
format = "pcm"
pcm_rate_hz = 192000
sample_format = "s24le"
"#;
        let r = report(Some(caps(&["S32_LE"], &[176_400, 192_000], false, false)));
        let f = check(&settings(body), &r);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "target.sample_format");
        let r = report(Some(caps(&["S24_LE"], &[48_000], false, false)));
        let f = check(&settings(body), &r);
        assert_eq!(f[0].path, "target.pcm_rate_hz");
        assert_eq!(f[0].severity, Severity::Error);
    }

    #[test]
    fn unprobed_or_unknown_devices_give_info_only() {
        let r = report(None);
        let f = check(&settings(DSD_NATIVE), &r);
        assert_eq!((f.len(), f[0].severity), (1, Severity::Info));
        let other = DSD_NATIVE.replace("hw:CARD=K17,DEV=0", "plughw:2,0");
        let f = check(&settings(&other), &r);
        assert_eq!((f.len(), f[0].severity), (1, Severity::Info));
    }

    #[test]
    fn pipewire_output_is_not_checked() {
        let body =
            "[output]\nbackend = \"pipewire\"\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n";
        assert!(check(&settings(body), &report(None)).is_empty());
    }
}
