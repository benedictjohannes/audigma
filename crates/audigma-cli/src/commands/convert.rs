//! Handler for `audigma convert` subcommand.

use std::path::{Path, PathBuf};

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::OfflinePipeline;
use audigma_engine::traits::{AudioInput, AudioOutput};
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
    #[arg(short = 'c', long = "config", env = "AUDIGMA_CONFIG")]
    pub config: Option<PathBuf>,

    /// Emit JSON report at the end of conversion
    #[arg(long = "json")]
    pub json: bool,
}

pub fn run_convert(args: ConvertArgs) -> Result<(), CliError> {
    // 1. Resolve the container from the output path, then the settings.
    // `-o -` is a raw stream; otherwise the file extension decides (no config key).
    let container = if args.output == "-" {
        OutputContainer::Raw
    } else {
        OutputContainer::from_path(Path::new(&args.output)).ok_or_else(|| {
            CliError::Config(format!(
                "cannot tell the output container from '{}': use a .dsf, .wav, .flac or .raw extension, or -o - for a raw stream",
                args.output
            ))
        })?
    };
    let settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        Settings::from_toml_str(&content)?
    } else {
        Settings::default_offline(container)
    };
    if let Some(target) = settings.target.as_ref() {
        container.check_target(target)?;
    }

    // If writing to stdout, verify invariants (stdout is non-seekable, so only a raw
    // stream, which `-o -` always is, can be written to it).
    if args.output == "-" {
        let output = settings.output.as_ref();
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
    let mut input: Box<dyn AudioInput> = if args.input == "-" {
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
        Box::new(StdioAudioInput::new(sample_format, sample_rate, channels))
    } else {
        let dsd_gain_db = settings
            .input
            .as_ref()
            .map(|i| i.dsd_gain_db())
            .unwrap_or(6.0);
        let max_intermediate = settings.resolved_dsd_max_intermediate_rate_hz();
        Box::new(FileAudioInput::open_with_dsd_full_config(
            &args.input,
            dsd_gain_db,
            max_intermediate,
        )?)
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
        OfflinePipeline::prepare_for_input(
            settings,
            &*input,
            Some(crate::runtime::worker_setup(&placement)),
        )?
        .with_container(container)
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
