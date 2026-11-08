//! Handler for `audigma presets` subcommand.

use audigma_dsp::presets::{ALL_PRESETS, DEFAULT_SAFETY_MARGIN_DB, by_name};
use audigma_dsp::{DsdBitRateHz, max_stable_input_dbfs};
use clap::{Args, Subcommand};

use crate::error::CliError;

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

pub fn run_presets(args: PresetsArgs) -> Result<(), CliError> {
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
                println!(
                    "Safety Margin M:         {:.2} dB (DSD targets; headroom = L - M)",
                    DEFAULT_SAFETY_MARGIN_DB
                );
                println!(
                    "Modulator input limit L (dBFS), by order and DSD rate (independent of preset):"
                );
                println!("  {:<24} {:>8} {:>8}", "RATE", "5th", "7th");
                for rate in [
                    DsdBitRateHz::DSD64,
                    DsdBitRateHz::DSD128,
                    DsdBitRateHz::DSD256,
                    DsdBitRateHz::DSD512,
                    DsdBitRateHz::DSD64_48,
                    DsdBitRateHz::DSD128_48,
                    DsdBitRateHz::DSD256_48,
                    DsdBitRateHz::DSD512_48,
                ] {
                    let l = |order| {
                        max_stable_input_dbfs(order, rate)
                            .map_or_else(|_| "n/a".to_string(), |v| format!("{v:.2}"))
                    };
                    println!("  {:<24} {:>8} {:>8}", rate.to_string(), l(5), l(7));
                }
                Ok(())
            } else {
                Err(CliError::PresetNotFound { name })
            }
        }
    }
}
