//! Handler for `audigma play` subcommand.

use std::io::Write;
use std::path::PathBuf;

use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_dsp::presets::by_name;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::AudioInput;
use audigma_engine::{IdleCarrierFormat, LifecycleController, OfflinePipeline, StreamSink};
use audigma_io::alsa::{AlsaAudioOutput, AlsaOutputConfig};
use audigma_io::file::FileAudioInput;
use audigma_io::stdio::StdioAudioInput;
use audigma_settings::{OutputContainer, OutputTransport, Settings};
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

    /// Resampler & filter preset ("balanced", "reference", "low-latency")
    #[arg(short = 'p', long = "preset")]
    pub preset: Option<String>,

    /// Optional playback volume in dB (e.g. -3.0) or percentage (e.g. 80%),
    /// using the same syntax as `audigma set volume`
    #[arg(short = 'v', long = "volume", allow_hyphen_values = true)]
    pub volume: Option<String>,

    /// Ad-hoc configuration override; the value is parsed as a TOML literal
    /// (e.g. -O output.transport=dop -O output.buffer_ms=20 -O output.device='"2"')
    #[arg(short = 'O', long = "override")]
    pub overrides: Vec<String>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
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
        let serialized = toml::to_string(&default_settings).map_err(|e| {
            CliError::Serialization(format!("failed to serialize default settings: {e}"))
        })?;
        toml::from_str(&serialized)
            .map_err(|e| CliError::Config(format!("failed to parse default TOML: {e}")))?
    };

    // 2. Configure output as exclusive ALSA Raw container
    let output_entry = toml_table
        .entry("output".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(output_tbl) = output_entry {
        // Live hardware playback always uses ALSA and raw stream container
        output_tbl.insert(
            "backend".to_string(),
            toml::Value::String("alsa".to_string()),
        );
        output_tbl.insert(
            "container".to_string(),
            toml::Value::String("raw".to_string()),
        );
        output_tbl
            .entry("transport".to_string())
            .or_insert_with(|| toml::Value::String("native".to_string()));
        output_tbl
            .entry("buffer_ms".to_string())
            .or_insert_with(|| toml::Value::Integer(40));
        output_tbl
            .entry("period_ms".to_string())
            .or_insert_with(|| toml::Value::Integer(5));
        output_tbl
            .entry("reserve_device".to_string())
            .or_insert_with(|| toml::Value::Boolean(true));
    }

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

    // 3. Apply common CLI ergonomics (device, preset). Volume is parsed here but
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
    if let Some(ref p) = args.preset {
        let preset = by_name(p).ok_or_else(|| CliError::PresetNotFound { name: p.clone() })?;
        let preset_str = match preset.preset {
            audigma_dsp::FilterPreset::Balanced => "balanced",
            audigma_dsp::FilterPreset::Reference => "reference",
            audigma_dsp::FilterPreset::LowLatency => "low-latency",
        };
        apply_toml_override(&mut toml_table, "resampler.preset", preset_str)?;
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

    // 5. Deserialize merged TOML table into typed Settings and validate
    let settings_str = toml::to_string(&toml_table)
        .map_err(|e| CliError::Serialization(format!("failed to serialize merged TOML: {e}")))?;
    let settings = Settings::from_toml_str(&settings_str)?;
    settings.validate()?;

    // 3. Open AudioInput
    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if args.input == "-" {
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
        let dsd_preset = settings.resolved_dsd_decimation_preset();
        let max_intermediate = settings.resolved_dsd_max_intermediate_rate_hz();
        let file_input = FileAudioInput::open_with_dsd_full_config(
            &args.input,
            dsd_gain_db,
            dsd_preset,
            max_intermediate,
        )?;
        let rate = file_input.info().sample_rate;
        (Box::new(file_input), rate)
    };

    // 4. Prepare Pipeline and layer the CLI volume on top of the configured preamp
    let pipeline = {
        let placement = crate::runtime::resolve(&settings, 2, true)?;
        crate::runtime::apply_housekeeping(&placement);
        OfflinePipeline::prepare_with_workers(
            settings.clone(),
            input_rate,
            Some(crate::runtime::worker_setup(&placement)),
        )?
    };
    let pipeline = match user_volume_db {
        Some(db) => pipeline.with_user_volume_db(db),
        None => pipeline,
    };
    let plan = pipeline.plan().clone();
    let target_spec = plan.stream_spec.target;

    // 5. Build ALSA Output Driver
    let alsa_config = AlsaOutputConfig::from_settings(&settings, &target_spec)
        .map_err(|e| CliError::Other(format!("failed to configure ALSA output: {e}")))?;

    if !args.json {
        eprintln!("audigma: starting hardware playback");
        eprintln!(
            "  Input:         {} ({} Hz, {} channels)",
            args.input,
            input_rate.get(),
            input.info().original_channels
        );
        eprintln!("  Device:        {}", alsa_config.device);
        eprintln!(
            "  ALSA Format:   {} @ {} frames/s",
            alsa_config.alsa_format, alsa_config.frame_rate_hz
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
        eprintln!(
            "  Filter Preset: {:?} (phase: {:?})",
            plan.dsp_config.preset, plan.dsp_config.phase
        );
        eprintln!(
            "  Gain:          preamp {:.2} dB + volume {:.2} dB + headroom {:.2} dB = {:.2} dB",
            plan.dsp_config.preamp_db,
            plan.dsp_config.user_volume_db,
            plan.dsp_config.profile_headroom_db,
            plan.dsp_config.total_configured_gain_db()
        );
    }

    let mut alsa_output = AlsaAudioOutput::open(alsa_config)
        .map_err(|e| CliError::Other(format!("ALSA open failed: {e}")))?;

    // 6. Click-free Priming
    let carrier = IdleCarrierFormat::from_settings(&settings, &target_spec);

    let mut lifecycle = LifecycleController::new(carrier, 256, 256);
    let prime_frames = (alsa_output.period_frames() * 2) as usize;
    let prime_bytes = prime_frames * alsa_output.frame_bytes();
    let mut prime_buf = vec![0u8; prime_bytes];
    lifecycle.emit_idle_frames(&mut prime_buf);
    let _ = alsa_output.write_all(&prime_buf);

    // 7. Stream audio through pipeline
    let report = pipeline.run(&mut *input, Box::new(StreamSink(&mut alsa_output)))?;

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
