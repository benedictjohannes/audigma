//! `audigma`: Executable entry point and CLI router (`Design.md §7`).

pub mod commands;

use clap::Parser;
use commands::{Cli, Commands, run_benchmark, run_config, run_convert, run_presets};

fn main() {
    let _ftz = audigma_dsp::ScopedFtzDaz::new();
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Convert(args) => run_convert(args),
        Commands::Benchmark(args) => run_benchmark(args),
        Commands::Presets(args) => {
            run_presets(args);
            Ok(())
        }
        Commands::Config(args) => run_config(args),
    };

    if let Err(err) = result {
        eprintln!("audigma error: {err}");
        std::process::exit(1);
    }
}
