//! Measurement and benchmark harness for `audigma-dsp` spectral verification (`Design.md §8`).
//!
//! Generates test signals, runs direct and cascaded DSP chains with FTZ/DAZ active,
//! and writes raw bitstreams and JSON telemetry for analysis by `tools/measure/measure_spectral.py`.

use std::f64::consts::PI;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

use audigma_dsp::{
    CrfbModulator, DitherType, DsdBitRateHz, FilterPreset, HalfBandCascade, OverloadPolicy,
    ScopedFtzDaz, get_crfb_coefficients,
};

#[derive(Debug, Clone)]
struct HarnessConfig {
    mode: String,
    rate: String,
    preset: FilterPreset,
    samples: usize,
    amp_db: f64,
    dither: DitherType,
    overload_policy: OverloadPolicy,
    out_bin: Option<String>,
    out_json: Option<String>,
}

fn parse_args() -> HarnessConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut config = HarnessConfig {
        mode: "direct-sine".to_string(),
        rate: "dsd64".to_string(),
        preset: FilterPreset::Balanced,
        samples: 1_048_576,
        amp_db: -6.0,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        out_bin: None,
        out_json: None,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                i += 1;
                config.mode = args[i].clone();
            }
            "--rate" => {
                i += 1;
                config.rate = args[i].clone();
            }
            "--preset" => {
                i += 1;
                config.preset = match args[i].as_str() {
                    "balanced" => FilterPreset::Balanced,
                    "reference" => FilterPreset::Reference,
                    "low-latency" => FilterPreset::LowLatency,
                    other => panic!("Unknown preset: {other}"),
                };
            }
            "--samples" => {
                i += 1;
                config.samples = args[i].parse().expect("Invalid samples");
            }
            "--amp-db" => {
                i += 1;
                config.amp_db = args[i].parse().expect("Invalid amp-db");
            }
            "--dither" => {
                i += 1;
                config.dither = match args[i].as_str() {
                    "tpdf" => DitherType::Tpdf,
                    "none" => DitherType::None,
                    other => panic!("Unknown dither: {other}"),
                };
            }
            "--overload-policy" => {
                i += 1;
                config.overload_policy = match args[i].as_str() {
                    "mute-recover" => OverloadPolicy::MuteRecover,
                    "error" => OverloadPolicy::Error,
                    other => panic!("Unknown overload policy: {other}"),
                };
            }
            "--out-bin" => {
                i += 1;
                config.out_bin = Some(args[i].clone());
            }
            "--out-json" => {
                i += 1;
                config.out_json = Some(args[i].clone());
            }
            other => panic!("Unknown argument: {other}"),
        }
        i += 1;
    }

    config
}

fn parse_rate(rate_str: &str) -> (DsdBitRateHz, u32, usize, u32) {
    match rate_str {
        "dsd64" => (DsdBitRateHz::DSD64, 2_822_400, 6, 44_100),
        "dsd128" => (DsdBitRateHz::DSD128, 5_644_800, 7, 44_100),
        "dsd256" => (DsdBitRateHz::DSD256, 11_289_600, 8, 44_100),
        "dsd64_48" => (DsdBitRateHz::DSD64_48, 3_072_000, 6, 48_000),
        "dsd128_48" => (DsdBitRateHz::DSD128_48, 6_144_000, 7, 48_000),
        "dsd256_48" => (DsdBitRateHz::DSD256_48, 12_288_000, 8, 48_000),
        other => panic!("Unknown rate: {other}"),
    }
}

fn main() {
    let _ftz = ScopedFtzDaz::new();
    let config = parse_args();
    let (bit_rate, f_bit_hz, cascade_stages, baseband_fs) = parse_rate(&config.rate);
    let coeffs = *get_crfb_coefficients(bit_rate)
        .unwrap_or_else(|| panic!("Coefficients not found for {}", config.rate));

    let mut modulator = CrfbModulator::new(coeffs, config.dither, config.overload_policy);
    modulator.set_dither_seed(12345);

    match config.mode.as_str() {
        "direct-sine" => {
            run_direct_sine(&config, &mut modulator, f_bit_hz);
        }
        "cascade-sine" => {
            run_cascade_sine(
                &config,
                &mut modulator,
                f_bit_hz,
                cascade_stages,
                baseband_fs,
            );
        }
        "silence" => {
            run_silence(&config, &mut modulator, f_bit_hz);
        }
        "dc" => {
            run_dc(&config, &mut modulator, f_bit_hz);
        }
        "multitone" => {
            run_multitone(
                &config,
                &mut modulator,
                f_bit_hz,
                cascade_stages,
                baseband_fs,
            );
        }
        "overload" => {
            run_overload(&config, &mut modulator, f_bit_hz);
        }
        "benchmark" => {
            run_benchmark(&config, &mut modulator, cascade_stages, baseband_fs);
        }
        other => panic!("Unknown mode: {other}"),
    }
}

fn run_direct_sine(config: &HarnessConfig, modulator: &mut CrfbModulator, f_bit_hz: u32) {
    let n = config.samples;
    let k_bin = ((1000.0 * n as f64) / (f_bit_hz as f64)).round() as u64;
    let f_tone_hz = (k_bin as f64) * (f_bit_hz as f64) / (n as f64);
    let amp = 10.0_f64.powf(config.amp_db / 20.0);

    // Warm-up with continuous periodic phase matching the steady-state buffer
    let n_warmup = 32_768i64;
    for i in -n_warmup..0 {
        let phase = 2.0 * PI * (k_bin as f64) * (i as f64) / (n as f64);
        let s = amp * phase.sin();
        let _ = modulator.step_sample(s);
    }

    let mut raw_bytes = vec![0u8; n / 8];
    const TILE_BITS: usize = 4096;
    let mut pcm_tile = [0.0f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];

    let start = Instant::now();
    let num_tiles = n / TILE_BITS;
    let mut max_state = [0.0f64; 5];

    for t_idx in 0..num_tiles {
        let bit_offset = t_idx * TILE_BITS;
        for (i, pcm) in pcm_tile.iter_mut().enumerate() {
            let sample_idx = bit_offset + i;
            let phase = 2.0 * PI * (k_bin as f64) * (sample_idx as f64) / (n as f64);
            *pcm = amp * phase.sin();
        }

        modulator
            .process_tile_bytes(&pcm_tile, &mut tile_bytes)
            .unwrap();
        raw_bytes[t_idx * (TILE_BITS / 8)..(t_idx + 1) * (TILE_BITS / 8)]
            .copy_from_slice(&tile_bytes);

        for (cur_max, &s) in max_state.iter_mut().zip(&modulator.state()) {
            if s.abs() > *cur_max {
                *cur_max = s.abs();
            }
        }
    }
    let elapsed = start.elapsed();

    if let Some(ref path) = config.out_bin {
        let mut f = File::create(path).unwrap();
        f.write_all(&raw_bytes).unwrap();
    }

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "f_bit_hz": {},
  "f_tone_hz": {:.6},
  "k_bin": {},
  "samples": {},
  "amp_db": {:.2},
  "max_state": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "overload_count": {},
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            f_bit_hz,
            f_tone_hz,
            k_bin,
            n,
            config.amp_db,
            max_state[0],
            max_state[1],
            max_state[2],
            max_state[3],
            max_state[4],
            modulator.overload_count(),
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_cascade_sine(
    config: &HarnessConfig,
    modulator: &mut CrfbModulator,
    f_bit_hz: u32,
    cascade_stages: usize,
    baseband_fs: u32,
) {
    let n = config.samples;
    let multiplier = 1usize << cascade_stages;
    let n_baseband = n / multiplier;
    let k_bin = ((1000.0 * n as f64) / (f_bit_hz as f64)).round() as u64;
    let f_tone_hz = (k_bin as f64) * (f_bit_hz as f64) / (n as f64);
    let amp = 10.0_f64.powf(config.amp_db / 20.0);

    const TILE_BASEBAND: usize = 64;
    let tile_expanded = TILE_BASEBAND * multiplier;
    let mut cascade = HalfBandCascade::new(config.preset, cascade_stages, TILE_BASEBAND);

    // Warm-up cascade and modulator so delay lines are completely full
    let group_delay_baseband = cascade.total_group_delay_output_samples() / multiplier + 128;
    let warmup_tiles = (group_delay_baseband / TILE_BASEBAND) + 4;
    let mut bb_scratch = [0.0f64; TILE_BASEBAND];
    let mut exp_scratch = vec![0.0f64; tile_expanded];
    let mut out_tile_bytes = vec![0u8; tile_expanded / 8];

    for w_idx in (0..warmup_tiles).rev() {
        let neg_offset = -((w_idx as i64 + 1) * TILE_BASEBAND as i64);
        for (i, bb) in bb_scratch.iter_mut().enumerate() {
            let sample_idx = neg_offset + i as i64;
            let phase = 2.0 * PI * (k_bin as f64) * (sample_idx as f64) / (n_baseband as f64);
            *bb = amp * phase.sin();
        }
        cascade.process_tile(&bb_scratch, &mut exp_scratch).unwrap();
        modulator
            .process_tile_bytes(&exp_scratch, &mut out_tile_bytes)
            .unwrap();
    }

    let mut raw_bytes = vec![0u8; n / 8];
    let num_tiles = n_baseband / TILE_BASEBAND;
    let mut max_state = [0.0f64; 5];
    let start = Instant::now();

    for t_idx in 0..num_tiles {
        let base_idx = t_idx * TILE_BASEBAND;
        for (i, bb) in bb_scratch.iter_mut().enumerate() {
            let sample_idx = base_idx + i;
            let phase = 2.0 * PI * (k_bin as f64) * (sample_idx as f64) / (n_baseband as f64);
            *bb = amp * phase.sin();
        }

        cascade.process_tile(&bb_scratch, &mut exp_scratch).unwrap();
        modulator
            .process_tile_bytes(&exp_scratch, &mut out_tile_bytes)
            .unwrap();

        let byte_offset = t_idx * (tile_expanded / 8);
        raw_bytes[byte_offset..byte_offset + (tile_expanded / 8)].copy_from_slice(&out_tile_bytes);

        for (cur_max, &s) in max_state.iter_mut().zip(&modulator.state()) {
            if s.abs() > *cur_max {
                *cur_max = s.abs();
            }
        }
    }
    let elapsed = start.elapsed();

    if let Some(ref path) = config.out_bin {
        let mut f = File::create(path).unwrap();
        f.write_all(&raw_bytes).unwrap();
    }

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "preset": "{:?}",
  "f_bit_hz": {},
  "baseband_fs": {},
  "cascade_stages": {},
  "f_tone_hz": {:.6},
  "k_bin": {},
  "samples": {},
  "amp_db": {:.2},
  "max_state": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "overload_count": {},
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            config.preset,
            f_bit_hz,
            baseband_fs,
            cascade_stages,
            f_tone_hz,
            k_bin,
            n,
            config.amp_db,
            max_state[0],
            max_state[1],
            max_state[2],
            max_state[3],
            max_state[4],
            modulator.overload_count(),
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_silence(config: &HarnessConfig, modulator: &mut CrfbModulator, f_bit_hz: u32) {
    let n = config.samples;
    let mut raw_bytes = vec![0u8; n / 8];
    const TILE_BITS: usize = 4096;
    let silence_tile = [0.0f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];
    let mut max_state = [0.0f64; 5];

    let start = Instant::now();
    for t_idx in 0..(n / TILE_BITS) {
        modulator
            .process_tile_bytes(&silence_tile, &mut tile_bytes)
            .unwrap();
        raw_bytes[t_idx * (TILE_BITS / 8)..(t_idx + 1) * (TILE_BITS / 8)]
            .copy_from_slice(&tile_bytes);

        for (cur_max, &s) in max_state.iter_mut().zip(&modulator.state()) {
            if s.abs() > *cur_max {
                *cur_max = s.abs();
            }
        }
    }
    let elapsed = start.elapsed();

    let total_ones: usize = raw_bytes.iter().map(|b| b.count_ones() as usize).sum();
    let ones_ratio = total_ones as f64 / n as f64;

    if let Some(ref path) = config.out_bin {
        let mut f = File::create(path).unwrap();
        f.write_all(&raw_bytes).unwrap();
    }

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "f_bit_hz": {},
  "samples": {},
  "ones_ratio": {:.6},
  "max_state": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "overload_count": {},
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            f_bit_hz,
            n,
            ones_ratio,
            max_state[0],
            max_state[1],
            max_state[2],
            max_state[3],
            max_state[4],
            modulator.overload_count(),
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_dc(config: &HarnessConfig, modulator: &mut CrfbModulator, f_bit_hz: u32) {
    let n = config.samples;
    let mut raw_bytes = vec![0u8; n / 8];
    const TILE_BITS: usize = 4096;
    let dc_val = 0.5f64; // +0.5 normalized DC within envelope
    let dc_tile = [dc_val; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];
    let mut max_state = [0.0f64; 5];

    let start = Instant::now();
    for t_idx in 0..(n / TILE_BITS) {
        modulator
            .process_tile_bytes(&dc_tile, &mut tile_bytes)
            .unwrap();
        raw_bytes[t_idx * (TILE_BITS / 8)..(t_idx + 1) * (TILE_BITS / 8)]
            .copy_from_slice(&tile_bytes);

        for (cur_max, &s) in max_state.iter_mut().zip(&modulator.state()) {
            if s.abs() > *cur_max {
                *cur_max = s.abs();
            }
        }
    }
    let elapsed = start.elapsed();

    let total_ones: usize = raw_bytes.iter().map(|b| b.count_ones() as usize).sum();
    let ones_ratio = total_ones as f64 / n as f64;

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "f_bit_hz": {},
  "dc_val": {:.4},
  "samples": {},
  "ones_ratio": {:.6},
  "max_state": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "overload_count": {},
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            f_bit_hz,
            dc_val,
            n,
            ones_ratio,
            max_state[0],
            max_state[1],
            max_state[2],
            max_state[3],
            max_state[4],
            modulator.overload_count(),
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_multitone(
    config: &HarnessConfig,
    modulator: &mut CrfbModulator,
    f_bit_hz: u32,
    cascade_stages: usize,
    baseband_fs: u32,
) {
    let n = config.samples;
    let multiplier = 1usize << cascade_stages;
    let n_baseband = n / multiplier;

    // 5 tones: 50, 500, 1000, 5000, 15000 Hz, scaled to sum <= -6 dBFS peak
    let freqs = [50.0, 500.0, 1000.0, 5000.0, 15000.0];
    let tone_amp = (10.0_f64.powf(-6.0 / 20.0)) / (freqs.len() as f64);

    const TILE_BASEBAND: usize = 64;
    let tile_expanded = TILE_BASEBAND * multiplier;
    let mut cascade = HalfBandCascade::new(config.preset, cascade_stages, TILE_BASEBAND);

    let mut bb_scratch = [0.0f64; TILE_BASEBAND];
    let mut exp_scratch = vec![0.0f64; tile_expanded];
    let mut out_tile_bytes = vec![0u8; tile_expanded / 8];
    let mut max_state = [0.0f64; 5];

    let start = Instant::now();
    for t_idx in 0..(n_baseband / TILE_BASEBAND) {
        let base_idx = t_idx * TILE_BASEBAND;
        for (i, bb) in bb_scratch.iter_mut().enumerate() {
            let t = (base_idx + i) as f64 / baseband_fs as f64;
            let mut sum = 0.0;
            for &f in &freqs {
                sum += tone_amp * (2.0 * PI * f * t).sin();
            }
            *bb = sum;
        }

        cascade.process_tile(&bb_scratch, &mut exp_scratch).unwrap();
        modulator
            .process_tile_bytes(&exp_scratch, &mut out_tile_bytes)
            .unwrap();

        for (cur_max, &s) in max_state.iter_mut().zip(&modulator.state()) {
            if s.abs() > *cur_max {
                *cur_max = s.abs();
            }
        }
    }
    let elapsed = start.elapsed();

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "f_bit_hz": {},
  "samples": {},
  "max_state": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "overload_count": {},
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            f_bit_hz,
            n,
            max_state[0],
            max_state[1],
            max_state[2],
            max_state[3],
            max_state[4],
            modulator.overload_count(),
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_overload(config: &HarnessConfig, modulator: &mut CrfbModulator, f_bit_hz: u32) {
    let n = config.samples;
    let mut raw_bytes = vec![0u8; n / 8];
    const TILE_BITS: usize = 4096;
    let mut pcm_tile = [0.0f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];
    let amp = 10.0_f64.powf(-6.0 / 20.0);

    let mut overload_occurred = false;
    let mut state_after_recovery = [0.0f64; 5];

    let start = Instant::now();
    for t_idx in 0..(n / TILE_BITS) {
        let bit_offset = t_idx * TILE_BITS;
        for (i, pcm) in pcm_tile.iter_mut().enumerate() {
            let sample_idx = bit_offset + i;
            let t = sample_idx as f64 / f_bit_hz as f64;
            // Inject +5.0 severe overload at t_idx == 10
            if t_idx == 10 && i < 64 {
                *pcm = 5.0;
                overload_occurred = true;
            } else {
                *pcm = amp * (2.0 * PI * 1000.0 * t).sin();
            }
        }

        modulator
            .process_tile_bytes(&pcm_tile, &mut tile_bytes)
            .unwrap();
        raw_bytes[t_idx * (TILE_BITS / 8)..(t_idx + 1) * (TILE_BITS / 8)]
            .copy_from_slice(&tile_bytes);

        if t_idx > 12 {
            state_after_recovery = modulator.state();
        }
    }
    let elapsed = start.elapsed();

    let recovered_stable = state_after_recovery
        .iter()
        .all(|&s| s.abs() < CrfbModulator::STABILITY_THRESHOLD);

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "f_bit_hz": {},
  "samples": {},
  "overload_injected": {},
  "overload_count": {},
  "recovered_stable": {},
  "state_after_recovery": [{:.6}, {:.6}, {:.6}, {:.6}, {:.6}],
  "elapsed_micros": {}
}}"#,
            config.mode,
            config.rate,
            f_bit_hz,
            n,
            overload_occurred,
            modulator.overload_count(),
            recovered_stable,
            state_after_recovery[0],
            state_after_recovery[1],
            state_after_recovery[2],
            state_after_recovery[3],
            state_after_recovery[4],
            elapsed.as_micros()
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_benchmark(
    config: &HarnessConfig,
    modulator: &mut CrfbModulator,
    cascade_stages: usize,
    baseband_fs: u32,
) {
    let multiplier = 1usize << cascade_stages;
    const TILE_BASEBAND: usize = 64; // Block size = 64 frames (e.g. 1.45 ms at 44.1 kHz)
    let tile_expanded = TILE_BASEBAND * multiplier;

    let mut cascade = HalfBandCascade::new(config.preset, cascade_stages, TILE_BASEBAND);
    let mut bb_scratch = [0.0f64; TILE_BASEBAND];
    let mut exp_scratch = vec![0.0f64; tile_expanded];
    let mut out_tile_bytes = vec![0u8; tile_expanded / 8];

    // Pre-populate input with sine tone
    let amp = 10.0_f64.powf(-6.0 / 20.0);
    for (i, s) in bb_scratch.iter_mut().enumerate() {
        let t = i as f64 / baseband_fs as f64;
        *s = amp * (2.0 * PI * 1000.0 * t).sin();
    }

    // Warm up cache and pipelines
    for _ in 0..100 {
        cascade.process_tile(&bb_scratch, &mut exp_scratch).unwrap();
        modulator
            .process_tile_bytes(&exp_scratch, &mut out_tile_bytes)
            .unwrap();
    }

    const BENCH_BLOCKS: usize = 2000;
    let mut block_times_ns = Vec::with_capacity(BENCH_BLOCKS);

    let total_start = Instant::now();
    for _ in 0..BENCH_BLOCKS {
        let block_start = Instant::now();
        cascade.process_tile(&bb_scratch, &mut exp_scratch).unwrap();
        modulator
            .process_tile_bytes(&exp_scratch, &mut out_tile_bytes)
            .unwrap();
        block_times_ns.push(block_start.elapsed().as_nanos() as u64);
    }
    let total_elapsed = total_start.elapsed();

    block_times_ns.sort_unstable();
    let min_ns = block_times_ns[0];
    let max_ns = *block_times_ns.last().unwrap();
    let p50_ns = block_times_ns[BENCH_BLOCKS * 50 / 100];
    let p90_ns = block_times_ns[BENCH_BLOCKS * 90 / 100];
    let p99_ns = block_times_ns[BENCH_BLOCKS * 99 / 100];
    let p999_ns = block_times_ns[BENCH_BLOCKS * 999 / 1000];
    let avg_ns = block_times_ns.iter().sum::<u64>() / BENCH_BLOCKS as u64;

    let audio_block_duration_sec = TILE_BASEBAND as f64 / baseband_fs as f64;
    let audio_block_duration_micros = audio_block_duration_sec * 1_000_000.0;
    let total_audio_duration_sec = BENCH_BLOCKS as f64 * audio_block_duration_sec;
    let throughput_x_realtime = total_audio_duration_sec / total_elapsed.as_secs_f64();
    let p999_percent_of_budget = ((p999_ns as f64 / 1_000.0) / audio_block_duration_micros) * 100.0;

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "preset": "{:?}",
  "bench_blocks": {},
  "baseband_block_frames": {},
  "audio_block_duration_micros": {:.2},
  "throughput_x_realtime": {:.2},
  "min_micros": {:.3},
  "avg_micros": {:.3},
  "p50_micros": {:.3},
  "p90_micros": {:.3},
  "p99_micros": {:.3},
  "p999_micros": {:.3},
  "max_micros": {:.3},
  "p999_percent_of_budget": {:.2}
}}"#,
            config.mode,
            config.rate,
            config.preset,
            BENCH_BLOCKS,
            TILE_BASEBAND,
            audio_block_duration_micros,
            throughput_x_realtime,
            min_ns as f64 / 1000.0,
            avg_ns as f64 / 1000.0,
            p50_ns as f64 / 1000.0,
            p90_ns as f64 / 1000.0,
            p99_ns as f64 / 1000.0,
            p999_ns as f64 / 1000.0,
            max_ns as f64 / 1000.0,
            p999_percent_of_budget
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}
