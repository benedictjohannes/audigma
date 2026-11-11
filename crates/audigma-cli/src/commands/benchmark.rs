//! Handler for `audigma benchmark` subcommand.

use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::{AudioInput, DecoderInput};
use audigma_engine::{ChannelWorkerPool, MockAudioInput, OfflinePipeline, TargetDsdConfig};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_settings::{OutputContainer, Settings};
use clap::Args;
use serde_json::json;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::time::Instant;

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
        let info =
            AudioStreamInfo::new_pcm(PcmRateHz::PCM_44100, 2, Some(total_frames as u64), Some(24));
        (
            Box::new(MockAudioInput::new(info, left, right, true)),
            PcmRateHz::PCM_44100,
        )
    };

    // Sink into in-memory cursor or dev-null-like sink
    let output = Box::new(std::io::Cursor::new(Vec::new()));

    let pipeline = {
        let placement = crate::runtime::resolve(&settings, 2, true)?;
        crate::runtime::apply_housekeeping(&placement);
        OfflinePipeline::prepare_with_workers(
            settings.clone(),
            input_rate,
            Some(crate::runtime::worker_setup(&placement)),
        )?
    };
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
    let amp = 10.0f64.powf(-6.0 / 20.0);

    let mut bb_l = [0.0f64; BLOCK_FRAMES];
    let mut bb_r = [0.0f64; BLOCK_FRAMES];
    for i in 0..BLOCK_FRAMES {
        let t = i as f64 / input_rate.get() as f64;
        bb_l[i] = amp * (2.0 * PI * 1000.0 * t).sin();
        bb_r[i] = amp * (2.0 * PI * 2000.0 * t).sin();
    }

    // 2.A: Single-Threaded Sequential Baseline (Stereo L + R on 1 thread)
    let mut seq_cascade_l = audigma_dsp::fir::HalfBandCascade::new(
        plan.dsp_config.phase,
        plan.dsp_config.apodization,
        stages,
        BLOCK_FRAMES,
    );
    let mut seq_cascade_r = audigma_dsp::fir::HalfBandCascade::new(
        plan.dsp_config.phase,
        plan.dsp_config.apodization,
        stages,
        BLOCK_FRAMES,
    );
    let mut exp_l = vec![0.0f64; tile_expanded];
    let mut exp_r = vec![0.0f64; tile_expanded];
    let mut dsd_bytes_l = vec![0u8; tile_expanded / 8];
    let mut dsd_bytes_r = vec![0u8; tile_expanded / 8];

    let mut seq_mod_l = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            let mut m = audigma_dsp::modulator::Modulator::new(
                plan.dsp_config.modulator_order,
                bit_rate,
                audigma_dsp::config::DitherType::Tpdf,
                audigma_dsp::config::OverloadPolicy::MuteRecover,
            )
            .unwrap();
            m.set_dither_seed(12345);
            Some(m)
        } else {
            None
        }
    } else {
        None
    };

    let mut seq_mod_r = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            let mut m = audigma_dsp::modulator::Modulator::new(
                plan.dsp_config.modulator_order,
                bit_rate,
                audigma_dsp::config::DitherType::Tpdf,
                audigma_dsp::config::OverloadPolicy::MuteRecover,
            )
            .unwrap();
            m.set_dither_seed(12346);
            Some(m)
        } else {
            None
        }
    } else {
        None
    };

    // Sequential Warm-up
    for _ in 0..50 {
        seq_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut exp_r).unwrap();
        if let (Some(ml), Some(mr)) = (&mut seq_mod_l, &mut seq_mod_r) {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
            mr.process_tile_bytes(&exp_r, &mut dsd_bytes_r).unwrap();
        }
    }

    const BENCH_BLOCKS: usize = 2000;
    let seq_start = Instant::now();
    for _ in 0..BENCH_BLOCKS {
        seq_cascade_l.process_tile(&bb_l, &mut exp_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut exp_r).unwrap();
        if let (Some(ml), Some(mr)) = (&mut seq_mod_l, &mut seq_mod_r) {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l).unwrap();
            mr.process_tile_bytes(&exp_r, &mut dsd_bytes_r).unwrap();
        }
    }
    let seq_total_elapsed = seq_start.elapsed();

    // 2.B: Multi-Threaded ChannelWorkerPool (1 dedicated RT thread per channel)
    let target_dsd = if is_dsd {
        if let audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } = target_spec {
            Some(TargetDsdConfig {
                bit_rate,
                modulator_order: plan.dsp_config.modulator_order,
                dither: audigma_dsp::config::DitherType::Tpdf,
                overload_policy: audigma_dsp::config::OverloadPolicy::MuteRecover,
                seed_base: 12345,
            })
        } else {
            None
        }
    } else {
        None
    };

    let mut worker_pool = ChannelWorkerPool::new(
        2,
        plan.dsp_config.phase,
        plan.dsp_config.apodization,
        stages,
        BLOCK_FRAMES,
        target_dsd,
        None,
    )?;

    // Worker Pool Warm-up
    for _ in 0..50 {
        if is_dsd {
            worker_pool.process_tile_dsd(&[&bb_l, &bb_r]).unwrap();
        } else {
            worker_pool.process_tile_pcm(&[&bb_l, &bb_r]).unwrap();
        }
    }

    let mut worker_block_times_ns = Vec::with_capacity(BENCH_BLOCKS);
    let worker_start = Instant::now();

    for _ in 0..BENCH_BLOCKS {
        let b_start = Instant::now();
        if is_dsd {
            worker_pool.process_tile_dsd(&[&bb_l, &bb_r]).unwrap();
        } else {
            worker_pool.process_tile_pcm(&[&bb_l, &bb_r]).unwrap();
        }
        worker_block_times_ns.push(b_start.elapsed().as_nanos() as u64);
    }
    let worker_total_elapsed = worker_start.elapsed();

    worker_block_times_ns.sort_unstable();
    let dsp_audio_duration_sec =
        BENCH_BLOCKS as f64 * (BLOCK_FRAMES as f64 / input_rate.get() as f64);
    let worker_throughput_x = dsp_audio_duration_sec / worker_total_elapsed.as_secs_f64();
    let seq_throughput_x = dsp_audio_duration_sec / seq_total_elapsed.as_secs_f64();
    let speedup_factor = seq_total_elapsed.as_secs_f64() / worker_total_elapsed.as_secs_f64();

    let p50_ns = worker_block_times_ns[BENCH_BLOCKS * 50 / 100];
    let p99_ns = worker_block_times_ns[BENCH_BLOCKS * 99 / 100];
    let p999_ns = worker_block_times_ns[BENCH_BLOCKS * 999 / 1000];
    let avg_ns = worker_block_times_ns.iter().sum::<u64>() / BENCH_BLOCKS as u64;

    let block_budget_micros = (BLOCK_FRAMES as f64 / input_rate.get() as f64) * 1_000_000.0;
    let p999_micros = p999_ns as f64 / 1000.0;
    let p999_budget_used_percent = (p999_micros / block_budget_micros) * 100.0;
    let efficiency_pct = (speedup_factor / 2.0) * 100.0;

    let realtime_status = if worker_throughput_x >= 2.0 {
        "Optimal (>= 2x realtime margin)"
    } else if worker_throughput_x >= 1.0 {
        "Capable (realtime playback supported)"
    } else {
        "Constrained (sub-realtime; offline conversion recommended)"
    };

    let budget_status = if p999_budget_used_percent <= 50.0 {
        "Ample RT headroom"
    } else if p999_budget_used_percent <= 70.0 {
        "Within safe RT limits"
    } else {
        "High CPU utilization for 64-frame block"
    };

    if args.json {
        let json_val = json!({
            "filter": plan.dsp_config.filter_description(),
            "input_rate_hz": input_rate.get(),
            "target_format": format!("{:?}", report.target_format),
            "dsp_worker_throughput_x_realtime": worker_throughput_x,
            "dsp_sequential_throughput_x_realtime": seq_throughput_x,
            "dsp_speedup_factor": speedup_factor,
            "parallel_efficiency_percent": efficiency_pct,
            "realtime_tier": if worker_throughput_x >= 2.0 { "optimal" } else if worker_throughput_x >= 1.0 { "capable" } else { "constrained" },
            "end_to_end_throughput_x_realtime": e2e_throughput_x,
            "audio_duration_sec": audio_duration_sec,
            "block_frames": BLOCK_FRAMES,
            "block_budget_micros": block_budget_micros,
            "avg_block_micros": avg_ns as f64 / 1000.0,
            "p50_block_micros": p50_ns as f64 / 1000.0,
            "p99_block_micros": p99_ns as f64 / 1000.0,
            "p999_block_micros": p999_micros,
            "p999_budget_percent": p999_budget_used_percent,
        });
        println!("{}", serde_json::to_string_pretty(&json_val).unwrap());
    } else {
        println!("=======================================================");
        println!(
            "audigma benchmark: {}",
            plan.dsp_config.filter_description()
        );
        println!("=======================================================");
        println!("  Input Rate:            {} Hz", input_rate.get());
        println!("  Target Format:         {:?}", report.target_format);
        println!(
            "  Worker Pool Rate:      {:.2}x real-time ({})",
            worker_throughput_x, realtime_status
        );
        println!(
            "  Sequential Baseline:   {:.2}x real-time",
            seq_throughput_x
        );
        println!(
            "  Parallel Scaling:      {:.2}x speedup across 2 channels ({:.1}% efficiency)",
            speedup_factor, efficiency_pct
        );
        println!(
            "  p99.9 Block Latency:   {:.2} µs / {:.2} µs ({:.1}% of budget: {})",
            p999_micros, block_budget_micros, p999_budget_used_percent, budget_status
        );
        println!("  Avg Block Latency:     {:.2} µs", avg_ns as f64 / 1000.0);
        println!(
            "  End-to-End Rate:       {:.2}x real-time (with two-pass peak analysis)",
            e2e_throughput_x
        );
        println!("=======================================================");
    }

    Ok(())
}
