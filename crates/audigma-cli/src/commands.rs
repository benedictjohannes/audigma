//! CLI subcommands and commands for `audigma` (`Design.md §7`).

use crate::ipc::{
    ControlClient, ControlServer, IpcError, IpcRequest, IpcResponse, default_socket_path,
};
use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_dsp::presets::{ALL_PRESETS, by_name};
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::live::{DaemonControlHandle, LivePipeline};
use audigma_engine::traits::{AudioInput, AudioOutput, DecoderInput, EngineError};
use audigma_engine::{
    ChannelWorkerPool, IdleCarrierFormat, LifecycleController, MockAudioInput, OfflinePipeline,
    StreamExitReason, StreamSink, TargetDsdConfig,
};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_format::dop::DopContainerFormat;
use audigma_io::alsa::{
    AlsaAudioOutput, AlsaHardwareMixer, AlsaOutputConfig, is_alsa_device_present,
    resolve_card_index, wait_for_alsa_device,
};

#[cfg(feature = "dbus")]
use audigma_io::dbus::{DeviceReservation, request_realtime_priority};
use audigma_io::file::{FileAudioInput, FileAudioOutput};
#[cfg(feature = "pipewire")]
use audigma_io::pipewire::{PipeWirePcmOutput, PipeWireVirtualSink};
use audigma_io::stdio::{StdioAudioInput, StdioAudioOutput};
use audigma_settings::{InputBackend, OutputBackend, OutputContainer, OutputTransport, Settings};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::f64::consts::PI;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "audigma",
    version,
    about = "High-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD) engine",
    long_about = "audigma is a high-precision Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run live audio streaming daemon in foreground
    Run(RunArgs),

    /// Request orderly shutdown of a running audigma daemon
    Stop(StopArgs),

    /// Query live daemon runtime status and telemetry
    Status(StatusArgs),

    /// Modify daemon volume or preset dynamically
    Set(SetArgs),

    /// Play audio directly to hardware via exclusive ALSA
    Play(PlayArgs),

    /// Convert an audio file or stream offline
    Convert(ConvertArgs),

    /// Run DSP throughput and execution budget benchmarks
    Benchmark(BenchmarkArgs),

    /// Inspect and display DSP presets
    Presets(PresetsArgs),

    /// Validate or inspect configuration files
    Config(ConfigArgs),

    /// Probe audio hardware endpoints and supported formats/rates
    Probe(ProbeArgs),
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Optional configuration file path (defaults to ~/.config/audigma/config.toml)
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct StopArgs {
    /// Optional control socket path override
    #[arg(long = "socket")]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Continuously watch daemon status telemetry
    #[arg(short = 'w', long = "watch")]
    pub watch: bool,

    /// Output report in structured JSON format
    #[arg(long = "json")]
    pub json: bool,

    /// Optional control socket path override
    #[arg(long = "socket")]
    pub socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    #[command(subcommand)]
    pub command: SetCommands,

    /// Optional control socket path override
    #[arg(long = "socket")]
    pub socket: Option<PathBuf>,
}

/// Parses a volume argument into decibels (dB) according to confirmed rules:
/// - Suffix `%` (e.g. `80%`, `100%`, `0%`): perceptual percentage converted via ALSA's volume_mapping.c curve.
/// - No `%` (e.g. `-6`, `-3.2`, `0`, `0.0`, `+1.5`, `-6dB`): treated directly as decibels.
pub fn parse_volume_to_db(arg: &str) -> Result<f64, String> {
    let s = arg.trim();
    if s.is_empty() {
        return Err("volume argument cannot be empty".to_string());
    }

    if let Some(pct_str) = s.strip_suffix('%') {
        let p: f64 = pct_str
            .trim()
            .parse()
            .map_err(|e| format!("invalid percentage volume '{arg}': {e}"))?;
        if p < 0.0 {
            return Err("percentage volume cannot be negative".to_string());
        }
        Ok(audigma_dsp::alsamixer_percentage_to_db(
            p,
            audigma_dsp::DEFAULT_SOFTWARE_MIN_DB,
        ))
    } else {
        let clean = s
            .strip_suffix("dB")
            .or_else(|| s.strip_suffix("db"))
            .unwrap_or(s)
            .trim();
        let db: f64 = clean
            .parse()
            .map_err(|e| format!("invalid decibel volume '{arg}': {e}"))?;
        Ok(db)
    }
}

#[derive(Subcommand, Debug)]
pub enum SetCommands {
    /// Set live playback volume in dB (e.g. -3.0 or 0) or percentage (e.g. 80% or 0%)
    Volume {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },

    /// Switch active DSP preset (balanced, reference, low-latency)
    Preset { name: String },
}

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

    /// Optional playback volume/preamp in dB (e.g. -3.0 or --volume=-3.0)
    #[arg(short = 'v', long = "volume", allow_hyphen_values = true)]
    pub volume: Option<f64>,

    /// Ad-hoc configuration override (e.g. -o output.transport=dop -o output.buffer_ms=20)
    #[arg(short = 'o', long = "override")]
    pub overrides: Vec<String>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Emit JSON status report at playback completion
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ProbeArgs {
    /// Audio backend to probe ("alsa" or "pipewire")
    #[arg(short = 'b', long = "backend", default_value = "alsa")]
    pub backend: String,

    /// Output report in structured JSON format
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct ConvertArgs {
    /// Input file path (or '-' for stdin)
    #[arg(short = 'i', long = "input")]
    pub input: String,

    /// Output file path (or '-' for stdout)
    #[arg(short = 'o', long = "output")]
    pub output: String,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Emit JSON report at the end of conversion
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BenchmarkArgs {
    /// Optional input audio file (if omitted, uses a deterministic 10s synthetic stereo signal)
    #[arg(short = 'i', long = "input")]
    pub input: Option<PathBuf>,

    /// Resampler & filter preset ("balanced", "reference", "low-latency")
    #[arg(short = 'p', long = "preset")]
    pub preset: Option<String>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Emit benchmark results in JSON format
    #[arg(long = "json")]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct PresetsArgs {
    #[command(subcommand)]
    pub command: PresetsCommands,
}

#[derive(Subcommand, Debug)]
pub enum PresetsCommands {
    /// List all built-in DSP presets
    List,

    /// Show detailed specifications of a specific preset
    Show {
        /// Name of the preset (balanced, reference, low-latency)
        name: String,
    },
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommands,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Validate configuration file syntax and invariants
    Check {
        /// Configuration file path (defaults to ~/.config/audigma/config.toml)
        #[arg(short = 'c', long = "config")]
        config: Option<PathBuf>,
    },

    /// Explain the resolved DSP plan for an input sample rate
    Explain {
        /// Configuration file path (defaults to ~/.config/audigma/config.toml)
        #[arg(short = 'c', long = "config")]
        config: Option<PathBuf>,

        /// Input PCM sample rate in Hz (defaults to 44100)
        #[arg(long = "input-rate", default_value = "44100")]
        input_rate: u32,

        /// Output explanation in JSON format
        #[arg(long = "json")]
        json: bool,
    },
}

fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("audigma/config.toml")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/audigma/config.toml")
    } else {
        PathBuf::from("config.toml")
    }
}

/// Recursively sets a dotted key (e.g. "output.transport" or "gain.preamp_db") on a `toml::Table`.
fn apply_toml_override(
    root: &mut toml::Table,
    key_path: &str,
    raw_value: &str,
) -> Result<(), EngineError> {
    let parts: Vec<&str> = key_path.split('.').collect();
    if parts.is_empty() {
        return Err(EngineError::Pipeline("empty override key".to_string()));
    }

    // Parse the value: try integer, float, boolean, or fallback to string
    let parsed_val: toml::Value = if let Ok(i) = raw_value.parse::<i64>() {
        toml::Value::Integer(i)
    } else if let Ok(f) = raw_value.parse::<f64>() {
        toml::Value::Float(f)
    } else if let Ok(b) = raw_value.parse::<bool>() {
        toml::Value::Boolean(b)
    } else {
        // Strip optional surrounding quotes
        let trimmed = raw_value.trim();
        let unquoted = if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };
        toml::Value::String(unquoted.to_string())
    };

    let mut current = root;
    for (i, &part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            current.insert(part.to_string(), parsed_val);
            return Ok(());
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        match entry {
            toml::Value::Table(tbl) => current = tbl,
            _ => {
                return Err(EngineError::Pipeline(format!(
                    "cannot set nested key '{key_path}': segment '{part}' is not a table"
                )));
            }
        }
    }

    Ok(())
}

pub fn run_run(args: RunArgs) -> Result<(), EngineError> {
    let config_path = resolve_config_path(args.config.as_deref());
    if !config_path.exists() {
        return Err(EngineError::Pipeline(format!(
            "configuration file not found at: {}",
            config_path.display()
        )));
    }

    let content = std::fs::read_to_string(&config_path)?;
    let settings = Settings::from_toml_str(&content)?;
    settings.validate()?;

    let in_cfg = settings
        .input
        .as_ref()
        .ok_or_else(|| EngineError::Pipeline("missing [input] section in config".to_string()))?;
    let out_cfg = settings
        .output
        .as_ref()
        .ok_or_else(|| EngineError::Pipeline("missing [output] section in config".to_string()))?;

    let input_rate = in_cfg.rate_hz.unwrap_or(PcmRateHz::PCM_48000);
    let channels = in_cfg.channels.unwrap_or(2);

    let plan = settings.explain_for_input_rate(input_rate)?;
    let target_spec = plan.stream_spec.target;

    eprintln!("audigma: starting live daemon");
    eprintln!("  Configuration: {}", config_path.display());
    eprintln!(
        "  Input:         {:?} ({} Hz, {} channels)",
        in_cfg.backend,
        input_rate.get(),
        channels
    );
    eprintln!("  Output:        {:?}", out_cfg.backend);
    eprintln!("  Preset:        {:?}", plan.dsp_config.preset);

    // D-Bus device reservation if exclusive ALSA output is selected
    #[cfg(feature = "dbus")]
    let mut _reservation: Option<DeviceReservation> =
        if out_cfg.backend == OutputBackend::Alsa && out_cfg.reserve_device.unwrap_or(true) {
            let card_idx = out_cfg
                .device
                .as_deref()
                .and_then(resolve_card_index)
                .unwrap_or(0);

            match DeviceReservation::acquire(card_idx, "audigma", 100) {
                Ok(res) => {
                    eprintln!(
                        "  DeviceLock:    Acquired {} via D-Bus org.freedesktop.ReserveDevice1",
                        res.well_known_name()
                    );
                    Some(res)
                }
                Err(e) => {
                    eprintln!("  DeviceLock:    Warning: failed to acquire D-Bus reservation: {e}");
                    None
                }
            }
        } else {
            None
        };

    // Real-time thread priority
    #[cfg(feature = "dbus")]
    {
        if let Err(e) = request_realtime_priority(0, 10) {
            eprintln!("  RT Priority:   RealtimeKit request skipped/fallback: {e}");
        } else {
            eprintln!("  RT Priority:   Acquired real-time priority via RealtimeKit");
        }
    }

    // Set up Input Backend helper closure
    let create_input_driver =
        |in_cfg: &audigma_settings::InputConfig| -> Result<Box<dyn AudioInput>, EngineError> {
            match in_cfg.backend {
                #[cfg(feature = "pipewire")]
                InputBackend::Pipewire => {
                    let sink = PipeWireVirtualSink::from_input_config(in_cfg, input_rate.get(), 32)
                        .map_err(|e| {
                            EngineError::Pipeline(format!(
                                "failed to bind PipeWire virtual sink: {e}"
                            ))
                        })?;
                    Ok(Box::new(sink))
                }
                InputBackend::Alsa => Err(EngineError::Pipeline(
                    "ALSA live capture input not yet configured".to_string(),
                )),
                InputBackend::File | InputBackend::Stdio => Err(EngineError::Pipeline(
                    "File/Stdio backend is for offline mode; use 'audigma convert'".to_string(),
                )),
                #[cfg(not(feature = "pipewire"))]
                InputBackend::Pipewire => Err(EngineError::Pipeline(
                    "PipeWire feature is disabled in this build".to_string(),
                )),
            }
        };

    let mut input_driver: Option<Box<dyn AudioInput>> = Some(create_input_driver(in_cfg)?);

    // Helper closure to create/open output backend driver
    let create_output_driver = |out_cfg: &audigma_settings::OutputConfig,
                                target: &TargetRateSpec|
     -> Result<Box<dyn AudioOutput>, EngineError> {
        match out_cfg.backend {
            OutputBackend::Alsa => {
                let alsa_config =
                    AlsaOutputConfig::from_settings(&settings, target).map_err(|e| {
                        EngineError::Pipeline(format!("failed to configure ALSA output: {e}"))
                    })?;
                let alsa_out = AlsaAudioOutput::open(alsa_config).map_err(|e| {
                    EngineError::Pipeline(format!("failed to open ALSA output: {e}"))
                })?;
                Ok(Box::new(alsa_out))
            }
            #[cfg(feature = "pipewire")]
            OutputBackend::Pipewire => {
                let target_node = out_cfg.target_node.as_deref();
                let pw_rate = match target {
                    TargetRateSpec::Pcm { rate, .. } => rate.get(),
                    TargetRateSpec::Dsd { bit_rate } => {
                        return Err(EngineError::Pipeline(format!(
                            "PipeWire PCM output cannot carry DSD bit rate {bit_rate}"
                        )));
                    }
                };
                let pw_out = PipeWirePcmOutput::new(
                    "audigma_pcm_out",
                    target_node,
                    pw_rate,
                    channels as u32,
                    32,
                )
                .map_err(|e| {
                    EngineError::Pipeline(format!("failed to initialize PipeWire output: {e}"))
                })?;
                Ok(Box::new(pw_out))
            }
            #[cfg(not(feature = "pipewire"))]
            OutputBackend::Pipewire => Err(EngineError::Pipeline(
                "PipeWire feature is disabled in this build".to_string(),
            )),
            OutputBackend::File | OutputBackend::Stdio => Err(EngineError::Pipeline(
                "File/Stdio backend is for offline mode; use 'audigma convert'".to_string(),
            )),
        }
    };

    let (out_fmt_str, out_rate_hz) = match target_spec {
        TargetRateSpec::Dsd { bit_rate } => ("dsd".to_string(), bit_rate.get()),
        TargetRateSpec::Pcm { rate, .. } => ("pcm".to_string(), rate.get()),
    };

    let control = DaemonControlHandle::new(
        input_rate.get(),
        &out_fmt_str,
        out_rate_hz,
        match plan.dsp_config.preset {
            audigma_dsp::config::FilterPreset::Balanced => "balanced",
            audigma_dsp::config::FilterPreset::Reference => "reference",
            audigma_dsp::config::FilterPreset::LowLatency => "low-latency",
        },
        plan.dsp_config.preamp_db,
    );

    // Start Unix Control Socket
    let socket_path = default_socket_path();
    let _server = ControlServer::start(socket_path.clone(), control.clone())
        .map_err(|e| EngineError::Pipeline(format!("failed to start control socket: {e}")))?;
    eprintln!("  ControlSocket: {}", socket_path.display());

    // Prepare and Run Live Pipeline
    let mut pipeline = LivePipeline::prepare(settings.clone(), input_rate)?;
    let mut current_input_rate = input_rate;
    let mut current_target_spec = target_spec;
    let mut generation: u64 = 1;
    eprintln!("audigma: daemon running. Press Ctrl+C or run 'audigma stop' to exit.");

    let target_device = out_cfg
        .device
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let custom_mixer_control = out_cfg.mixer_control();
    let vol_control_mode = out_cfg.volume_control();
    let device_wait_ms = out_cfg.device_wait_ms();
    let keep_audio_sink_ms = out_cfg.keep_audio_sink_ms();

    let mut hw_mixer: Option<AlsaHardwareMixer> = if out_cfg.backend == OutputBackend::Alsa {
        match AlsaHardwareMixer::open(&target_device, custom_mixer_control) {
            Ok(m) => {
                eprintln!(
                    "  HardwareMixer: Bound to ALSA mixer control '{}'",
                    m.element_name()
                );
                Some(m)
            }
            Err(e) => {
                eprintln!("  HardwareMixer: Note: hardware mixer unavailable ({e})");
                None
            }
        }
    } else {
        None
    };

    let active_vol_mode = match vol_control_mode {
        audigma_settings::VolumeControlMode::HardwareSoftwareFallback => {
            if hw_mixer.is_some() {
                "hardware"
            } else {
                "software"
            }
        }
        audigma_settings::VolumeControlMode::Hardware => {
            if hw_mixer.is_none() {
                eprintln!(
                    "  VolumeControl: Warning: 'hardware' volume requested but no mixer element found; digital gain locked at 0 dBFS"
                );
            }
            "hardware"
        }
        audigma_settings::VolumeControlMode::Software => "software",
        audigma_settings::VolumeControlMode::None => "none",
    };
    control.set_volume_mode(active_vol_mode);
    eprintln!(
        "  VolumeControl: {} (configured: {:?})",
        active_vol_mode, vol_control_mode
    );

    let mut current_output: Option<Box<dyn AudioOutput>> = None;

    // Resilient daemon loop
    while control.is_running() {
        // Step 1: Ensure output device is connected and opened
        if current_output.is_none() {
            if out_cfg.backend == OutputBackend::Alsa && !is_alsa_device_present(&target_device) {
                if device_wait_ms == 0 {
                    return Err(EngineError::DeviceDisconnected(format!(
                        "target ALSA device '{target_device}' is not present at startup"
                    )));
                }

                eprintln!(
                    "[INFO] audigma: waiting for output device '{target_device}' (device_wait_ms: {device_wait_ms}, keep_audio_sink_ms: {keep_audio_sink_ms})..."
                );
                control.set_state("WaitingForDevice");
                control.set_disconnect_info(Some(target_device.clone()), Some(0));

                let wait_start = Instant::now();
                let mut sink_dropped = false;
                let mut last_log_secs = 0u64;

                while control.is_running() {
                    let elapsed_ms = wait_start.elapsed().as_millis() as u64;
                    let elapsed_secs = elapsed_ms / 1000;
                    control.set_disconnect_info(Some(target_device.clone()), Some(elapsed_ms));

                    // Periodic waiting heartbeat every 5 seconds
                    if elapsed_secs >= last_log_secs + 5 {
                        last_log_secs = elapsed_secs;
                        eprintln!(
                            "[INFO] audigma: still waiting for '{target_device}'... ({elapsed_secs}s elapsed, sink_active: {})",
                            !sink_dropped
                        );
                    }

                    // Check device_wait_ms timeout
                    if device_wait_ms > 0 && elapsed_ms >= device_wait_ms as u64 {
                        eprintln!(
                            "[ERROR] audigma: device_wait_ms timeout ({device_wait_ms} ms) reached; exiting."
                        );
                        return Err(EngineError::DeviceDisconnected(format!(
                            "timed out waiting for device '{target_device}' after {device_wait_ms} ms"
                        )));
                    }

                    // Check keep_audio_sink_ms grace period
                    if !sink_dropped
                        && keep_audio_sink_ms >= 0
                        && elapsed_ms >= keep_audio_sink_ms as u64
                    {
                        eprintln!(
                            "[WARN] audigma: keep_audio_sink_ms ({keep_audio_sink_ms} ms) expired; unregistering virtual sink from PipeWire graph."
                        );
                        input_driver = None; // Drop virtual sink to unregister from PipeWire
                        sink_dropped = true;
                    }

                    // If virtual sink is still registered, bit-bucket drain incoming frames pacingly
                    if let Some(ref mut input) = input_driver {
                        let mut drain_left = [0.0; 256];
                        let mut drain_right = [0.0; 256];
                        let _ = input.read_stereo(&mut drain_left, &mut drain_right);
                    }

                    // Wait for inotify event or slice
                    if wait_for_alsa_device(&target_device, 200, &control.running).unwrap_or(false)
                    {
                        eprintln!(
                            "[INFO] audigma: hotplug event detected: output device '{target_device}' is present!"
                        );
                        break;
                    }
                }

                if !control.is_running() {
                    break;
                }

                // Re-register input virtual sink if it was dropped
                if input_driver.is_none() {
                    eprintln!("[INFO] audigma: re-registering virtual sink in PipeWire graph...");
                    input_driver = Some(create_input_driver(in_cfg)?);
                }

                // Re-acquire D-Bus device reservation if needed
                #[cfg(feature = "dbus")]
                if out_cfg.reserve_device.unwrap_or(true) {
                    let card_idx = resolve_card_index(&target_device).unwrap_or(0);
                    eprintln!(
                        "[INFO] audigma: re-acquiring D-Bus reservation for card {card_idx}..."
                    );
                    match DeviceReservation::acquire(card_idx, "audigma", 100) {
                        Ok(res) => {
                            eprintln!(
                                "[INFO] audigma: acquired {} via D-Bus org.freedesktop.ReserveDevice1",
                                res.well_known_name()
                            );
                            _reservation = Some(res);
                        }
                        Err(e) => {
                            eprintln!(
                                "[WARN] audigma: failed to re-acquire D-Bus reservation: {e}"
                            );
                        }
                    }
                }
            }

            // Open output device
            match create_output_driver(out_cfg, &current_target_spec) {
                Ok(out) => {
                    eprintln!(
                        "[INFO] audigma: opened output device '{target_device}'; resuming streaming."
                    );
                    current_output = Some(out);
                    control.set_disconnect_info(None, None);

                    // Re-bind ALSA hardware mixer if applicable
                    if out_cfg.backend == OutputBackend::Alsa {
                        hw_mixer =
                            match AlsaHardwareMixer::open(&target_device, custom_mixer_control) {
                                Ok(mut m) => {
                                    eprintln!(
                                        "[INFO] audigma: re-bound ALSA hardware mixer '{}'",
                                        m.element_name()
                                    );
                                    let cur_vol = control.get_volume();
                                    let is_muted = control.is_muted();
                                    let _ = m.set_volume_db(cur_vol);
                                    let _ = m.set_mute(is_muted);
                                    Some(m)
                                }
                                Err(_) => None,
                            };
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[WARN] audigma: failed to open output device '{target_device}': {e}; retrying in 500ms..."
                    );
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }

        // Ensure input driver is active
        if input_driver.is_none() {
            input_driver = Some(create_input_driver(in_cfg)?);
        }

        let input = input_driver.as_mut().unwrap();
        let output = current_output.as_mut().unwrap();

        // Run streaming loop
        match pipeline.run_streaming_with_bridge(
            &mut **input,
            &mut **output,
            control.clone(),
            hw_mixer
                .as_mut()
                .map(|m| m as &mut dyn audigma_engine::HardwareVolumeBridge),
        ) {
            Ok(StreamExitReason::Stopped) => break,
            Ok(StreamExitReason::RateChanged {
                new_rate,
                channels: _,
            }) => {
                eprintln!(
                    "[INFO] audigma: dynamic sample rate switch detected: {} Hz -> {} Hz",
                    current_input_rate, new_rate
                );
                current_input_rate = new_rate;

                // Check if output rate or ALSA configuration must change
                let new_plan = settings.explain_for_input_rate(new_rate)?;
                let new_target_spec = new_plan.stream_spec.target;

                let need_reopen_output = match (current_target_spec, new_target_spec) {
                    (
                        TargetRateSpec::Dsd { bit_rate: old_br },
                        TargetRateSpec::Dsd { bit_rate: new_br },
                    ) => old_br != new_br,
                    (
                        TargetRateSpec::Pcm { rate: old_r, .. },
                        TargetRateSpec::Pcm { rate: new_r, .. },
                    ) => old_r != new_r,
                    _ => true,
                };

                if need_reopen_output && out_cfg.backend == OutputBackend::Alsa {
                    eprintln!("[INFO] audigma: re-locking ALSA device for new target rate...");
                    current_output = None;
                }

                // Re-prepare the LivePipeline for the new input rate
                pipeline = LivePipeline::prepare(settings.clone(), new_rate)?;
                current_target_spec = new_target_spec;

                // Bump generation and update active rate on virtual sink
                generation += 1;
                control.generation.store(generation, Ordering::SeqCst);
                input.set_generation(generation);
                input.set_active_rate(new_rate);

                // Update telemetry
                {
                    let mut telem = control.telemetry.lock().unwrap();
                    telem.input_rate_hz = new_rate.get();
                    match new_target_spec {
                        TargetRateSpec::Dsd { bit_rate } => {
                            telem.output_format = "dsd".to_string();
                            telem.output_rate_hz = bit_rate.get();
                        }
                        TargetRateSpec::Pcm { rate, .. } => {
                            telem.output_format = "pcm".to_string();
                            telem.output_rate_hz = rate.get();
                        }
                    }
                }

                continue;
            }
            Err(EngineError::DeviceDisconnected(reason)) => {
                eprintln!("[WARN] audigma: output device disconnected: {reason}");
                current_output = None; // Force output driver teardown
                hw_mixer = None;
                #[cfg(feature = "dbus")]
                {
                    eprintln!(
                        "[INFO] audigma: releasing D-Bus reservation for disconnected device."
                    );
                    _reservation = None; // Release D-Bus reservation while device is gone
                }
                control.set_state("WaitingForDevice");
                control.set_disconnect_info(Some(target_device.clone()), Some(0));
            }

            Err(e) => {
                eprintln!("[ERROR] audigma: runtime streaming error: {e}");
                return Err(e);
            }
        }
    }

    eprintln!("audigma: daemon stopped cleanly.");
    Ok(())
}

pub fn run_stop(args: StopArgs) -> Result<(), EngineError> {
    let socket_path = args.socket.unwrap_or_else(default_socket_path);
    match ControlClient::send(&socket_path, IpcRequest::Stop) {
        Ok(IpcResponse::Ok) => {
            eprintln!("audigma: stop request sent to daemon.");
            Ok(())
        }
        Ok(IpcResponse::Error(msg)) => Err(EngineError::Pipeline(format!(
            "daemon returned error: {msg}"
        ))),
        Ok(_) => Ok(()),
        Err(IpcError::DaemonNotRunning(_)) => {
            eprintln!("audigma: daemon is not running.");
            Ok(())
        }
        Err(e) => Err(EngineError::Pipeline(format!("failed to stop daemon: {e}"))),
    }
}

pub fn run_status(args: StatusArgs) -> Result<(), EngineError> {
    let socket_path = args.socket.unwrap_or_else(default_socket_path);

    loop {
        match ControlClient::send(&socket_path, IpcRequest::GetStatus) {
            Ok(IpcResponse::Telemetry(telem)) => {
                if args.json {
                    let json_val = serde_json::to_string_pretty(&telem)
                        .map_err(|e| EngineError::Pipeline(e.to_string()))?;
                    println!("{json_val}");
                } else {
                    println!("Audigma Daemon Status:");
                    println!("  State:               {}", telem.state);
                    if let Some(ref target) = telem.device_target {
                        println!("  Device Target:       {}", target);
                    }
                    if let Some(dur) = telem.disconnect_duration_ms {
                        println!("  Disconnected For:    {} ms", dur);
                    }
                    println!("  Uptime:              {}s", telem.uptime_secs);

                    println!("  Input Rate:          {} Hz", telem.input_rate_hz);
                    println!(
                        "  Output Target:       {} @ {} Hz",
                        telem.output_format, telem.output_rate_hz
                    );
                    println!("  Active Preset:       {}", telem.preset);
                    let vol_pct = telem.volume_percent;
                    let mute_str = if telem.is_muted { " [MUTED]" } else { "" };
                    println!(
                        "  Volume:              {:.1} dB ({:.0}%){mute_str} (mode: {}, preamp: {:.1} dB)",
                        telem.volume_db, vol_pct, telem.volume_mode, telem.preamp_db
                    );
                    println!(
                        "  Frames In / Out:     {} / {}",
                        telem.total_input_frames, telem.total_output_frames
                    );
                    println!("  Queue Occupancy:     {:.1}%", telem.queue_fill_pct);
                    println!("  ASRC Clock Drift:    {:.1} ppm", telem.drift_ppm);
                    println!("  Overload Events:     {}", telem.overload_count);
                    println!("  ALSA XRuns:          {}", telem.xrun_count);
                }
            }
            Ok(IpcResponse::Error(msg)) => {
                return Err(EngineError::Pipeline(format!(
                    "daemon returned error: {msg}"
                )));
            }
            Ok(_) => {}
            Err(IpcError::DaemonNotRunning(_)) => {
                if args.json {
                    println!("{}", json!({ "status": "stopped", "running": false }));
                } else {
                    eprintln!("audigma: daemon is not currently running (no active socket).");
                }
                return Ok(());
            }
            Err(e) => return Err(EngineError::Pipeline(format!("IPC query failed: {e}"))),
        }

        if !args.watch {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    Ok(())
}

pub fn run_set(args: SetArgs) -> Result<(), EngineError> {
    let socket_path = args.socket.unwrap_or_else(default_socket_path);
    let req = match args.command {
        SetCommands::Volume { value } => {
            let s = value.trim();
            if let Some(pct_str) = s.strip_suffix('%') {
                let p: f64 = pct_str
                    .trim()
                    .parse()
                    .map_err(|e| EngineError::Pipeline(format!("invalid percentage '{value}': {e}")))?;
                if p < 0.0 {
                    return Err(EngineError::Pipeline(
                        "percentage volume cannot be negative".to_string(),
                    ));
                }
                IpcRequest::SetVolumePercent { percent: p }
            } else {
                let volume_db = parse_volume_to_db(&value).map_err(EngineError::Pipeline)?;
                IpcRequest::SetVolume { volume_db }
            }
        }
        SetCommands::Preset { name } => IpcRequest::SetPreset { preset: name },
    };

    match ControlClient::send(&socket_path, req) {
        Ok(IpcResponse::Ok) => {
            eprintln!("audigma: setting updated successfully.");
            Ok(())
        }
        Ok(IpcResponse::Error(msg)) => Err(EngineError::Pipeline(format!("daemon error: {msg}"))),
        Ok(_) => Ok(()),
        Err(e) => Err(EngineError::Pipeline(format!(
            "failed to communicate with daemon: {e}"
        ))),
    }
}

pub fn run_play(args: PlayArgs) -> Result<(), EngineError> {
    // 1. Resolve base settings as a toml::Table for flexible layered overriding
    let default_settings = Settings::default_offline(OutputContainer::Raw);
    let mut toml_table: toml::Table = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        toml::from_str(&content)
            .map_err(|e| EngineError::Pipeline(format!("failed to parse config TOML: {e}")))?
    } else {
        let serialized = toml::to_string(&default_settings).map_err(|e| {
            EngineError::Pipeline(format!("failed to serialize default settings: {e}"))
        })?;
        toml::from_str(&serialized)
            .map_err(|e| EngineError::Pipeline(format!("failed to parse default TOML: {e}")))?
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
    // Users can still explicitly opt into two-pass using `-o gain.peak_analysis=two-pass`.
    let gain_entry = toml_table
        .entry("gain".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(gain_tbl) = gain_entry {
        gain_tbl.insert(
            "peak_analysis".to_string(),
            toml::Value::String("single-pass".to_string()),
        );
    }

    // 3. Apply common CLI ergonomics (device, volume, preset)
    if let Some(ref dev) = args.device {
        apply_toml_override(&mut toml_table, "output.device", dev)?;
    }
    if let Some(vol) = args.volume {
        apply_toml_override(&mut toml_table, "gain.preamp_db", &vol.to_string())?;
    }
    if let Some(ref p) = args.preset {
        let preset = by_name(p)
            .ok_or_else(|| EngineError::Pipeline(format!("unknown filter preset '{p}'")))?;
        let preset_str = match preset.preset {
            audigma_dsp::FilterPreset::Balanced => "balanced",
            audigma_dsp::FilterPreset::Reference => "reference",
            audigma_dsp::FilterPreset::LowLatency => "low-latency",
        };
        apply_toml_override(&mut toml_table, "resampler.preset", preset_str)?;
    }

    // 4. Apply ad-hoc CLI overrides (-o key=value)
    for ovr in &args.overrides {
        let (key, val) = ovr.split_once('=').ok_or_else(|| {
            EngineError::Pipeline(format!(
                "invalid override syntax '{ovr}' (expected key=value)"
            ))
        })?;
        apply_toml_override(&mut toml_table, key.trim(), val.trim())?;
    }

    // 5. Deserialize merged TOML table into typed Settings and validate
    let settings_str = toml::to_string(&toml_table)
        .map_err(|e| EngineError::Pipeline(format!("failed to serialize merged TOML: {e}")))?;
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
        let file_input = FileAudioInput::open(&args.input)?;
        let rate = file_input.info().sample_rate;
        (Box::new(file_input), rate)
    };

    // 4. Prepare Pipeline
    let pipeline = OfflinePipeline::prepare(settings.clone(), input_rate)?;
    let plan = pipeline.plan().clone();
    let target_spec = plan.stream_spec.target;

    // 5. Build ALSA Output Driver
    let alsa_config = AlsaOutputConfig::from_settings(&settings, &target_spec)
        .map_err(|e| EngineError::Pipeline(format!("failed to configure ALSA output: {e}")))?;

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
        if let Some(ref g) = settings.gain {
            eprintln!("  Preamp / Gain: {:.2} dB", g.preamp_db);
        }
    }

    let mut alsa_output = AlsaAudioOutput::open(alsa_config)
        .map_err(|e| EngineError::Pipeline(format!("ALSA open failed: {e}")))?;

    // 6. Click-free Priming
    let carrier = match target_spec {
        TargetRateSpec::Dsd { .. } => {
            let transport = settings
                .output
                .as_ref()
                .and_then(|o| o.transport)
                .unwrap_or(OutputTransport::Native);
            match transport {
                OutputTransport::Native => IdleCarrierFormat::NativeDsd,
                OutputTransport::Dop => {
                    let fmt = settings
                        .output
                        .as_ref()
                        .and_then(|o| o.sample_format)
                        .unwrap_or(SampleFormat::S32Le);
                    let dop_fmt = match fmt {
                        SampleFormat::S32Le => DopContainerFormat::S32Le,
                        SampleFormat::S24Le => DopContainerFormat::S24Le,
                        _ => DopContainerFormat::S24_3Le,
                    };
                    IdleCarrierFormat::DoP(dop_fmt)
                }
                OutputTransport::Pcm => IdleCarrierFormat::Pcm,
            }
        }
        TargetRateSpec::Pcm { .. } => IdleCarrierFormat::Pcm,
    };

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
        .map_err(|e| EngineError::Pipeline(format!("ALSA drain failed: {e}")))?;

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

pub fn run_convert(args: ConvertArgs) -> Result<(), EngineError> {
    // 1. Resolve Settings
    let settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        Settings::from_toml_str(&content)?
    } else if args.output != "-" {
        // Auto-infer from output extension
        let ext = Path::new(&args.output)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let container = OutputContainer::from_extension(ext).unwrap_or(OutputContainer::Dsf);
        Settings::default_offline(container)
    } else {
        // Output is stdout ("-")
        Settings::default_offline(OutputContainer::Raw)
    };

    // If writing to stdout, verify invariants
    if args.output == "-" {
        // Check if container was explicitly configured or defaulted to seekable container
        let container = settings.output.as_ref().and_then(|o| o.container);
        match container {
            Some(OutputContainer::Dsf) => {
                return Err(EngineError::SeekableOutputRequired { container: "dsf" });
            }
            Some(OutputContainer::Wav) => {
                return Err(EngineError::SeekableOutputRequired { container: "wav" });
            }
            _ => {}
        }
    }

    settings.validate()?;

    // 2. Open AudioInput
    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if args.input == "-" {
        let sample_format = settings
            .input
            .as_ref()
            .and_then(|i| i.sample_format)
            .unwrap_or(audigma_dsp::config::SampleFormat::S16Le);
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
        let file_input = FileAudioInput::open(&args.input)?;
        let rate = file_input.info().sample_rate;
        (Box::new(file_input), rate)
    };

    // 3. Open AudioOutput
    let output: Box<dyn AudioOutput> = if args.output == "-" {
        Box::new(StdioAudioOutput::new())
    } else {
        Box::new(FileAudioOutput::create(&args.output)?)
    };

    // 4. Prepare and Run Pipeline
    let pipeline = OfflinePipeline::prepare(settings, input_rate)?;
    let report = pipeline.run(&mut *input, output)?;

    // 5. Output Report
    if args.json {
        let json_val = json!({
            "input_rate_hz": report.input_rate.get(),
            "target_format": format!("{:?}", report.target_format),
            "total_input_frames": report.total_input_frames,
            "total_output_frames": report.total_output_frames,
            "total_bytes_written": report.total_bytes_written,
            "measured_peak_dbfs": report.measured_peak_dbfs,
            "applied_gain_db": report.applied_gain_db,
            "overload_count": report.overload_count,
            "elapsed_ms": report.elapsed_ms,
        });
        eprintln!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        eprintln!("audigma: conversion complete.");
        eprintln!(
            "  Input Rate: {} Hz ({} frames)",
            report.input_rate.get(),
            report.total_input_frames
        );
        eprintln!(
            "  Output: {:?} ({} frames/samples, {} bytes)",
            report.target_format, report.total_output_frames, report.total_bytes_written
        );
        if let Some(peak) = report.measured_peak_dbfs {
            eprintln!("  Pass 1 Measured Peak: {:.2} dBFS", peak);
        }
        eprintln!("  Applied Gain: {:.2} dB", report.applied_gain_db);
        eprintln!("  Overloads: {}", report.overload_count);
        eprintln!("  Elapsed: {} ms", report.elapsed_ms);
    }

    Ok(())
}

pub fn run_benchmark(args: BenchmarkArgs) -> Result<(), EngineError> {
    let mut settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        let s = Settings::from_toml_str(&content)?;
        s.validate()?;
        s
    } else {
        Settings::default_offline(OutputContainer::Dsf)
    };

    if let Some(ref preset_name) = args.preset {
        let preset_def = by_name(preset_name)
            .ok_or_else(|| EngineError::Pipeline(format!("unknown preset '{preset_name}'")))?;
        if let Some(ref mut resampler) = settings.resampler {
            resampler.preset = preset_def.preset;
        } else {
            let r = audigma_settings::ResamplerConfig {
                preset: preset_def.preset,
                ..Default::default()
            };
            settings.resampler = Some(r);
        }
    } else if args.config.is_none() {
        // Default standalone benchmark preset is "balanced" (standard live real-time DSP preset)
        if let Some(ref mut resampler) = settings.resampler {
            resampler.preset = audigma_dsp::config::FilterPreset::Balanced;
        }
    }

    const BENCH_SECONDS: usize = 10;
    const SAMPLE_RATE_HZ: u32 = 44_100;
    let total_frames = BENCH_SECONDS * SAMPLE_RATE_HZ as usize;

    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if let Some(ref in_path) =
        args.input
    {
        let file = std::fs::File::open(in_path)?;
        let decoder = AudioDecoder::try_new(file, in_path.extension().and_then(|e| e.to_str()))?;
        let rate = decoder.info().sample_rate;
        (Box::new(DecoderInput::new(decoder)), rate)
    } else {
        // Generate deterministic 10s synthetic stereo test signal (1 kHz coherent sine at -6 dBFS)
        let amp = 10.0f64.powf(-6.0 / 20.0);
        let mut left = Vec::with_capacity(total_frames);
        let mut right = Vec::with_capacity(total_frames);
        for i in 0..total_frames {
            let t = i as f64 / SAMPLE_RATE_HZ as f64;
            let s = amp * (2.0 * PI * 1000.0 * t).sin();
            left.push(s);
            right.push(s);
        }
        let info = AudioStreamInfo {
            sample_rate: PcmRateHz::PCM_44100,
            original_channels: 2,
            total_frames: Some(total_frames as u64),
            bits_per_sample: Some(24),
        };
        (
            Box::new(MockAudioInput::new(info, left, right, true)),
            PcmRateHz::PCM_44100,
        )
    };

    // Sink into in-memory cursor or dev-null-like sink
    let output = Box::new(std::io::Cursor::new(Vec::new()));

    let pipeline = OfflinePipeline::prepare(settings.clone(), input_rate)?;
    let plan = pipeline.plan().clone();

    let start_bench = Instant::now();
    let report = pipeline.run(&mut *input, output)?;
    let e2e_elapsed = start_bench.elapsed();

    let audio_duration_sec = report.total_input_frames as f64 / input_rate.get() as f64;
    let e2e_throughput_x = audio_duration_sec / e2e_elapsed.as_secs_f64();

    // 2. Measure DSP-only worker block timing (Design.md §8: p99.9 worker execution time <= 70% of block budget)
    // Run 2000 blocks of 64 baseband frames through the LTI cascade and 5th-order CRFB DSM
    const BLOCK_FRAMES: usize = 64;
    let target_spec = plan.stream_spec.target;
    let (stages, is_dsd) = match target_spec {
        audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } => {
            let mult = bit_rate.get() / input_rate.get();
            ((mult as f64).log2().round() as usize, true)
        }
        audigma_dsp::config::TargetRateSpec::Pcm { rate, .. } => {
            let mult = rate.get() / input_rate.get();
            let stages = if mult >= 1 {
                (mult as f64).log2().round() as usize
            } else {
                0
            };
            (stages, false)
        }
    };

    let mult = 1usize << stages;
    let tile_expanded = BLOCK_FRAMES * mult;
    let amp = 10.0f64.powf(-6.0 / 20.0);

    let mut bb_l = [0.0f64; BLOCK_FRAMES];
    let mut bb_r = [0.0f64; BLOCK_FRAMES];
    for i in 0..BLOCK_FRAMES {
        let t = i as f64 / input_rate.get() as f64;
        bb_l[i] = amp * (2.0 * PI * 1000.0 * t).sin();
        bb_r[i] = amp * (2.0 * PI * 2000.0 * t).sin();
    }

    // 2.A: Single-Threaded Sequential Baseline (Stereo L + R on 1 thread)
    let mut seq_cascade_l =
        audigma_dsp::fir::HalfBandCascade::new(plan.dsp_config.preset, stages, BLOCK_FRAMES);
    let mut seq_cascade_r =
        audigma_dsp::fir::HalfBandCascade::new(plan.dsp_config.preset, stages, BLOCK_FRAMES);
    let mut exp_l = vec![0.0f64; tile_expanded];
    let mut exp_r = vec![0.0f64; tile_expanded];
    let mut dsd_bytes_l = vec![0u8; tile_expanded / 8];
    let mut dsd_bytes_r = vec![0u8; tile_expanded / 8];

    let mut seq_mod_l = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            let coeffs = audigma_dsp::tables::get_crfb_coefficients(bit_rate).unwrap();
            let mut m = audigma_dsp::modulator::CrfbModulator::new(
                *coeffs,
                audigma_dsp::config::DitherType::Tpdf,
                audigma_dsp::config::OverloadPolicy::MuteRecover,
            );
            m.set_dither_seed(12345);
            Some(m)
        } else {
            None
        }
    } else {
        None
    };

    let mut seq_mod_r = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            let coeffs = audigma_dsp::tables::get_crfb_coefficients(bit_rate).unwrap();
            let mut m = audigma_dsp::modulator::CrfbModulator::new(
                *coeffs,
                audigma_dsp::config::DitherType::Tpdf,
                audigma_dsp::config::OverloadPolicy::MuteRecover,
            );
            m.set_dither_seed(12346);
            Some(m)
        } else {
            None
        }
    } else {
        None
    };

    // Sequential Warm-up
    for _ in 0..50 {
        seq_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut exp_r).unwrap();
        if let (Some(ml), Some(mr)) = (&mut seq_mod_l, &mut seq_mod_r) {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
            mr.process_tile_bytes(&exp_r, &mut dsd_bytes_r).unwrap();
        }
    }

    const BENCH_BLOCKS: usize = 2000;
    let seq_start = Instant::now();
    for _ in 0..BENCH_BLOCKS {
        seq_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut exp_r).unwrap();
        if let (Some(ml), Some(mr)) = (&mut seq_mod_l, &mut seq_mod_r) {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
            mr.process_tile_bytes(&exp_r, &mut dsd_bytes_r).unwrap();
        }
    }
    let seq_total_elapsed = seq_start.elapsed();

    // 2.B: Multi-Threaded ChannelWorkerPool (1 dedicated RT thread per channel)
    let target_dsd = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            Some(TargetDsdConfig {
                bit_rate,
                dither: audigma_dsp::config::DitherType::Tpdf,
                overload_policy: audigma_dsp::config::OverloadPolicy::MuteRecover,
                seed_base: 12345,
            })
        } else {
            None
        }
    } else {
        None
    };

    let mut worker_pool = ChannelWorkerPool::new(
        2,
        plan.dsp_config.preset,
        stages,
        BLOCK_FRAMES,
        target_dsd,
        None,
        None,
    )?;

    // Worker Pool Warm-up
    for _ in 0..50 {
        if is_dsd {
            worker_pool.process_tile_dsd(&[&bb_l, &bb_r]).unwrap();
        } else {
            worker_pool.process_tile_pcm(&[&bb_l, &bb_r]).unwrap();
        }
    }

    let mut worker_block_times_ns = Vec::with_capacity(BENCH_BLOCKS);
    let worker_start = Instant::now();

    for _ in 0..BENCH_BLOCKS {
        let b_start = Instant::now();
        if is_dsd {
            worker_pool.process_tile_dsd(&[&bb_l, &bb_r]).unwrap();
        } else {
            worker_pool.process_tile_pcm(&[&bb_l, &bb_r]).unwrap();
        }
        worker_block_times_ns.push(b_start.elapsed().as_nanos() as u64);
    }
    let worker_total_elapsed = worker_start.elapsed();

    worker_block_times_ns.sort_unstable();
    let dsp_audio_duration_sec =
        BENCH_BLOCKS as f64 * (BLOCK_FRAMES as f64 / input_rate.get() as f64);
    let worker_throughput_x = dsp_audio_duration_sec / worker_total_elapsed.as_secs_f64();
    let seq_throughput_x = dsp_audio_duration_sec / seq_total_elapsed.as_secs_f64();
    let speedup_factor = seq_total_elapsed.as_secs_f64() / worker_total_elapsed.as_secs_f64();

    let p50_ns = worker_block_times_ns[BENCH_BLOCKS * 50 / 100];
    let p99_ns = worker_block_times_ns[BENCH_BLOCKS * 99 / 100];
    let p999_ns = worker_block_times_ns[BENCH_BLOCKS * 999 / 1000];
    let avg_ns = worker_block_times_ns.iter().sum::<u64>() / BENCH_BLOCKS as u64;

    let block_budget_micros = (BLOCK_FRAMES as f64 / input_rate.get() as f64) * 1_000_000.0;
    let p999_micros = p999_ns as f64 / 1000.0;
    let p999_budget_used_percent = (p999_micros / block_budget_micros) * 100.0;
    let efficiency_pct = (speedup_factor / 2.0) * 100.0;

    let realtime_status = if worker_throughput_x >= 2.0 {
        "Optimal (>= 2x realtime margin)"
    } else if worker_throughput_x >= 1.0 {
        "Capable (realtime playback supported)"
    } else {
        "Constrained (sub-realtime; offline conversion recommended)"
    };

    let budget_status = if p999_budget_used_percent <= 50.0 {
        "Ample RT headroom"
    } else if p999_budget_used_percent <= 70.0 {
        "Within safe RT limits"
    } else {
        "High CPU utilization for 64-frame block"
    };

    if args.json {
        let json_val = json!({
            "preset": plan.preset_def.name,
            "input_rate_hz": input_rate.get(),
            "target_format": format!("{:?}", report.target_format),
            "dsp_worker_throughput_x_realtime": worker_throughput_x,
            "dsp_sequential_throughput_x_realtime": seq_throughput_x,
            "dsp_speedup_factor": speedup_factor,
            "parallel_efficiency_percent": efficiency_pct,
            "realtime_tier": if worker_throughput_x >= 2.0 { "optimal" } else if worker_throughput_x >= 1.0 { "capable" } else { "constrained" },
            "end_to_end_throughput_x_realtime": e2e_throughput_x,
            "audio_duration_sec": audio_duration_sec,
            "block_frames": BLOCK_FRAMES,
            "block_budget_micros": block_budget_micros,
            "avg_block_micros": avg_ns as f64 / 1000.0,
            "p50_block_micros": p50_ns as f64 / 1000.0,
            "p99_block_micros": p99_ns as f64 / 1000.0,
            "p999_block_micros": p999_micros,
            "p999_budget_percent": p999_budget_used_percent,
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        println!("=======================================================");
        println!("audigma benchmark: {}", plan.preset_def.name);
        println!("=======================================================");
        println!("  Input Rate:            {} Hz", input_rate.get());
        println!("  Target Format:         {:?}", report.target_format);
        println!(
            "  Worker Pool Rate:      {:.2}x real-time ({})",
            worker_throughput_x, realtime_status
        );
        println!(
            "  Sequential Baseline:   {:.2}x real-time",
            seq_throughput_x
        );
        println!(
            "  Parallel Scaling:      {:.2}x speedup across 2 channels ({:.1}% efficiency)",
            speedup_factor, efficiency_pct
        );
        println!(
            "  p99.9 Block Latency:   {:.2} µs / {:.2} µs ({:.1}% of budget: {})",
            p999_micros, block_budget_micros, p999_budget_used_percent, budget_status
        );
        println!("  Avg Block Latency:     {:.2} µs", avg_ns as f64 / 1000.0);
        println!(
            "  End-to-End Rate:       {:.2}x real-time (with two-pass peak analysis)",
            e2e_throughput_x
        );
        println!("=======================================================");
    }

    Ok(())
}

pub fn run_presets(args: PresetsArgs) {
    match args.command {
        PresetsCommands::List => {
            println!(
                "{:<14} {:<10} {:<10} DESCRIPTION",
                "PRESET", "RIPPLE", "REJECTION"
            );
            println!("{}", "-".repeat(70));
            for p in ALL_PRESETS {
                println!(
                    "{:<14} <= {:.3} dB >= {:.0} dB   {}",
                    p.name, p.passband_ripple_max_db, p.stopband_rejection_min_db, p.description
                );
            }
        }
        PresetsCommands::Show { name } => {
            if let Some(p) = by_name(&name) {
                println!("Preset: {}", p.name);
                println!("Description:             {}", p.description);
                println!(
                    "Passband Ripple Max:     <= {:.4} dB",
                    p.passband_ripple_max_db
                );
                println!(
                    "Stopband Rejection Min:  >= {:.1} dB",
                    p.stopband_rejection_min_db
                );
                println!("Modulator Order:         {}", p.modulator.order);
                println!("NTF H_inf:               {:.2}", p.modulator.ntf_h_inf);
                println!(
                    "Max Stable Input:        {:.2} dBFS",
                    p.modulator.max_stable_input_dbfs
                );
                println!(
                    "Default Safety Margin:   {:.2} dB",
                    p.modulator.default_safety_margin_db
                );
                println!(
                    "Baseline Headroom:       {:.2} dB",
                    p.modulator.baseline_headroom_db
                );
            } else {
                eprintln!("audigma error: unknown preset '{}'", name);
                eprintln!("Available presets: balanced, reference, low-latency");
                std::process::exit(1);
            }
        }
    }
}

pub fn run_config(args: ConfigArgs) -> Result<(), EngineError> {
    match args.command {
        ConfigCommands::Check { config } => {
            let path = resolve_config_path(config.as_deref());
            let content = std::fs::read_to_string(&path).map_err(|e| {
                EngineError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to read config file '{}': {e}", path.display()),
                ))
            })?;
            let settings = Settings::from_toml_str(&content)?;
            settings.validate()?;
            println!("Configuration '{}' is valid.", path.display());
            Ok(())
        }
        ConfigCommands::Explain {
            config,
            input_rate,
            json,
        } => {
            let path = resolve_config_path(config.as_deref());
            let content = std::fs::read_to_string(&path).map_err(|e| {
                EngineError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to read config file '{}': {e}", path.display()),
                ))
            })?;
            let settings = Settings::from_toml_str(&content)?;
            settings.validate()?;

            let pcm_rate = PcmRateHz::new(input_rate).ok_or_else(|| {
                EngineError::Pipeline(format!("unsupported input sample rate {input_rate} Hz"))
            })?;
            let plan = settings.explain_for_input_rate(pcm_rate)?;

            if json {
                let json_val = json!({
                    "config_file": path.display().to_string(),
                    "input_rate_hz": input_rate,
                    "preset": plan.preset_def.name,
                    "target_spec": format!("{:?}", plan.stream_spec.target),
                    "total_configured_gain_db": plan.dsp_config.total_configured_gain_db(),
                    "preamp_db": plan.dsp_config.preamp_db,
                    "profile_headroom_db": plan.dsp_config.profile_headroom_db,
                    "max_stable_input_dbfs": plan.dsp_config.max_stable_input_dbfs,
                    "dither": format!("{:?}", plan.dsp_config.dither),
                    "overload_policy": format!("{:?}", plan.dsp_config.overload_policy),
                });
                println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
            } else {
                println!("Configuration: {}", path.display());
                println!("Input Rate:    {} Hz", input_rate);
                println!("Preset:        {}", plan.preset_def.name);
                println!("Target:        {:?}", plan.stream_spec.target);
                println!(
                    "Total Gain:    {:.2} dB (preamp: {:.2} dB, headroom: {:.2} dB)",
                    plan.dsp_config.total_configured_gain_db(),
                    plan.dsp_config.preamp_db,
                    plan.dsp_config.profile_headroom_db
                );
                println!(
                    "Max Input:     {:.2} dBFS",
                    plan.dsp_config.max_stable_input_dbfs
                );
                println!("Dither:        {:?}", plan.dsp_config.dither);
                println!("Overload:      {:?}", plan.dsp_config.overload_policy);
            }
            Ok(())
        }
    }
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

fn print_alsa_probe_report(report: &audigma_io::AlsaProbeReport) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_volume_to_db_decibels() {
        assert_eq!(parse_volume_to_db("0").unwrap(), 0.0);
        assert_eq!(parse_volume_to_db("0.0").unwrap(), 0.0);
        assert_eq!(parse_volume_to_db("-6").unwrap(), -6.0);
        assert_eq!(parse_volume_to_db("-3.5").unwrap(), -3.5);
        assert_eq!(parse_volume_to_db("+2.0").unwrap(), 2.0);
        assert_eq!(parse_volume_to_db("-12dB").unwrap(), -12.0);
        assert_eq!(parse_volume_to_db("-10.5 dB").unwrap(), -10.5);
        assert_eq!(parse_volume_to_db("0dB").unwrap(), 0.0);
    }

    #[test]
    fn test_parse_volume_to_db_percentage() {
        assert_eq!(parse_volume_to_db("100%").unwrap(), 0.0);
        assert_eq!(parse_volume_to_db("0%").unwrap(), -120.0);

        let half = parse_volume_to_db("50%").unwrap();
        // 50% slider => ALSA volume_mapping curve with -60 dB floor => ~= -15.58 dB
        assert!((half - (-15.58)).abs() < 0.1);

        let ten = parse_volume_to_db("10%").unwrap();
        // 10% slider => ALSA curve => ~= -43.27 dB (audible, pleasant background)
        assert!((ten - (-43.27)).abs() < 0.1);
    }

    #[test]
    fn test_parse_volume_to_db_invalid() {
        assert!(parse_volume_to_db("").is_err());
        assert!(parse_volume_to_db("   ").is_err());
        assert!(parse_volume_to_db("xyz").is_err());
        assert!(parse_volume_to_db("-5%").is_err());
        assert!(parse_volume_to_db("abc%").is_err());
    }
}
