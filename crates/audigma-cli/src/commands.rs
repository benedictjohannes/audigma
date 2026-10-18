//! CLI subcommands and commands for `audigma` (`Design.md §7`).

use audigma_dsp::presets::{ALL_PRESETS, by_name};
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::{AudioInput, AudioOutput, DecoderInput, EngineError};
use audigma_engine::{MockAudioInput, OfflinePipeline};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_io::file::{FileAudioInput, FileAudioOutput};
use audigma_io::stdio::{StdioAudioInput, StdioAudioOutput};
use audigma_settings::{OutputContainer, Settings};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "audigma",
    version,
    about = "High-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD) engine",
    long_about = "audigma is a high-precision Rust audio engine, CLI, and Linux audio service for high-precision PCM resampling and PCM-to-1-bit Delta-Sigma Modulation (DSD)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert an audio file or stream offline
    Convert(ConvertArgs),

    /// Run DSP throughput and execution budget benchmarks
    Benchmark(BenchmarkArgs),

    /// Inspect and display DSP presets
    Presets(PresetsArgs),

    /// Validate or inspect configuration files
    Config(ConfigArgs),
}

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

#[derive(Args, Debug)]
pub struct BenchmarkArgs {
    /// Optional input audio file (if omitted, uses a deterministic 10s synthetic stereo signal)
    #[arg(short = 'i', long = "input")]
    pub input: Option<PathBuf>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Emit benchmark results in JSON format
    #[arg(long = "json")]
    pub json: bool,
}

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

fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
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

pub fn run_convert(args: ConvertArgs) -> Result<(), EngineError> {
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
        // Check if container was explicitly configured or defaulted to seekable container
        let container = settings.output.as_ref().and_then(|o| o.container);
        match container {
            Some(OutputContainer::Dsf) => {
                return Err(EngineError::SeekableOutputRequired { container: "dsf" });
            }
            Some(OutputContainer::Wav) => {
                return Err(EngineError::SeekableOutputRequired { container: "wav" });
            }
            _ => {}
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
        let file_input = FileAudioInput::open(&args.input)?;
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
    let pipeline = OfflinePipeline::prepare(settings, input_rate)?;
    let report = pipeline.run(&mut *input, output)?;

    // 5. Output Report
    if args.json {
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

pub fn run_benchmark(args: BenchmarkArgs) -> Result<(), EngineError> {
    let settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        let s = Settings::from_toml_str(&content)?;
        s.validate()?;
        s
    } else {
        Settings::default_offline(OutputContainer::Dsf)
    };

    const BENCH_SECONDS: usize = 10;
    const SAMPLE_RATE_HZ: u32 = 44_100;
    let total_frames = BENCH_SECONDS * SAMPLE_RATE_HZ as usize;

    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if let Some(ref in_path) =
        args.input
    {
        let file = std::fs::File::open(in_path)?;
        let decoder = AudioDecoder::try_new(file, in_path.extension().and_then(|e| e.to_str()))?;
        let rate = decoder.info().sample_rate;
        (Box::new(DecoderInput::new(decoder)), rate)
    } else {
        // Generate deterministic 10s synthetic stereo test signal (1 kHz coherent sine at -6 dBFS)
        let amp = 10.0f64.powf(-6.0 / 20.0);
        let mut left = Vec::with_capacity(total_frames);
        let mut right = Vec::with_capacity(total_frames);
        for i in 0..total_frames {
            let t = i as f64 / SAMPLE_RATE_HZ as f64;
            let s = amp * (2.0 * PI * 1000.0 * t).sin();
            left.push(s);
            right.push(s);
        }
        let info = AudioStreamInfo {
            sample_rate: PcmRateHz::PCM_44100,
            original_channels: 2,
            total_frames: Some(total_frames as u64),
            bits_per_sample: Some(24),
        };
        (
            Box::new(MockAudioInput::new(info, left, right, true)),
            PcmRateHz::PCM_44100,
        )
    };

    // Sink into in-memory cursor or dev-null-like sink
    let output = Box::new(std::io::Cursor::new(Vec::new()));

    let pipeline = OfflinePipeline::prepare(settings.clone(), input_rate)?;
    let plan = pipeline.plan().clone();

    let start_bench = Instant::now();
    let report = pipeline.run(&mut *input, output)?;
    let e2e_elapsed = start_bench.elapsed();

    let audio_duration_sec = report.total_input_frames as f64 / input_rate.get() as f64;
    let e2e_throughput_x = audio_duration_sec / e2e_elapsed.as_secs_f64();

    // 2. Measure DSP-only worker block timing (Design.md §8: p99.9 worker execution time <= 70% of block budget)
    // Run 2000 blocks of 64 baseband frames through the LTI cascade and 5th-order CRFB DSM
    const BLOCK_FRAMES: usize = 64;
    let target_spec = plan.stream_spec.target;
    let (stages, is_dsd) = match target_spec {
        audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } => {
            let mult = bit_rate.get() / input_rate.get();
            ((mult as f64).log2().round() as usize, true)
        }
        audigma_dsp::config::TargetRateSpec::Pcm { rate, .. } => {
            let mult = rate.get() / input_rate.get();
            let stages = if mult >= 1 {
                (mult as f64).log2().round() as usize
            } else {
                0
            };
            (stages, false)
        }
    };

    let mult = 1usize << stages;
    let tile_expanded = BLOCK_FRAMES * mult;
    let mut dsp_cascade_l =
        audigma_dsp::fir::HalfBandCascade::new(plan.dsp_config.preset, stages, BLOCK_FRAMES);
    let mut bb_l = [0.0f64; BLOCK_FRAMES];
    let mut exp_l = vec![0.0f64; tile_expanded];
    let mut dsd_bytes_l = vec![0u8; tile_expanded / 8];

    let amp = 10.0f64.powf(-6.0 / 20.0);
    for (i, item) in bb_l.iter_mut().enumerate() {
        let t = i as f64 / input_rate.get() as f64;
        let s = amp * (2.0 * PI * 1000.0 * t).sin();
        *item = s;
    }

    let mut dsp_mod_l = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            let coeffs = audigma_dsp::tables::get_crfb_coefficients(bit_rate).unwrap();
            Some(audigma_dsp::modulator::CrfbModulator::new(
                *coeffs,
                audigma_dsp::config::DitherType::Tpdf,
                audigma_dsp::config::OverloadPolicy::MuteRecover,
            ))
        } else {
            None
        }
    } else {
        None
    };

    // Warm up
    for _ in 0..100 {
        dsp_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        if let Some(ml) = &mut dsp_mod_l {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
        }
    }

    const BENCH_BLOCKS: usize = 2000;
    let mut worker_block_times_ns = Vec::with_capacity(BENCH_BLOCKS);
    let worker_start = Instant::now();

    for _ in 0..BENCH_BLOCKS {
        let b_start = Instant::now();
        dsp_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        if let Some(ml) = &mut dsp_mod_l {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
        }
        worker_block_times_ns.push(b_start.elapsed().as_nanos() as u64);
    }
    let worker_total_elapsed = worker_start.elapsed();

    worker_block_times_ns.sort_unstable();
    let dsp_audio_duration_sec =
        BENCH_BLOCKS as f64 * (BLOCK_FRAMES as f64 / input_rate.get() as f64);
    let worker_throughput_x = dsp_audio_duration_sec / worker_total_elapsed.as_secs_f64();

    let p50_ns = worker_block_times_ns[BENCH_BLOCKS * 50 / 100];
    let p99_ns = worker_block_times_ns[BENCH_BLOCKS * 99 / 100];
    let p999_ns = worker_block_times_ns[BENCH_BLOCKS * 999 / 1000];
    let avg_ns = worker_block_times_ns.iter().sum::<u64>() / BENCH_BLOCKS as u64;

    let block_budget_micros = (BLOCK_FRAMES as f64 / input_rate.get() as f64) * 1_000_000.0;
    let p999_micros = p999_ns as f64 / 1000.0;
    let p999_budget_used_percent = (p999_micros / block_budget_micros) * 100.0;

    let pass_throughput = worker_throughput_x >= 2.0;
    let pass_budget = p999_budget_used_percent <= 70.0;
    let status_pass = pass_throughput && pass_budget;

    if args.json {
        let json_val = json!({
            "preset": plan.preset_def.name,
            "input_rate_hz": input_rate.get(),
            "target_format": format!("{:?}", report.target_format),
            "dsp_worker_throughput_x_realtime": worker_throughput_x,
            "end_to_end_throughput_x_realtime": e2e_throughput_x,
            "audio_duration_sec": audio_duration_sec,
            "block_frames": BLOCK_FRAMES,
            "block_budget_micros": block_budget_micros,
            "avg_block_micros": avg_ns as f64 / 1000.0,
            "p50_block_micros": p50_ns as f64 / 1000.0,
            "p99_block_micros": p99_ns as f64 / 1000.0,
            "p999_block_micros": p999_micros,
            "p999_budget_percent": p999_budget_used_percent,
            "target_throughput_pass": pass_throughput,
            "target_budget_pass": pass_budget,
            "status": if status_pass { "PASS" } else { "FAIL" },
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        println!("=======================================================");
        println!("audigma benchmark: {}", plan.preset_def.name);
        println!("=======================================================");
        println!("  Input Rate:        {} Hz", input_rate.get());
        println!("  Target Format:     {:?}", report.target_format);
        println!(
            "  Worker Throughput: {:.2}x real-time (Goal: >= 2.0x) -> {}",
            worker_throughput_x,
            if pass_throughput { "PASS" } else { "FAIL" }
        );
        println!(
            "  p99.9 Latency:     {:.2} µs / {:.2} µs ({:.1}% of budget, Goal: <= 70%) -> {}",
            p999_micros,
            block_budget_micros,
            p999_budget_used_percent,
            if pass_budget { "PASS" } else { "FAIL" }
        );
        println!("  Avg Block Latency: {:.2} µs", avg_ns as f64 / 1000.0);
        println!(
            "  End-to-End Rate:   {:.2}x real-time (with two-pass peak analysis)",
            e2e_throughput_x
        );
        println!("=======================================================");
    }

    Ok(())
}

pub fn run_presets(args: PresetsArgs) {
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
            } else {
                eprintln!("audigma error: unknown preset '{}'", name);
                eprintln!("Available presets: balanced, reference, low-latency");
                std::process::exit(1);
            }
        }
    }
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
