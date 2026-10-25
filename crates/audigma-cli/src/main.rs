//! `audigma`: Executable entry point and CLI router (`Design.md §7`).

pub mod commands;
pub mod ipc;
pub mod overrides;
pub mod volume;

use clap::Parser;
use commands::{
    Cli, Commands, run_benchmark, run_config, run_convert, run_play, run_presets, run_probe,
    run_run, run_set, run_status, run_stop,
};

fn main() {
    let _ftz = audigma_dsp::ScopedFtzDaz::new();
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Run(args) => run_run(args),
        Commands::Stop(args) => run_stop(args),
        Commands::Status(args) => run_status(args),
        Commands::Set(args) => run_set(args),
        Commands::Play(args) => run_play(args),
        Commands::Convert(args) => run_convert(args),
        Commands::Benchmark(args) => run_benchmark(args),
        Commands::Presets(args) => {
            run_presets(args);
            Ok(())
        }
        Commands::Config(args) => run_config(args),
        Commands::Probe(args) => run_probe(args),
    };

    if let Err(err) = result {
        eprintln!("audigma error: {err}");
        std::process::exit(1);
    }
}
