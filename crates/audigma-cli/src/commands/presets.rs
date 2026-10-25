//! Handler for `audigma presets` subcommand.

use audigma_dsp::presets::{ALL_PRESETS, by_name};
use audigma_engine::EngineError;
use clap::{Args, Subcommand};

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

pub fn run_presets(args: PresetsArgs) -> Result<(), EngineError> {
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
            Ok(())
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
                Ok(())
            } else {
                Err(EngineError::Pipeline(format!(
                    "unknown preset '{name}'; available presets: balanced, reference, low-latency"
                )))
            }
        }
    }
}
