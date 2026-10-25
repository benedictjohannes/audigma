//! Handler for `audigma probe` subcommand.

use audigma_engine::traits::EngineError;
use clap::Args;
use serde_json::json;

#[derive(Args, Debug)]
pub struct ProbeArgs {
    /// Audio backend to probe ("alsa" or "pipewire")
    #[arg(short = 'b', long = "backend", default_value = "alsa")]
    pub backend: String,

    /// Output report in structured JSON format
    #[arg(long = "json")]
    pub json: bool,
}

pub fn run_probe(args: ProbeArgs) -> Result<(), EngineError> {
    match args.backend.to_lowercase().as_str() {
        "alsa" => {
            let report = audigma_io::alsa::probe_alsa_devices()
                .map_err(|e| EngineError::Pipeline(format!("ALSA probe failed: {e}")))?;

            if args.json {
                let json_val = serde_json::to_string_pretty(&report).map_err(|e| {
                    EngineError::Pipeline(format!("JSON serialization failed: {e}"))
                })?;
                println!("{json_val}");
            } else {
                print_alsa_probe_report(&report);
            }
            Ok(())
        }
        "pipewire" => {
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "backend": "pipewire",
                        "status": "planned for Phase 4 (PipeWire Virtual Sink & Graph Clock Bridge)",
                        "note": "Use 'audigma probe --backend alsa' to inspect direct ALSA DAC hardware."
                    }))
                    .unwrap()
                );
            } else {
                println!("audigma probe: PipeWire backend probing is scheduled for Phase 4.");
                println!(
                    "Use 'audigma probe --backend alsa' to probe direct hardware ALSA devices."
                );
            }
            Ok(())
        }
        other => Err(EngineError::Pipeline(format!(
            "unknown audio backend '{other}'; supported backends: 'alsa', 'pipewire'"
        ))),
    }
}

pub fn print_alsa_probe_report(report: &audigma_io::AlsaProbeReport) {
    println!("=== ALSA Hardware Capability Probe Report ===");
    println!("Discovered {} sound card(s):\n", report.cards.len());

    for card in &report.cards {
        let usb_str = card
            .usb_id
            .as_ref()
            .map(|u| format!(" [USB ID: {u}]"))
            .unwrap_or_default();
        println!(
            "Card {}: [{}] {}{}",
            card.index, card.id, card.name, usb_str
        );
        if !card.longname.is_empty() && card.longname != card.name {
            println!("  Longname: {}", card.longname);
        }

        if card.devices.is_empty() {
            println!("  (No PCM playback devices found)");
        }

        for dev in &card.devices {
            println!("  Endpoint: {}", dev.device_path);
            if dev.is_busy {
                if let Some(ref st) = dev.active_status {
                    println!(
                        "    Status:      ACTIVE / BUSY (state: {}, format: {}, rate: {} Hz, period: {}, buffer: {})",
                        st.state, st.format, st.rate_hz, st.period_size, st.buffer_size
                    );
                } else {
                    println!("    Status:      BUSY (opened by another process)");
                }
            } else {
                println!("    Status:      IDLE (available for exclusive access)");
            }

            if let Some(ref caps) = dev.capabilities {
                if caps.native_dsd.supported {
                    let dsd_fmts = caps.native_dsd.formats.join(", ");
                    let tier = caps.native_dsd.max_dsd_tier.as_deref().unwrap_or("unknown");
                    let raw_str = caps
                        .native_dsd
                        .dsd_raw
                        .as_ref()
                        .map(|r| format!(" (raw: {r})"))
                        .unwrap_or_default();
                    println!(
                        "    Native DSD:  Supported (max: {tier}, formats: [{dsd_fmts}]{raw_str})"
                    );
                } else {
                    println!("    Native DSD:  Not supported");
                }

                if caps.dop.supported {
                    let tier = caps.dop.max_dop_tier.as_deref().unwrap_or("unknown");
                    let containers = caps.dop.supported_containers.join(", ");
                    println!(
                        "    DoP (v1.1):  Supported (max: {tier}, containers: [{containers}])"
                    );
                } else {
                    println!("    DoP (v1.1):  Not supported");
                }

                if !caps.formats.is_empty() {
                    let pcm_fmts: Vec<&str> = caps
                        .formats
                        .iter()
                        .filter(|f| !f.starts_with("DSD_"))
                        .map(|s| s.as_str())
                        .collect();
                    if !pcm_fmts.is_empty() {
                        println!("    PCM Formats: {}", pcm_fmts.join(", "));
                    }
                }

                if !caps.rates_hz.is_empty() {
                    let rates_str: Vec<String> = caps
                        .rates_hz
                        .iter()
                        .map(|r| {
                            if *r >= 1000 && *r % 1000 == 0 {
                                format!("{}k", *r / 1000)
                            } else if *r >= 1000 {
                                format!("{:.1}k", *r as f64 / 1000.0)
                            } else {
                                format!("{r}")
                            }
                        })
                        .collect();
                    println!("    Rates:       {} Hz", rates_str.join(", "));
                }

                if !caps.usb_altsets.is_empty() {
                    println!("    USB Altsets:");
                    for alt in &caps.usb_altsets {
                        let dsd = alt
                            .dsd_raw
                            .as_ref()
                            .map(|d| format!(" (DSD: {d})"))
                            .unwrap_or_default();
                        println!(
                            "      Altset {}: {} ({} ch){dsd}",
                            alt.altset, alt.format, alt.channels
                        );
                    }
                }
            } else if let Some(ref err) = dev.error {
                println!("    Error:       {err}");
            }
        }
        println!();
    }
}
