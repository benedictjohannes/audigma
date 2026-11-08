//! Handler for `audigma convert` subcommand.

use std::path::{Path, PathBuf};

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::OfflinePipeline;
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_io::file::{FileAudioInput, FileAudioOutput};
use audigma_io::stdio::{StdioAudioInput, StdioAudioOutput};
use audigma_settings::{OutputContainer, Settings};
use clap::Args;
use serde_json::json;

use crate::error::CliError;

#[derive(Args, Debug)]
pub struct ConvertArgs {
    /// Input file path (or '-' for stdin)
    #[arg(short = 'i', long = "input")]
    pub input: String,

    /// Output file path (or '-' for stdout)
    #[arg(short = 'o', long = "output")]
    pub output: String,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Emit JSON report at the end of conversion
    #[arg(long = "json")]
    pub json: bool,
}

pub fn run_convert(args: ConvertArgs) -> Result<(), CliError> {
    // 1. Resolve Settings
    let settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        Settings::from_toml_str(&content)?
    } else if args.output != "-" {
        // Auto-infer from output extension
        let ext = Path::new(&args.output)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let container = OutputContainer::from_extension(ext).unwrap_or(OutputContainer::Dsf);
        Settings::default_offline(container)
    } else {
        // Output is stdout ("-")
        Settings::default_offline(OutputContainer::Raw)
    };

    // If writing to stdout, verify invariants
    if args.output == "-" {
        // Stdout is non-seekable: only headerless raw streams can be written to it.
        let output = settings.output.as_ref();
        match output.and_then(|o| o.container) {
            Some(OutputContainer::Dsf) => {
                return Err(EngineError::SeekableOutputRequired { container: "dsf" }.into());
            }
            Some(OutputContainer::Wav) => {
                return Err(EngineError::SeekableOutputRequired { container: "wav" }.into());
            }
            Some(OutputContainer::Flac) => {
                return Err(EngineError::SeekableOutputRequired { container: "flac" }.into());
            }
            Some(OutputContainer::Raw) => {}
            None => {
                return Err(CliError::Config(
                    "writing to stdout (-o -) requires [output] container = \"raw\"".to_string(),
                ));
            }
        }
        let target = settings.target.as_ref();
        let transport = output.and_then(|o| o.transport);
        if transport == Some(audigma_settings::OutputTransport::Dop) {
            if output.and_then(|o| o.dop_sample_format).is_none() {
                return Err(CliError::Config(
                    "writing DoP to stdout (-o -) requires an explicit [output] dop_sample_format"
                        .to_string(),
                ));
            }
        } else if target.map(|t| t.format) == Some(audigma_dsp::config::TargetFormat::Pcm)
            && target.and_then(|t| t.sample_format).is_none()
        {
            return Err(CliError::Config(
                "writing PCM to stdout (-o -) requires an explicit [target] sample_format"
                    .to_string(),
            ));
        }
    }

    settings.validate()?;

    // 2. Open AudioInput
    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if args.input == "-" {
        let sample_format = settings
            .input
            .as_ref()
            .and_then(|i| i.sample_format)
            .unwrap_or(audigma_dsp::config::SampleFormat::S16Le);
        let sample_rate = settings
            .input
            .as_ref()
            .and_then(|i| i.rate_hz)
            .unwrap_or(PcmRateHz::PCM_44100);
        let channels = settings
            .input
            .as_ref()
            .and_then(|i| i.channels)
            .unwrap_or(2);
        (
            Box::new(StdioAudioInput::new(sample_format, sample_rate, channels)),
            sample_rate,
        )
    } else {
        let dsd_gain_db = settings
            .input
            .as_ref()
            .map(|i| i.dsd_gain_db())
            .unwrap_or(6.0);
        let dsd_preset = settings.resolved_dsd_decimation_preset();
        let max_intermediate = settings.resolved_dsd_max_intermediate_rate_hz();
        let file_input = FileAudioInput::open_with_dsd_full_config(
            &args.input,
            dsd_gain_db,
            dsd_preset,
            max_intermediate,
        )?;
        let rate = file_input.info().sample_rate;
        (Box::new(file_input), rate)
    };

    // 3. Open AudioOutput
    let output: Box<dyn AudioOutput> = if args.output == "-" {
        Box::new(StdioAudioOutput::new())
    } else {
        Box::new(FileAudioOutput::create(&args.output)?)
    };

    // 4. Prepare and Run Pipeline
    let write_report = settings
        .offline
        .as_ref()
        .and_then(|o| o.write_report)
        .unwrap_or(false);
    let pipeline = {
        let placement = crate::runtime::resolve(&settings, 2, true)?;
        crate::runtime::apply_housekeeping(&placement);
        OfflinePipeline::prepare_with_workers(
            settings,
            input_rate,
            Some(crate::runtime::worker_setup(&placement)),
        )?
    };
    let report = pipeline.run(&mut *input, output)?;

    // 5. Output Report
    let emit_json = args.json || write_report;

    if emit_json {
        let json_val = json!({
            "input_rate_hz": report.input_rate.get(),
            "target_format": format!("{:?}", report.target_format),
            "total_input_frames": report.total_input_frames,
            "total_output_frames": report.total_output_frames,
            "total_bytes_written": report.total_bytes_written,
            "measured_peak_dbfs": report.measured_peak_dbfs,
            "applied_gain_db": report.applied_gain_db,
            "overload_count": report.overload_count,
            "elapsed_ms": report.elapsed_ms,
        });
        eprintln!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        eprintln!("audigma: conversion complete.");
        eprintln!(
            "  Input Rate: {} Hz ({} frames)",
            report.input_rate.get(),
            report.total_input_frames
        );
        eprintln!(
            "  Output: {:?} ({} frames/samples, {} bytes)",
            report.target_format, report.total_output_frames, report.total_bytes_written
        );
        if let Some(peak) = report.measured_peak_dbfs {
            eprintln!("  Pass 1 Measured Peak: {:.2} dBFS", peak);
        }
        eprintln!("  Applied Gain: {:.2} dB", report.applied_gain_db);
        eprintln!("  Overloads: {}", report.overload_count);
        eprintln!("  Elapsed: {} ms", report.elapsed_ms);
    }

    Ok(())
}
