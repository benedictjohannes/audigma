//! Handlers for daemon control and IPC subcommands (`stop`, `pause`, `resume`, `status`, `set`).

use clap::{Args, Subcommand};
use serde_json::json;

use super::InstanceArgs;
use crate::error::CliError;
use crate::ipc::{ControlClient, Instance, IpcError, IpcRequest, IpcResponse, list_live_instances};
use crate::volume::{VolumeArg, parse_volume_arg, validate_volume_db};

#[derive(Args, Debug)]
pub struct StopArgs {
    #[command(flatten)]
    pub instance: InstanceArgs,
}

#[derive(Args, Debug)]
pub struct PauseArgs {
    #[command(flatten)]
    pub instance: InstanceArgs,
}

#[derive(Args, Debug)]
pub struct ResumeArgs {
    #[command(flatten)]
    pub instance: InstanceArgs,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Continuously watch daemon status telemetry
    #[arg(short = 'w', long = "watch")]
    pub watch: bool,

    /// Output report in structured JSON format
    #[arg(long = "json")]
    pub json: bool,

    #[command(flatten)]
    pub instance: InstanceArgs,
}

#[derive(Args, Debug)]
pub struct SetArgs {
    #[command(subcommand)]
    pub command: SetCommands,

    #[command(flatten)]
    pub instance: InstanceArgs,
}

#[derive(Subcommand, Debug)]
pub enum SetCommands {
    /// Set live playback volume in dB (-120..=+12, e.g. -3.0 or 0) or percentage (e.g. 80% or 0%)
    Volume {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

/// Picks the instance a control command targets.
/// Explicit name wins; otherwise the sole live instance; with none live, `default` (so errors
/// read "not running"); with several live, an error listing them.
fn resolve_target(explicit: Option<&str>) -> Result<Instance, CliError> {
    if let Some(name) = explicit {
        return Ok(Instance::new(name)?);
    }
    let mut live = list_live_instances(None);
    match live.len() {
        0 => Ok(Instance::default_instance()),
        1 => Ok(Instance::new(live.remove(0))?),
        _ => Err(CliError::Other(format!(
            "multiple audigma instances are running ({}); choose one with -i/--instance",
            live.join(", ")
        ))),
    }
}

fn print_telemetry(telem: &audigma_engine::DaemonTelemetry) {
    if telem.state == audigma_engine::DaemonState::Paused {
        let why = match telem.pause_reason {
            Some(audigma_engine::PauseReason::DeviceWaitExpired) => " (device not found)",
            _ => "",
        };
        println!("  State:               {}{why}", telem.state);
        // No pipeline exists while paused; only process-level facts are meaningful.
        println!("  Uptime:              {}s", telem.daemon_uptime_secs);
        let mute_str = if telem.is_muted { " [MUTED]" } else { "" };
        println!(
            "  Volume:              {:.1} dB ({:.0}%){mute_str}",
            telem.volume_db, telem.volume_percent
        );
        return;
    }
    println!("  State:               {}", telem.state);
    if let Some(ref target) = telem.device_target {
        println!("  Device Target:       {}", target);
    }
    if let Some(dur) = telem.disconnect_duration_ms {
        println!("  Disconnected For:    {} ms", dur);
    }
    println!("  Uptime:              {}s", telem.daemon_uptime_secs);
    println!("  Stream Uptime:       {}s", telem.stream_uptime_secs);

    match telem
        .input_dsd_bit_rate_hz
        .and_then(audigma_dsp::DsdBitRateHz::new)
    {
        Some(dsd) => println!(
            "  Input Rate:          {dsd} (decimated to {} Hz)",
            telem.input_rate_hz
        ),
        None => println!("  Input Rate:          {} Hz", telem.input_rate_hz),
    }
    println!(
        "  Output Target:       {} @ {} Hz",
        telem.output_format, telem.output_rate_hz
    );
    println!("  Filter:              {}", telem.filter);
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
    println!(
        "  Queue Occupancy:     {:.1}% ({:.1} ms)",
        telem.queue_fill_pct, telem.buffer_fill_ms
    );
    println!(
        "  Input Ring Chunks:   {} processed, {} dropped",
        telem.processed_chunks, telem.dropped_chunks
    );
    let fault_str = if telem.asrc_fault_active {
        " [FAULT ACTIVE]"
    } else {
        ""
    };
    println!(
        "  ASRC:                {}, correction {:+.1} ppm, estimated drift {} (starvations: {}, overruns: {}){fault_str}",
        telem.asrc_mode.as_str(),
        telem.asrc_correction_ppm,
        telem
            .drift_estimate_ppm
            .map_or_else(|| "n/a".to_string(), |ppm| format!("{ppm:+.1} ppm")),
        telem.asrc_starvations,
        telem.asrc_overruns
    );
    println!(
        "  Pipeline Health:     {} empty polls, {} idle entries ({} tiles)",
        telem.empty_polls, telem.idle_entries, telem.idle_tiles
    );
    println!(
        "  DSP Tile Execution:  max {} µs, {} late tiles",
        telem.max_dsp_tile_us, telem.late_dsp_tiles
    );
    println!("  Overload Events:     {}", telem.overload_count);
    println!("  ALSA XRuns:          {}", telem.xrun_count);
    if telem.rate_switch_count > 0 {
        println!("  Rate Switches:       {}", telem.rate_switch_count);
    }
}

/// Sends a request that must reach a running daemon (`pause`/`resume`); `verb` names it in output.
fn send_lifecycle(instance: Option<&str>, request: IpcRequest, done: &str) -> Result<(), CliError> {
    let target = resolve_target(instance)?;
    match ControlClient::send(&target, request) {
        Ok(IpcResponse::Ok) => {
            eprintln!("audigma: instance '{}' {done}.", target.name());
            Ok(())
        }
        Ok(IpcResponse::Error(msg)) => Err(CliError::Daemon(msg)),
        Ok(_) => Ok(()),
        Err(IpcError::DaemonNotRunning(name)) => Err(CliError::Daemon(format!(
            "instance '{name}' is not running"
        ))),
        Err(e) => Err(CliError::Ipc(e)),
    }
}

pub fn run_pause(args: PauseArgs) -> Result<(), CliError> {
    send_lifecycle(
        args.instance.instance.as_deref(),
        IpcRequest::Pause,
        "paused",
    )
}

pub fn run_resume(args: ResumeArgs) -> Result<(), CliError> {
    send_lifecycle(
        args.instance.instance.as_deref(),
        IpcRequest::Resume,
        "resumed",
    )
}

pub fn run_stop(args: StopArgs) -> Result<(), CliError> {
    let target = resolve_target(args.instance.instance.as_deref())?;
    match ControlClient::send(&target, IpcRequest::Stop) {
        Ok(IpcResponse::Ok) => {
            eprintln!(
                "audigma: stop request sent to instance '{}'.",
                target.name()
            );
            Ok(())
        }
        Ok(IpcResponse::Error(msg)) => Err(CliError::Daemon(msg)),
        Ok(_) => Ok(()),
        Err(IpcError::DaemonNotRunning(name)) => {
            eprintln!("audigma: instance '{name}' is not running.");
            Ok(())
        }
        Err(e) => Err(CliError::Ipc(e)),
    }
}

/// Queries one instance. `Ok(None)` means it is not running.
fn query_status(
    target: &Instance,
) -> Result<Option<Box<audigma_engine::DaemonTelemetry>>, CliError> {
    match ControlClient::send(target, IpcRequest::GetStatus) {
        Ok(IpcResponse::Telemetry(telem)) => Ok(Some(telem)),
        Ok(IpcResponse::Error(msg)) => Err(CliError::Daemon(msg)),
        Ok(_) => Ok(None),
        Err(IpcError::DaemonNotRunning(_)) => Ok(None),
        Err(e) => Err(CliError::Ipc(e)),
    }
}

fn telemetry_json(
    name: &str,
    telem: &audigma_engine::DaemonTelemetry,
) -> Result<serde_json::Value, CliError> {
    let mut value =
        serde_json::to_value(telem).map_err(|e| CliError::Serialization(e.to_string()))?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("instance".to_string(), json!(name));
    }
    Ok(value)
}

pub fn run_status(args: StatusArgs) -> Result<(), CliError> {
    // Bare `status` reports every live instance; `-i` reports exactly one.
    let targets: Vec<Instance> = match args.instance.instance.as_deref() {
        Some(name) => vec![Instance::new(name)?],
        None => {
            let live = list_live_instances(None);
            if live.is_empty() {
                vec![Instance::default_instance()]
            } else {
                live.into_iter()
                    .map(Instance::new)
                    .collect::<Result<_, _>>()?
            }
        }
    };
    if args.watch && targets.len() > 1 {
        return Err(CliError::Other(
            "--watch needs a single instance; choose one with -i/--instance".to_string(),
        ));
    }

    loop {
        let mut json_out = Vec::new();
        for (idx, target) in targets.iter().enumerate() {
            match query_status(target)? {
                Some(telem) if args.json => json_out.push(telemetry_json(target.name(), &telem)?),
                Some(telem) => {
                    if idx > 0 {
                        println!();
                    }
                    println!("Audigma Daemon Status (instance: {}):", target.name());
                    print_telemetry(&telem);
                }
                None if args.json => json_out.push(json!({
                    "instance": target.name(),
                    "status": "stopped",
                    "running": false
                })),
                None => eprintln!("audigma: instance '{}' is not running.", target.name()),
            }
        }
        if args.json {
            let value = if json_out.len() == 1 {
                json_out.remove(0)
            } else {
                serde_json::Value::Array(json_out)
            };
            let text = serde_json::to_string_pretty(&value)
                .map_err(|e| CliError::Serialization(e.to_string()))?;
            println!("{text}");
        }

        if !args.watch {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    Ok(())
}

pub fn run_set(args: SetArgs) -> Result<(), CliError> {
    let target = resolve_target(args.instance.instance.as_deref())?;
    let req = match args.command {
        SetCommands::Volume { value } => {
            match parse_volume_arg(&value).map_err(CliError::Volume)? {
                VolumeArg::Percent(percent) => IpcRequest::SetVolumePercent { percent },
                VolumeArg::Db(volume_db) => IpcRequest::SetVolume {
                    volume_db: validate_volume_db(volume_db).map_err(CliError::Volume)?,
                },
            }
        }
    };

    match ControlClient::send(&target, req) {
        Ok(IpcResponse::Ok) => {
            eprintln!("audigma: setting updated successfully.");
            Ok(())
        }
        Ok(IpcResponse::Error(msg)) => Err(CliError::Daemon(msg)),
        Ok(_) => Ok(()),
        Err(e) => Err(CliError::Ipc(e)),
    }
}
