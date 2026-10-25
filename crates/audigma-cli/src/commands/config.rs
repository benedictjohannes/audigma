//! Handler for `audigma config` subcommand.

use std::path::PathBuf;

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::EngineError;
use audigma_settings::Settings;
use clap::{Args, Subcommand};
use serde_json::json;

use super::resolve_config_path;

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
