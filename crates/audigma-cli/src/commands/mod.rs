//! CLI subcommands and commands for `audigma` (`Design.md §7`).

pub mod benchmark;
pub mod config;
pub mod control;
pub mod convert;
pub mod play;
pub mod probe;
pub mod run;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

pub use benchmark::{BenchmarkArgs, run_benchmark};
pub use config::{ConfigArgs, ConfigCommands, run_config};
pub use control::{
    PauseArgs, ResumeArgs, SetArgs, SetCommands, StatusArgs, StopArgs, run_pause, run_resume,
    run_set, run_status, run_stop,
};
pub use convert::{ConvertArgs, run_convert};
pub use play::{PlayArgs, run_play};
pub use probe::{ProbeArgs, print_alsa_probe_report, run_probe};
pub use run::{RunArgs, run_run};

pub use crate::volume::parse_volume_to_db;

#[derive(Parser, Debug)]
#[command(
    name = "audigma",
    version,
    about = "High-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD) engine",
    long_about = "audigma is a high-precision Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD)."
)]
pub struct Cli {
    /// Log renderer format ("text", "json", "auto")
    #[arg(long = "log-format", global = true)]
    pub log_format: Option<audigma_log::LogFormat>,

    /// Diagnostic log level filter ("error", "warn", "info", "debug", "trace")
    #[arg(long = "log-level", global = true)]
    pub log_level: Option<audigma_log::LogLevel>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Selects which daemon instance a command targets. Flattened into `run`, `stop`, `status`
/// and `set` only (`-i` is already taken by `convert --input`).
#[derive(clap::Args, Debug, Clone, Default)]
pub struct InstanceArgs {
    /// Daemon instance name, for running several daemons side by side (1-32 chars of [A-Za-z0-9_-]).
    /// `run` defaults to "default". `status` lists all live instances; `stop` and `set` target the
    /// sole live instance when there is exactly one.
    #[arg(short = 'i', long = "instance", env = "AUDIGMA_INSTANCE")]
    pub instance: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run live audio streaming daemon in foreground
    Run(RunArgs),

    /// Request orderly shutdown of a running audigma daemon
    Stop(StopArgs),

    /// Release the output device and idle the daemon (undo with `resume`)
    Pause(PauseArgs),

    /// Rebuild the pipeline of a paused daemon from its re-read configuration
    Resume(ResumeArgs),

    /// Query live daemon runtime status and telemetry
    Status(StatusArgs),

    /// Modify daemon volume dynamically
    Set(SetArgs),

    /// Play audio directly to hardware via exclusive ALSA
    Play(PlayArgs),

    /// Convert an audio file or stream offline
    Convert(ConvertArgs),

    /// Run DSP throughput and execution budget benchmarks
    Benchmark(BenchmarkArgs),

    /// Validate or inspect configuration files
    Config(ConfigArgs),

    /// Probe audio hardware endpoints and supported formats/rates
    Probe(ProbeArgs),
}

pub fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
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
