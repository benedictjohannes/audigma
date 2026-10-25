//! Handlers for daemon control and IPC subcommands (`stop`, `status`, `set`).

use std::path::PathBuf;

use audigma_engine::traits::EngineError;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::ipc::{
    ControlClient, IpcError, IpcRequest, IpcResponse, default_socket_path,
};
use crate::volume::{VolumeArg, parse_volume_arg, validate_volume_db};

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

#[derive(Subcommand, Debug)]
pub enum SetCommands {
    /// Set live playback volume in dB (-120..=+12, e.g. -3.0 or 0) or percentage (e.g. 80% or 0%)
    Volume {
        #[arg(allow_hyphen_values = true)]
        value: String,
    },

    /// Switch active DSP preset (balanced, reference, low-latency)
    Preset { name: String },
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
                    println!(
                        "  Queue Occupancy:     {:.1}% ({:.1} ms)",
                        telem.queue_fill_pct, telem.buffer_fill_ms
                    );
                    println!(
                        "  Input Ring Chunks:   {} processed, {} dropped",
                        telem.processed_chunks, telem.dropped_chunks
                    );
                    let fault_str = if telem.asrc_fault_active { " [FAULT ACTIVE]" } else { "" };
                    println!(
                        "  ASRC Clock Drift:    {:.1} ppm (starvations: {}, overruns: {}){fault_str}",
                        telem.drift_ppm, telem.asrc_starvations, telem.asrc_overruns
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
            match parse_volume_arg(&value).map_err(EngineError::Pipeline)? {
                VolumeArg::Percent(percent) => IpcRequest::SetVolumePercent { percent },
                VolumeArg::Db(volume_db) => IpcRequest::SetVolume {
                    volume_db: validate_volume_db(volume_db).map_err(EngineError::Pipeline)?,
                },
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
