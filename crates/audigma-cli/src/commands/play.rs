//! Handler for `audigma play` subcommand.

use std::io::Write;
use std::path::PathBuf;

use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::{AudioInput, AudioOutput};
use audigma_engine::{IdleCarrierFormat, LifecycleController, OfflinePipeline};
use audigma_io::alsa::{AlsaAudioOutput, AlsaOutputConfig};
use audigma_io::file::FileAudioInput;
#[cfg(feature = "pipewire")]
use audigma_io::pipewire::PipeWirePcmOutput;
use audigma_io::stdio::StdioAudioInput;
use audigma_settings::{OutputBackend, OutputContainer, OutputTransport, Settings};
use clap::Args;
use serde_json::json;

use crate::error::CliError;
use crate::overrides::apply_toml_override;
use crate::volume::{VolumeArg, parse_volume_arg, validate_volume_db};

#[derive(Args, Debug)]
pub struct PlayArgs {
    /// Input file path (WAV/FLAC) or '-' for stdin
    #[arg(short = 'i', long = "input")]
    pub input: String,

    /// Target ALSA hardware device (e.g. "hw:CARD=K17,DEV=0" or "hw:CARD=JA11,DEV=0")
    #[arg(short = 'd', long = "device")]
    pub device: Option<String>,

    /// Optional playback volume in dB (e.g. -3.0) or percentage (e.g. 80%),
    /// using the same syntax as `audigma set volume`
    #[arg(short = 'v', long = "volume", allow_hyphen_values = true)]
    pub volume: Option<String>,

    /// Ad-hoc configuration override; the value is parsed as a TOML literal
    /// (e.g. -O output.transport=dop -O output.buffer_ms=20 -O output.device='"2"')
    #[arg(short = 'O', long = "override")]
    pub overrides: Vec<String>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config", env = "AUDIGMA_CONFIG")]
    pub config: Option<PathBuf>,

    /// Emit JSON status report at playback completion
    #[arg(long = "json")]
    pub json: bool,
}

pub fn run_play(args: PlayArgs) -> Result<(), CliError> {
    // 1. Resolve base settings as a toml::Table for flexible layered overriding
    let default_settings = Settings::default_offline(OutputContainer::Raw);
    let mut toml_table: toml::Table = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        toml::from_str(&content)
            .map_err(|e| CliError::Config(format!("failed to parse config TOML: {e}")))?
    } else {
        toml::Table::try_from(&default_settings).map_err(|e| {
            CliError::Serialization(format!("failed to convert default settings: {e}"))
        })?
    };

    // For live playback, default peak_analysis to single-pass (force override from file's two-pass)
    // Users can still explicitly opt into two-pass using `-O gain.peak_analysis=two-pass`.
    let gain_entry = toml_table
        .entry("gain".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(gain_tbl) = gain_entry {
        gain_tbl.insert(
            "peak_analysis".to_string(),
            toml::Value::String("single-pass".to_string()),
        );
    }

    // 3. Apply common CLI ergonomics (device). Volume is parsed here but
    //    applied later as the runtime user-volume component, not as a
    //    `[gain] preamp_db` override: the configured preamp is calibration, while
    //    `-v` is the user-facing playback volume.
    let user_volume_db: Option<f64> = match args.volume.as_deref() {
        None => None,
        Some(raw) => Some(match parse_volume_arg(raw).map_err(CliError::Volume)? {
            VolumeArg::Db(db) => validate_volume_db(db).map_err(CliError::Volume)?,
            // `play` has no hardware mixer bridge, so the software ALSA curve is
            // the analogue of the daemon's `volume_control = "software"` mode.
            VolumeArg::Percent(pct) => {
                audigma_dsp::alsamixer_percentage_to_db(pct, audigma_dsp::DEFAULT_SOFTWARE_MIN_DB)
            }
        }),
    };

    if let Some(ref dev) = args.device {
        apply_toml_override(&mut toml_table, "output.device", dev)?;
    }

    // 4. Apply ad-hoc CLI overrides (-O key=value)
    for ovr in &args.overrides {
        let (key, val) = ovr.split_once('=').ok_or_else(|| {
            CliError::Override(format!(
                "invalid override syntax '{ovr}' (expected key=value)"
            ))
        })?;
        apply_toml_override(&mut toml_table, key.trim(), val.trim())?;
    }

    // Live backend: whatever the config or `-O output.backend=...` chose, else exclusive ALSA.
    let backend = apply_live_output_defaults(&mut toml_table, args.device.is_some())?;

    // 5. Deserialize merged TOML table into typed Settings and validate
    let mut settings = Settings::from_toml_table(toml_table)?;
    // `play` streams in real time and cannot look ahead; peak analysis is convert-only.
    if let Some(gain) = settings.gain.as_mut() {
        gain.peak_analysis = audigma_settings::PeakAnalysisMode::SinglePass;
    }
    settings.validate()?;

    // 3. Open AudioInput
    let (input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if args.input == "-" {
        let sample_format = settings
            .input
            .as_ref()
            .and_then(|i| i.sample_format)
            .unwrap_or(SampleFormat::S16Le);
        let sample_rate = settings
            .input
            .as_ref()
            .and_then(|i| i.rate_hz)
            .unwrap_or(PcmRateHz::PCM_44100);
        let channels = settings
            .input
            .as_ref()
            .and_then(|i| i.channels)
            .unwrap_or(2);
        (
            Box::new(StdioAudioInput::new(sample_format, sample_rate, channels)),
            sample_rate,
        )
    } else {
        let dsd_gain_db = settings
            .input
            .as_ref()
            .map(|i| i.dsd_gain_db())
            .unwrap_or(6.0);
        let max_intermediate = settings.resolved_dsd_max_intermediate_rate_hz();
        let file_input =
            FileAudioInput::open_with_dsd_full_config(&args.input, dsd_gain_db, max_intermediate)?;
        let rate = file_input.info().sample_rate;
        (Box::new(file_input), rate)
    };

    // 4. Prepare Pipeline and layer the CLI volume on top of the configured preamp
    let pipeline = {
        let placement = crate::runtime::resolve(&settings, 2, true)?;
        crate::runtime::apply_housekeeping(&placement);
        OfflinePipeline::prepare_for_input(
            settings.clone(),
            &*input,
            Some(crate::runtime::worker_setup(&placement)),
        )?
    };
    let pipeline = match user_volume_db {
        Some(db) => pipeline.with_user_volume_db(db),
        None => pipeline,
    };
    // Live sinks are raw streams (not seekable); the engine would otherwise default to WAV/DSF.
    let pipeline = pipeline.with_container(OutputContainer::Raw);
    match backend {
        OutputBackend::Alsa => play_alsa(&args, &settings, input, input_rate, pipeline),
        OutputBackend::Pipewire => play_pipewire(&args, &settings, input, input_rate, pipeline),
    }
}

/// Applies the live-output defaults after all overrides and returns the chosen backend.
///
/// `backend` defaults to exclusive ALSA. A PipeWire output carries unquantized interleaved f32
/// (`Design.md §1.4`), so it fixes `transport = "pcm"` and `target.sample_format = "f32"`;
/// an explicit conflicting transport, a DSD target or `-d` is rejected rather than ignored.
fn apply_live_output_defaults(
    table: &mut toml::Table,
    device_flag: bool,
) -> Result<OutputBackend, CliError> {
    let out = table
        .entry("output".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .ok_or_else(|| CliError::Config("[output] must be a table".into()))?;
    let backend = match out.get("backend").and_then(|v| v.as_str()) {
        None | Some("alsa") => OutputBackend::Alsa,
        Some("pipewire") => OutputBackend::Pipewire,
        Some(other) => {
            return Err(CliError::Config(format!(
                "output.backend \"{other}\" is not a live backend (expected \"pipewire\" or \"alsa\")"
            )));
        }
    };
    out.insert(
        "backend".to_string(),
        toml::Value::String(
            match backend {
                OutputBackend::Alsa => "alsa",
                OutputBackend::Pipewire => "pipewire",
            }
            .to_string(),
        ),
    );
    match backend {
        OutputBackend::Alsa => {
            out.entry("transport".to_string())
                .or_insert_with(|| toml::Value::String("native".to_string()));
            out.entry("buffer_ms".to_string())
                .or_insert_with(|| toml::Value::Integer(40));
            out.entry("period_ms".to_string())
                .or_insert_with(|| toml::Value::Integer(5));
            out.entry("reserve_device".to_string())
                .or_insert_with(|| toml::Value::Boolean(true));
        }
        OutputBackend::Pipewire => {
            if device_flag {
                return Err(CliError::Config(
                    "-d/--device selects an ALSA device; with output.backend = \"pipewire\" use -O output.target_node='\"<node>\"'".into(),
                ));
            }
            match out.get("transport").and_then(|v| v.as_str()) {
                None | Some("pcm") => {}
                Some(t) => {
                    return Err(CliError::Config(format!(
                        "output.transport = \"{t}\" cannot be used with output.backend = \"pipewire\" (PCM only)"
                    )));
                }
            }
            out.insert(
                "transport".to_string(),
                toml::Value::String("pcm".to_string()),
            );
            out.entry("buffer_ms".to_string())
                .or_insert_with(|| toml::Value::Integer(40));
            if let Some(target) = table.get_mut("target").and_then(|v| v.as_table_mut()) {
                target.insert(
                    "sample_format".to_string(),
                    toml::Value::String("f32".to_string()),
                );
            }
        }
    }
    Ok(backend)
}

/// Plays through an exclusive ALSA device.
fn play_alsa(
    args: &PlayArgs,
    settings: &Settings,
    mut input: Box<dyn AudioInput>,
    input_rate: PcmRateHz,
    pipeline: OfflinePipeline,
) -> Result<(), CliError> {
    let plan = pipeline.plan().clone();
    let target_spec = plan.stream_spec.target;

    // 5. Build ALSA Output Driver
    let alsa_config = AlsaOutputConfig::from_settings(settings, &target_spec)
        .map_err(|e| CliError::Other(format!("failed to configure ALSA output: {e}")))?;
    // Opened before the banner: DoP `auto` only knows its word once the device is open.
    let mut alsa_output = AlsaAudioOutput::open(alsa_config)
        .map_err(|e| CliError::Other(format!("ALSA open failed: {e}")))?;

    if !args.json {
        eprintln!("audigma: starting hardware playback");
        eprintln!(
            "  Input:         {} ({} Hz, {} channels)",
            args.input,
            input_rate.get(),
            input.info().original_channels
        );
        eprintln!("  Device:        {}", alsa_output.device());
        eprintln!(
            "  ALSA Format:   {} @ {} frames/s",
            alsa_output.alsa_format(),
            alsa_output.frame_rate_hz()
        );
        match target_spec {
            TargetRateSpec::Dsd { bit_rate } => {
                let transport = settings
                    .output
                    .as_ref()
                    .and_then(|o| o.transport)
                    .unwrap_or(OutputTransport::Native);
                eprintln!(
                    "  Target:        DSD ({:.4} MHz) via {:?}",
                    bit_rate.get() as f64 / 1_000_000.0,
                    transport
                );
            }
            TargetRateSpec::Pcm {
                rate,
                sample_format,
                ..
            } => {
                eprintln!(
                    "  Target:        PCM {} Hz ({:?})",
                    rate.get(),
                    sample_format
                );
            }
        }
        eprintln!("  Filter:        {}", plan.dsp_config.filter_description());
        eprintln!(
            "  Gain:          preamp {:.2} dB + volume {:.2} dB + headroom {:.2} dB = {:.2} dB",
            plan.dsp_config.preamp_db,
            plan.dsp_config.user_volume_db,
            plan.dsp_config.profile_headroom_db,
            plan.dsp_config.total_configured_gain_db()
        );
    }

    // 6. Click-free Priming (in the DoP word the device was opened with)
    let carrier = IdleCarrierFormat::from_settings(settings, &target_spec).for_output(&alsa_output);

    let mut lifecycle = LifecycleController::new(carrier, 256, 256);
    let prime_frames = (alsa_output.period_frames() * 2) as usize;
    let prime_bytes = prime_frames * alsa_output.frame_bytes();
    let mut prime_buf = vec![0u8; prime_bytes];
    lifecycle.emit_idle_frames(&mut prime_buf);
    let _ = alsa_output.write_all(&prime_buf);

    // 7. Stream audio through pipeline
    // Passed as itself, not wrapped, so the pipeline's DoP packer sees the negotiated word.
    let report = pipeline.run(&mut *input, Box::new(&mut alsa_output))?;

    // 8. Drain hardware
    alsa_output
        .drain()
        .map_err(|e| CliError::Other(format!("ALSA drain failed: {e}")))?;

    let xruns = alsa_output.xrun_count();

    if args.json {
        let json_val = json!({
            "device": alsa_output.device(),
            "alsa_format": alsa_output.alsa_format(),
            "frame_rate_hz": alsa_output.frame_rate_hz(),
            "total_input_frames": report.total_input_frames,
            "total_output_frames": alsa_output.frames_written(),
            "total_bytes_written": alsa_output.bytes_written(),
            "xrun_count": xruns,
            "applied_gain_db": report.applied_gain_db,
            "overload_count": report.overload_count,
            "elapsed_ms": report.elapsed_ms,
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        eprintln!("audigma: playback finished cleanly.");
        eprintln!(
            "  Input Frames:    {} ({:.2} s)",
            report.total_input_frames,
            report.total_input_frames as f64 / input_rate.get() as f64
        );
        eprintln!(
            "  Output Frames:   {} ({} bytes written)",
            alsa_output.frames_written(),
            alsa_output.bytes_written()
        );
        eprintln!("  XRuns / Glitches: {}", xruns);
        eprintln!("  Elapsed Time:    {} ms", report.elapsed_ms);
    }

    Ok(())
}

/// Plays PCM into the PipeWire graph (the graph, not audigma, owns the device).
#[cfg(feature = "pipewire")]
fn play_pipewire(
    args: &PlayArgs,
    settings: &Settings,
    mut input: Box<dyn AudioInput>,
    input_rate: PcmRateHz,
    pipeline: OfflinePipeline,
) -> Result<(), CliError> {
    let plan = pipeline.plan().clone();
    let TargetRateSpec::Pcm { rate, .. } = plan.stream_spec.target else {
        return Err(CliError::Config(
            "output.backend = \"pipewire\" cannot carry a DSD target; use backend = \"alsa\""
                .into(),
        ));
    };
    let out_cfg = settings.output.as_ref();
    let buffer_ms = out_cfg.and_then(|o| o.buffer_ms).unwrap_or(40) as usize;
    let target_node = out_cfg.and_then(|o| o.target_node.clone());
    // Same queue sizing as `audigma run` (~64 frames per tile, 64..=256 tiles).
    let tiles_per_sec = (rate.get() as f64 / 64.0).ceil() as usize;
    let queue_depth = (tiles_per_sec * buffer_ms / 1000).clamp(64, 256);
    let placement = crate::runtime::resolve(settings, 2, true)?;
    let mut pw_out = PipeWirePcmOutput::new(
        "audigma_play",
        target_node.as_deref(),
        rate.get(),
        2,
        queue_depth,
        Some(placement.pipewire()),
    )
    .map_err(|e| CliError::Other(format!("failed to initialize PipeWire output: {e}")))?;

    if !args.json {
        eprintln!("audigma: starting PipeWire playback");
        eprintln!(
            "  Input:         {} ({} Hz, {} channels)",
            args.input,
            input_rate.get(),
            input.info().original_channels
        );
        eprintln!(
            "  Node:          {}",
            target_node.as_deref().unwrap_or("(default sink)")
        );
        eprintln!("  Format:        f32 @ {} Hz", rate.get());
        eprintln!("  Filter:        {}", plan.dsp_config.filter_description());
        eprintln!(
            "  Gain:          preamp {:.2} dB + volume {:.2} dB + headroom {:.2} dB = {:.2} dB",
            plan.dsp_config.preamp_db,
            plan.dsp_config.user_volume_db,
            plan.dsp_config.profile_headroom_db,
            plan.dsp_config.total_configured_gain_db()
        );
    }

    let report = pipeline.run(&mut *input, Box::new(&mut pw_out))?;
    pw_out.drain(std::time::Duration::from_millis(2000 + buffer_ms as u64));
    let xruns = pw_out.xrun_count();

    if args.json {
        let json_val = json!({
            "node": target_node,
            "format": "f32",
            "frame_rate_hz": rate.get(),
            "total_input_frames": report.total_input_frames,
            "total_output_frames": report.total_output_frames,
            "total_bytes_written": report.total_bytes_written,
            "xrun_count": xruns,
            "applied_gain_db": report.applied_gain_db,
            "overload_count": report.overload_count,
            "elapsed_ms": report.elapsed_ms,
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        eprintln!("audigma: playback finished cleanly.");
        eprintln!(
            "  Input Frames:    {} ({:.2} s)",
            report.total_input_frames,
            report.total_input_frames as f64 / input_rate.get() as f64
        );
        eprintln!(
            "  Output Frames:   {} ({} bytes written)",
            report.total_output_frames, report.total_bytes_written
        );
        eprintln!("  XRuns / Glitches: {}", xruns);
        eprintln!("  Elapsed Time:    {} ms", report.elapsed_ms);
    }
    Ok(())
}

#[cfg(not(feature = "pipewire"))]
fn play_pipewire(
    _args: &PlayArgs,
    _settings: &Settings,
    _input: Box<dyn AudioInput>,
    _input_rate: PcmRateHz,
    _pipeline: OfflinePipeline,
) -> Result<(), CliError> {
    Err(CliError::Config(
        "PipeWire feature is disabled in this build".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn backend_defaults_to_alsa_with_live_defaults() {
        let mut t = table("");
        assert_eq!(
            apply_live_output_defaults(&mut t, false).unwrap(),
            OutputBackend::Alsa
        );
        let out = t["output"].as_table().unwrap();
        assert_eq!(out["backend"].as_str(), Some("alsa"));
        assert_eq!(out["transport"].as_str(), Some("native"));
        assert_eq!(out["buffer_ms"].as_integer(), Some(40));
    }

    #[test]
    fn configured_alsa_settings_are_kept() {
        let mut t = table("[output]\nbackend = \"alsa\"\ntransport = \"dop\"\nbuffer_ms = 20\n");
        apply_live_output_defaults(&mut t, true).unwrap();
        let out = t["output"].as_table().unwrap();
        assert_eq!(out["transport"].as_str(), Some("dop"));
        assert_eq!(out["buffer_ms"].as_integer(), Some(20));
    }

    #[test]
    fn pipewire_is_honoured_and_forces_f32_pcm() {
        let mut t = table(
            "[output]\nbackend = \"pipewire\"\n[target]\nformat = \"pcm\"\nsample_format = \"s24le\"\n",
        );
        assert_eq!(
            apply_live_output_defaults(&mut t, false).unwrap(),
            OutputBackend::Pipewire
        );
        assert_eq!(t["output"]["backend"].as_str(), Some("pipewire"));
        assert_eq!(t["output"]["transport"].as_str(), Some("pcm"));
        assert_eq!(t["target"]["sample_format"].as_str(), Some("f32"));
        assert!(
            t["output"]
                .as_table()
                .unwrap()
                .get("reserve_device")
                .is_none()
        );
    }

    #[test]
    fn pipewire_rejects_device_flag_and_non_pcm_transport() {
        let mut t = table("[output]\nbackend = \"pipewire\"\n");
        assert!(apply_live_output_defaults(&mut t, true).is_err());
        let mut t = table("[output]\nbackend = \"pipewire\"\ntransport = \"dop\"\n");
        assert!(apply_live_output_defaults(&mut t, false).is_err());
    }

    #[test]
    fn non_live_backend_is_rejected() {
        let mut t = table("[output]\nbackend = \"file\"\n");
        assert!(apply_live_output_defaults(&mut t, false).is_err());
    }
}
