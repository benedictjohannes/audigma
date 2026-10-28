//! Production-size DSD worker benchmark, with a sequential subtile-size sweep.
//!
//! Examples (release builds only):
//! cargo run -p audigma-engine --release --example worker_benchmark -- --frames 256
//! cargo run -p audigma-engine --release --example worker_benchmark -- --mode sequential --channels 1 --subtile-frames 8
//! Linux affinity experiment (choose CPUs using `lscpu -e`):
//! cargo run -p audigma-engine --release --example worker_benchmark -- --main-cpu 4 --worker-cpus 0,2

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, FilterPhase, FilterPreset, HalfBandCascade, Modulator,
    OverloadPolicy, ScopedFtzDaz,
};
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};

fn pin_to_cpu(cpu: usize) {
    #[cfg(target_os = "linux")]
    unsafe {
        assert!(cpu < libc::CPU_SETSIZE as usize);
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
        assert_eq!(
            libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set),
            0
        );
    }
    #[cfg(not(target_os = "linux"))]
    panic!("CPU affinity is Linux-only (requested CPU {cpu})");
}

fn consume_output(bytes: &[u8], read_bytes: bool) {
    if read_bytes {
        black_box(bytes.iter().fold(0u8, |sum, byte| sum ^ byte));
    } else {
        black_box(bytes);
    }
}

fn main() {
    let mut frames = 256usize;
    let mut subtile_frames = None;
    let mut blocks = 2000usize;
    let mut channels = 2usize;
    let mut order = 7u8;
    let mut preset = FilterPreset::Reference;
    let mut rate = DsdBitRateHz::DSD512;
    let mut stages = 9;
    let mut baseband_fs = 44_100;
    let mut mode = String::from("worker");
    let mut main_cpu = None;
    let mut worker_cpus: Option<Vec<usize>> = None;
    let mut read_output = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().expect("each option requires a value");
        match arg.as_str() {
            "--frames" => frames = value.parse().unwrap(),
            "--subtile-frames" => subtile_frames = Some(value.parse::<usize>().unwrap()),
            "--blocks" => blocks = value.parse().unwrap(),
            "--channels" => channels = value.parse().unwrap(),
            "--order" => order = value.parse().unwrap(),
            "--mode" => mode = value,
            "--main-cpu" => main_cpu = Some(value.parse::<usize>().unwrap()),
            "--consume-output" => read_output = value.parse().unwrap(),
            "--worker-cpus" => {
                worker_cpus = Some(value.split(',').map(|cpu| cpu.parse().unwrap()).collect())
            }
            "--preset" => {
                preset = match value.as_str() {
                    "balanced" => FilterPreset::Balanced,
                    "reference" => FilterPreset::Reference,
                    "low-latency" => FilterPreset::LowLatency,
                    _ => panic!("unknown preset"),
                }
            }
            "--rate" => {
                (rate, stages, baseband_fs) = match value.as_str() {
                    "dsd64" => (DsdBitRateHz::DSD64, 6, 44_100),
                    "dsd128" => (DsdBitRateHz::DSD128, 7, 44_100),
                    "dsd256" => (DsdBitRateHz::DSD256, 8, 44_100),
                    "dsd512" => (DsdBitRateHz::DSD512, 9, 44_100),
                    "dsd64_48" => (DsdBitRateHz::DSD64_48, 6, 48_000),
                    "dsd128_48" => (DsdBitRateHz::DSD128_48, 7, 48_000),
                    "dsd256_48" => (DsdBitRateHz::DSD256_48, 8, 48_000),
                    "dsd512_48" => (DsdBitRateHz::DSD512_48, 9, 48_000),
                    _ => panic!("unknown rate"),
                }
            }
            _ => panic!("unknown option: {arg}"),
        }
    }
    assert!(frames > 0 && blocks > 0 && channels > 0);
    assert!(mode == "worker" || mode == "sequential");
    if let Some(ref cpus) = worker_cpus {
        assert!(mode == "worker" && cpus.len() == channels);
    }
    if let Some(cpu) = main_cpu {
        pin_to_cpu(cpu);
    }
    assert!(
        mode == "sequential" || subtile_frames.is_none(),
        "subtile override is sequential-only"
    );
    let subtile_frames = subtile_frames.unwrap_or(frames);
    assert!(subtile_frames > 0);
    let multiplier = 1 << stages;
    let input: Vec<Vec<f64>> = (0..channels)
        .map(|ch| {
            (0..frames)
                .map(|i| {
                    0.4 * (std::f64::consts::TAU * (1000 + ch * 500) as f64 * i as f64
                        / baseband_fs as f64)
                        .sin()
                })
                .collect()
        })
        .collect();
    let input_slices: Vec<&[f64]> = input.iter().map(Vec::as_slice).collect();
    let config = TargetDsdConfig {
        bit_rate: rate,
        modulator_order: order,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::Error,
        seed_base: 424242,
    };
    let _ftz = ScopedFtzDaz::new();
    let mut times = Vec::with_capacity(blocks);
    let mut pool;
    let mut cascades;
    let mut modulators;
    let mut pcm;
    let mut dsd;
    let mut process: Box<dyn FnMut()> = if mode == "worker" {
        let hook: Option<Arc<dyn Fn(usize) + Send + Sync>> = worker_cpus.clone().map(|cpus| {
            Arc::new(move |ch: usize| pin_to_cpu(cpus[ch])) as Arc<dyn Fn(usize) + Send + Sync>
        });
        pool = ChannelWorkerPool::new(
            channels,
            preset,
            FilterPhase::Linear,
            Apodization::Off,
            stages,
            frames,
            Some(config.clone()),
            hook,
        )
        .unwrap();
        Box::new(|| {
            pool.process_tile_dsd(black_box(&input_slices)).unwrap();
            for ch in 0..channels {
                consume_output(pool.channel_dsd(ch), read_output);
            }
        })
    } else {
        cascades = (0..channels)
            .map(|_| {
                HalfBandCascade::new(
                    preset,
                    FilterPhase::Linear,
                    Apodization::Off,
                    stages,
                    subtile_frames,
                )
            })
            .collect::<Vec<_>>();
        modulators = (0..channels)
            .map(|ch| {
                let mut m =
                    Modulator::new(order, rate, config.dither, config.overload_policy).unwrap();
                m.set_dither_seed(config.seed_base + ch as u64);
                m
            })
            .collect::<Vec<_>>();
        pcm = vec![vec![0.0; frames * multiplier]; channels];
        dsd = vec![vec![0; frames * multiplier / 8]; channels];
        Box::new(|| {
            for ch in 0..channels {
                for ((tile, out_pcm), out_dsd) in black_box(&input[ch])
                    .chunks(subtile_frames)
                    .zip(pcm[ch].chunks_mut(subtile_frames * multiplier))
                    .zip(dsd[ch].chunks_mut(subtile_frames * multiplier / 8))
                {
                    cascades[ch].process_tile(tile, out_pcm).unwrap();
                    modulators[ch].process_tile_bytes(out_pcm, out_dsd).unwrap();
                }
                consume_output(&dsd[ch], read_output);
            }
        })
    };
    for _ in 0..20 {
        process();
    }
    let start = Instant::now();
    for _ in 0..blocks {
        let block_start = Instant::now();
        process();
        times.push(block_start.elapsed().as_nanos() as u64);
    }
    let elapsed = start.elapsed().as_secs_f64();
    times.sort_unstable();
    let budget_us = frames as f64 / baseband_fs as f64 * 1e6;
    println!(
        "{}",
        serde_json::json!({
            "mode": mode, "rate_hz": rate.get(), "order": order,
            "preset": format!("{preset:?}"), "frames": frames, "channels": channels,
            "main_cpu": main_cpu, "worker_cpus": worker_cpus,
            "consume_output": read_output,
            "sequential_subtile_frames": if mode == "sequential" { Some(subtile_frames) } else { None },
            "blocks": blocks, "throughput_x_realtime": blocks as f64 * budget_us / 1e6 / elapsed,
            "p50_us": times[blocks / 2] as f64 / 1000.0,
            "p999_us": times[blocks * 999 / 1000] as f64 / 1000.0,
            "p999_percent_of_budget": times[blocks * 999 / 1000] as f64 / 1000.0 / budget_us * 100.0,
        })
    );
}
