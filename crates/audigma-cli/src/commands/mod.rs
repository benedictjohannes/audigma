//! CLI subcommands and commands for `audigma` (`Design.md §7`).

pub mod benchmark;
pub mod config;
pub mod control;
pub mod convert;
pub mod play;
pub mod presets;
pub mod probe;
pub mod run;

use std::path::{Path, PathBuf};
use clap::{Parser, Subcommand};

pub use benchmark::{BenchmarkArgs, run_benchmark};
pub use config::{ConfigArgs, ConfigCommands, run_config};
pub use control::{SetArgs, SetCommands, StatusArgs, StopArgs, run_set, run_status, run_stop};
pub use convert::{ConvertArgs, run_convert};
pub use play::{PlayArgs, run_play};
pub use presets::{PresetsArgs, PresetsCommands, run_presets};
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
