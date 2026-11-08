//! `audigma`: Executable entry point and CLI router (`Design.md §7`).

pub mod commands;
pub mod error;
pub mod ipc;
pub mod overrides;
pub mod runtime;
pub mod volume;

use clap::Parser;
use commands::{
    Cli, Commands, resolve_config_path, run_benchmark, run_config, run_convert, run_play,
    run_probe, run_run, run_set, run_status, run_stop,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let _ftz = audigma_dsp::ScopedFtzDaz::new();
    let cli = Cli::parse();

    // Rust ignores SIGPIPE by default, which makes `println!` panic on `audigma status | head`.
    // Short-lived commands restore the default so they exit quietly like other Unix CLIs. The
    // daemon keeps it ignored: a client or log pipe closing must stay an ordinary EPIPE error.
    if !matches!(cli.command, Commands::Run(_)) {
        // SAFETY: resetting a signal disposition at startup, before any thread is spawned.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }

    // 1. Determine command default log level: info for run, warn for short-lived commands (§6.1)
    let default_level = match &cli.command {
        Commands::Run(_) => audigma_log::LogLevel::Info,
        _ => audigma_log::LogLevel::Warn,
    };

    // 2. Pre-read config file to extract [logging] settings if present (§7.4)
    let explicit_config = match &cli.command {
        Commands::Run(args) => args.config.as_deref(),
        Commands::Play(args) => args.config.as_deref(),
        Commands::Convert(args) => args.config.as_deref(),
        Commands::Benchmark(args) => args.config.as_deref(),
        Commands::Config(args) => match &args.command {
            commands::ConfigCommands::Explain { config, .. } => config.as_deref(),
            commands::ConfigCommands::Check { config } => config.as_deref(),
        },
        _ => None,
    };
    let config_path = resolve_config_path(explicit_config);
    let (settings_format, settings_level) = if config_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            if let Ok(settings) = audigma_settings::Settings::from_toml_str(&content) {
                if let Some(log) = settings.logging {
                    (log.format, log.level)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // 3. Resolve log configuration with proper precedence (§7.4)
    let log_config = audigma_log::LogConfig::resolve(
        cli.log_format,
        cli.log_level,
        settings_format.as_deref(),
        settings_level.as_deref(),
        default_level,
    );

    // 4. Initialize structured logging with non-blocking writer and guard lifetime (§9)
    let _guard = match audigma_log::init(&log_config) {
        Ok(guard) => Some(guard),
        Err(err) => {
            eprintln!("audigma: logging initialization warning: {err}");
            None
        }
    };

    // 5. Dispatch CLI command
    let result = match cli.command {
        Commands::Run(args) => run_run(args),
        Commands::Stop(args) => run_stop(args),
        Commands::Status(args) => run_status(args),
        Commands::Set(args) => run_set(args),
        Commands::Play(args) => run_play(args),
        Commands::Convert(args) => run_convert(args),
        Commands::Benchmark(args) => run_benchmark(args),
        Commands::Config(args) => run_config(args),
        Commands::Probe(args) => run_probe(args),
    };

    // 6. Return ExitCode; destructors (including _guard drop and log flush) execute cleanly
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("audigma error: {err}");
            ExitCode::FAILURE
        }
    }
}
