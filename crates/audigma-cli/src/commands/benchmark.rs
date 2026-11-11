//! Handler for `audigma benchmark` subcommand.
//!
//! The measurement lives in `audigma_configurator::assist::benchmark`; this file loads the
//! configuration and prints the result.

use audigma_configurator::assist::benchmark::{self, BenchmarkRequest, Tier};
use audigma_settings::{OutputContainer, Settings};
use clap::Args;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::error::CliError;

#[derive(Args, Debug)]
pub struct BenchmarkArgs {
    /// Optional input audio file (if omitted, uses a deterministic 10s synthetic stereo signal)
    #[arg(short = 'i', long = "input")]
    pub input: Option<PathBuf>,

    /// Optional TOML configuration file path
    #[arg(short = 'c', long = "config", env = "AUDIGMA_CONFIG")]
    pub config: Option<PathBuf>,

    /// Emit benchmark results in JSON format
    #[arg(long = "json")]
    pub json: bool,
}

pub fn run_benchmark(args: BenchmarkArgs) -> Result<(), CliError> {
    let settings = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path)?;
        let s = Settings::from_toml_str(&content)?;
        s.validate()?;
        s
    } else {
        Settings::default_offline(OutputContainer::Dsf)
    };

    let mut req = BenchmarkRequest::new(settings);
    req.input = args.input;
    let r = benchmark::run(&req, &AtomicBool::new(false), &mut |_| {})
        .map_err(|e| CliError::Config(e.to_string()))?;

    let realtime_status = match r.tier {
        Tier::Optimal => "Optimal (>= 2x realtime margin)",
        Tier::Capable => "Capable (realtime playback supported)",
        Tier::Constrained => "Constrained (sub-realtime; offline conversion recommended)",
    };
    let budget_status = if r.p999_budget_percent <= 50.0 {
        "Ample RT headroom"
    } else if r.p999_budget_percent <= 70.0 {
        "Within safe RT limits"
    } else {
        "High CPU utilization for 64-frame block"
    };

    if args.json {
        let json_val = json!({
            "filter": r.filter,
            "input_rate_hz": r.input_rate_hz,
            "target_format": r.target_format,
            "dsp_worker_throughput_x_realtime": r.worker_throughput_x,
            "dsp_sequential_throughput_x_realtime": r.sequential_throughput_x,
            "dsp_speedup_factor": r.speedup_factor,
            "parallel_efficiency_percent": r.parallel_efficiency_percent,
            "realtime_tier": match r.tier {
                Tier::Optimal => "optimal",
                Tier::Capable => "capable",
                Tier::Constrained => "constrained",
            },
            "end_to_end_throughput_x_realtime": r.end_to_end_throughput_x,
            "audio_duration_sec": r.audio_duration_sec,
            "block_frames": r.block_frames,
            "block_budget_micros": r.block_budget_micros,
            "avg_block_micros": r.avg_block_micros,
            "p50_block_micros": r.p50_block_micros,
            "p99_block_micros": r.p99_block_micros,
            "p999_block_micros": r.p999_block_micros,
            "p999_budget_percent": r.p999_budget_percent,
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        println!("=======================================================");
        println!("audigma benchmark: {}", r.filter);
        println!("=======================================================");
        println!("  Input Rate:            {} Hz", r.input_rate_hz);
        println!("  Target Format:         {}", r.target_format);
        println!(
            "  Worker Pool Rate:      {:.2}x real-time ({})",
            r.worker_throughput_x, realtime_status
        );
        println!(
            "  Sequential Baseline:   {:.2}x real-time",
            r.sequential_throughput_x
        );
        println!(
            "  Parallel Scaling:      {:.2}x speedup across 2 channels ({:.1}% efficiency)",
            r.speedup_factor, r.parallel_efficiency_percent
        );
        println!(
            "  p99.9 Block Latency:   {:.2} µs / {:.2} µs ({:.1}% of budget: {})",
            r.p999_block_micros, r.block_budget_micros, r.p999_budget_percent, budget_status
        );
        println!("  Avg Block Latency:     {:.2} µs", r.avg_block_micros);
        println!(
            "  End-to-End Rate:       {:.2}x real-time (with two-pass peak analysis)",
            r.end_to_end_throughput_x
        );
        println!("=======================================================");
    }

    Ok(())
}
