//! Measurement and benchmark harness for the `audigma-dsp` spectral verification (`Design.md §8`).
//!
//! Generates test signals, runs direct and cascaded DSP chains with FTZ/DAZ active,
//! and writes raw bitstreams and JSON telemetry for analysis by `tools/measure/measure_spectral.py`.

use std::f64::consts::PI;
use std::fs::File;
use std::io::Write;
use std::time::Instant;

mod linear_modes;

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, FilterPhase, HalfBandCascade, Modulator, OverloadPolicy,
    STABILITY_THRESHOLD, ScopedFtzDaz,
};

#[derive(Debug, Clone)]
struct HarnessConfig {
    mode: String,
    rate: String,
    order: u8,
    apodization: Apodization,
    samples: usize,
    amp_db: f64,
    dither: DitherType,
    overload_policy: OverloadPolicy,
    out_bin: Option<String>,
    out_json: Option<String>,
    /// Pin the process to the best physical core via the runtime placement resolver.
    pin: bool,
}

fn parse_args() -> HarnessConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut config = HarnessConfig {
        mode: "direct-sine".to_string(),
        rate: "dsd64".to_string(),
        order: 5,
        apodization: Apodization::Off,
        samples: 1_048_576,
        amp_db: -6.0,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        out_bin: None,
        out_json: None,
        pin: false,
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
            "--order" => {
                i += 1;
                config.order = args[i].parse().expect("Invalid order");
            }
            "--apodization" => {
                i += 1;
                config.apodization = Apodization::from_name(&args[i])
                    .unwrap_or_else(|| panic!("Unknown apodization: {}", args[i]));
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
            "--pin" => {
                config.pin = true;
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
        "dsd512" => (DsdBitRateHz::DSD512, 22_579_200, 9, 44_100),
        "dsd64_48" => (DsdBitRateHz::DSD64_48, 3_072_000, 6, 48_000),
        "dsd128_48" => (DsdBitRateHz::DSD128_48, 6_144_000, 7, 48_000),
        "dsd256_48" => (DsdBitRateHz::DSD256_48, 12_288_000, 8, 48_000),
        "dsd512_48" => (DsdBitRateHz::DSD512_48, 24_576_000, 9, 48_000),
        other => panic!("Unknown rate: {other}"),
    }
}

fn format_state_json(state: &[f64]) -> String {
    let elems: Vec<String> = state.iter().map(|s| format!("{s:.6}")).collect();
    format!("[{}]", elems.join(", "))
}

/// Pins the calling thread to the core the runtime would give a single DSP worker
/// (same resolver as `audigma run`; normal scheduling class, no real-time request).
/// Returns a short description for the JSON report.
fn pin_like_runtime() -> String {
    use audigma_settings::{DspThreads, RealtimeMode, RuntimeConfig};
    let cfg = RuntimeConfig {
        dsp_threads: DspThreads::Count(1),
        realtime: RealtimeMode::Off,
        ..RuntimeConfig::default()
    };
    let topo = match audigma_io::read_cpu_topology() {
        Ok(t) => t,
        Err(e) => return format!("unpinned (topology unavailable: {e})"),
    };
    let placement = match audigma_engine::resolve_placement(Some(&topo), &cfg, 1) {
        Ok(p) => p,
        Err(e) => return format!("unpinned (placement failed: {e})"),
    };
    let Some(tp) = placement.dsp.first() else {
        return "unpinned (no worker placement)".to_string();
    };
    match audigma_io::apply(tp) {
        Ok(_) => match &tp.cpus {
            Some(c) => {
                let info = topo.cpus.iter().find(|i| Some(&i.id) == c.first());
                let perf = info
                    .and_then(|i| i.highest_perf)
                    .map(|p| format!(", highest_perf {p}"))
                    .unwrap_or_default();
                format!("cpu {:?}{perf}", c)
            }
            None => "unpinned (resolver returned no cpus)".to_string(),
        },
        Err(e) => format!("unpinned (apply failed: {e})"),
    }
}

fn main() {
    let _ftz = ScopedFtzDaz::new();

    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.iter().any(|a| a == "--probe-env") {
        println!(
            r#"{{"ftz_daz_active": {}, "target_arch": "{}", "target_os": "{}"}}"#,
            ScopedFtzDaz::is_active(),
            std::env::consts::ARCH,
            std::env::consts::OS
        );
        return;
    }

    if linear_modes::dispatch(&raw_args) {
        return;
    }

    let config = parse_args();
    let pin_desc = if config.pin {
        pin_like_runtime()
    } else {
        "unpinned (not requested)".to_string()
    };
    let (bit_rate, f_bit_hz, cascade_stages, baseband_fs) = parse_rate(&config.rate);

    let mut modulator = Modulator::new(
        config.order,
        bit_rate,
        config.dither,
        config.overload_policy,
    )
    .unwrap_or_else(|e| panic!("Failed to initialize modulator: {e}"));
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
        "input-limit" => {
            run_input_limit(&config, &mut modulator, bit_rate, f_bit_hz);
        }
        "benchmark" => {
            run_benchmark(
                &config,
                &mut modulator,
                cascade_stages,
                baseband_fs,
                &pin_desc,
            );
        }
        other => panic!("Unknown mode: {other}"),
    }
}

fn run_direct_sine(config: &HarnessConfig, modulator: &mut Modulator, f_bit_hz: u32) {
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
    let mut max_state = vec![0.0f64; modulator.state_slice().len()];

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

        for (cur_max, &s) in max_state.iter_mut().zip(modulator.state_slice()) {
            *cur_max = cur_max.max(s.abs());
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
  "order": {},
  "samples": {},
  "k_bin": {},
  "f_tone_hz": {:.6},
  "amp_db": {:.2},
  "elapsed_seconds": {:.6},
  "overload_count": {},
  "max_state": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            n,
            k_bin,
            f_tone_hz,
            config.amp_db,
            elapsed.as_secs_f64(),
            modulator.overload_count(),
            format_state_json(&max_state)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_cascade_sine(
    config: &HarnessConfig,
    modulator: &mut Modulator,
    f_bit_hz: u32,
    cascade_stages: usize,
    baseband_fs: u32,
) {
    let multiplier = 1usize << cascade_stages;
    let n = config.samples;
    let k_bin = ((1000.0 * n as f64) / (f_bit_hz as f64)).round() as u64;
    let f_tone_hz = (k_bin as f64) * (f_bit_hz as f64) / (n as f64);
    let amp = 10.0_f64.powf(config.amp_db / 20.0);

    const TILE_BASEBAND: usize = 64;
    let tile_expanded = TILE_BASEBAND * multiplier;

    let mut cascade = HalfBandCascade::new(
        FilterPhase::Linear,
        config.apodization,
        cascade_stages,
        TILE_BASEBAND,
    );

    // Warm-up cascade & modulator
    let n_warmup_tiles = 256;
    let mut bb_warmup = [0.0f64; TILE_BASEBAND];
    let mut exp_warmup = vec![0.0f64; tile_expanded];
    let mut dummy_bytes = vec![0u8; tile_expanded / 8];

    for t in 0..n_warmup_tiles {
        let start_sample = (t as i64 - n_warmup_tiles as i64) * (TILE_BASEBAND as i64);
        for (i, s) in bb_warmup.iter_mut().enumerate() {
            let sample_idx = start_sample + (i as i64);
            let t_sec = (sample_idx as f64) / (baseband_fs as f64);
            *s = amp * (2.0 * PI * f_tone_hz * t_sec).sin();
        }
        cascade.process_tile(&bb_warmup, &mut exp_warmup).unwrap();
        modulator
            .process_tile_bytes(&exp_warmup, &mut dummy_bytes)
            .unwrap();
    }

    let mut raw_bytes = vec![0u8; n / 8];
    let total_tiles = n / tile_expanded;
    let mut bb_tile = [0.0f64; TILE_BASEBAND];
    let mut exp_tile = vec![0.0f64; tile_expanded];
    let mut tile_bytes = vec![0u8; tile_expanded / 8];

    let start = Instant::now();
    let mut max_state = vec![0.0f64; modulator.state_slice().len()];

    for t_idx in 0..total_tiles {
        let sample_offset = t_idx * TILE_BASEBAND;
        for (i, s) in bb_tile.iter_mut().enumerate() {
            let sample_idx = sample_offset + i;
            let t_sec = (sample_idx as f64) / (baseband_fs as f64);
            *s = amp * (2.0 * PI * f_tone_hz * t_sec).sin();
        }

        cascade.process_tile(&bb_tile, &mut exp_tile).unwrap();
        modulator
            .process_tile_bytes(&exp_tile, &mut tile_bytes)
            .unwrap();

        let byte_offset = t_idx * (tile_expanded / 8);
        raw_bytes[byte_offset..byte_offset + (tile_expanded / 8)].copy_from_slice(&tile_bytes);

        for (cur_max, &s) in max_state.iter_mut().zip(modulator.state_slice()) {
            *cur_max = cur_max.max(s.abs());
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
  "order": {},
  "apodization": "{}",
  "cascade_stages": {},
  "f_bit_hz": {},
  "k_bin": {},
  "f_tone_hz": {:.6},
  "amp_db": {:.2},
  "elapsed_seconds": {:.6},
  "overload_count": {},
  "max_state": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            config.apodization,
            cascade_stages,
            f_bit_hz,
            k_bin,
            f_tone_hz,
            config.amp_db,
            elapsed.as_secs_f64(),
            modulator.overload_count(),
            format_state_json(&max_state)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_silence(config: &HarnessConfig, modulator: &mut Modulator, _f_bit_hz: u32) {
    let n = config.samples;
    const TILE_BITS: usize = 4096;
    let silence_tile = [0.0f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];
    let mut total_ones: u64 = 0;
    let mut zero_byte_count: u64 = 0;
    let mut full_byte_count: u64 = 0;
    let mut raw_bytes = if config.out_bin.is_some() {
        Some(vec![0u8; n / 8])
    } else {
        None
    };

    let start = Instant::now();
    let num_tiles = n / TILE_BITS;
    let mut max_state = vec![0.0f64; modulator.state_slice().len()];

    for t_idx in 0..num_tiles {
        modulator
            .process_tile_bytes(&silence_tile, &mut tile_bytes)
            .unwrap();
        for &b in &tile_bytes {
            total_ones += b.count_ones() as u64;
            if b == 0x00 {
                zero_byte_count += 1;
            } else if b == 0xFF {
                full_byte_count += 1;
            }
        }

        if let Some(ref mut raw) = raw_bytes {
            let offset = t_idx * (TILE_BITS / 8);
            raw[offset..offset + (TILE_BITS / 8)].copy_from_slice(&tile_bytes);
        }

        for (cur_max, &s) in max_state.iter_mut().zip(modulator.state_slice()) {
            *cur_max = cur_max.max(s.abs());
        }
    }
    let elapsed = start.elapsed();
    let ones_ratio = total_ones as f64 / n as f64;

    if let Some(ref path) = config.out_bin {
        if let Some(ref raw) = raw_bytes {
            let mut f = File::create(path).unwrap();
            f.write_all(raw).unwrap();
        }
    }

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "order": {},
  "samples": {},
  "ones_ratio": {:.6},
  "zero_bytes_count": {},
  "full_bytes_count": {},
  "elapsed_seconds": {:.6},
  "overload_count": {},
  "max_state": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            n,
            ones_ratio,
            zero_byte_count,
            full_byte_count,
            elapsed.as_secs_f64(),
            modulator.overload_count(),
            format_state_json(&max_state)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_dc(config: &HarnessConfig, modulator: &mut Modulator, _f_bit_hz: u32) {
    let n = config.samples;
    const TILE_BITS: usize = 4096;
    let dc_tile = [0.5f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];
    let mut total_ones: u64 = 0;

    let start = Instant::now();
    let num_tiles = n / TILE_BITS;
    let mut max_state = vec![0.0f64; modulator.state_slice().len()];

    for _ in 0..num_tiles {
        modulator
            .process_tile_bytes(&dc_tile, &mut tile_bytes)
            .unwrap();
        for &b in &tile_bytes {
            total_ones += b.count_ones() as u64;
        }

        for (cur_max, &s) in max_state.iter_mut().zip(modulator.state_slice()) {
            *cur_max = cur_max.max(s.abs());
        }
    }
    let elapsed = start.elapsed();
    let ones_ratio = total_ones as f64 / n as f64;

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "order": {},
  "dc_val": 0.5,
  "samples": {},
  "ones_ratio": {:.6},
  "elapsed_seconds": {:.6},
  "overload_count": {},
  "max_state": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            n,
            ones_ratio,
            elapsed.as_secs_f64(),
            modulator.overload_count(),
            format_state_json(&max_state)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_multitone(
    config: &HarnessConfig,
    modulator: &mut Modulator,
    _f_bit_hz: u32,
    cascade_stages: usize,
    baseband_fs: u32,
) {
    let multiplier = 1usize << cascade_stages;
    let n = config.samples;

    const TILE_BASEBAND: usize = 64;
    let tile_expanded = TILE_BASEBAND * multiplier;

    let mut cascade = HalfBandCascade::new(
        FilterPhase::Linear,
        config.apodization,
        cascade_stages,
        TILE_BASEBAND,
    );

    // Multitone: 10 tones from 100 Hz to 18 kHz with staggered phases
    let freqs: [f64; 10] = [
        100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 7500.0, 10000.0, 14000.0, 18000.0,
    ];
    // Peak composite multitone amplitude bounded within config.amp_db (-6.0 dBFS)
    let amp_per_tone = 10.0_f64.powf(config.amp_db / 20.0) / (freqs.len() as f64);

    let total_tiles = n / tile_expanded;
    let mut bb_tile = [0.0f64; TILE_BASEBAND];
    let mut exp_tile = vec![0.0f64; tile_expanded];
    let mut tile_bytes = vec![0u8; tile_expanded / 8];

    let start = Instant::now();
    let mut max_state = vec![0.0f64; modulator.state_slice().len()];

    for t_idx in 0..total_tiles {
        let sample_offset = t_idx * TILE_BASEBAND;
        for (i, s) in bb_tile.iter_mut().enumerate() {
            let sample_idx = sample_offset + i;
            let t_sec = (sample_idx as f64) / (baseband_fs as f64);
            let mut val = 0.0;
            for (f_idx, &f) in freqs.iter().enumerate() {
                val += amp_per_tone * (2.0 * PI * f * t_sec + (f_idx as f64 * 0.785)).sin();
            }
            *s = val;
        }

        cascade.process_tile(&bb_tile, &mut exp_tile).unwrap();
        modulator
            .process_tile_bytes(&exp_tile, &mut tile_bytes)
            .unwrap();

        for (cur_max, &s) in max_state.iter_mut().zip(modulator.state_slice()) {
            *cur_max = cur_max.max(s.abs());
        }
    }
    let elapsed = start.elapsed();

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "order": {},
  "samples": {},
  "elapsed_seconds": {:.6},
  "overload_count": {},
  "max_state": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            n,
            elapsed.as_secs_f64(),
            modulator.overload_count(),
            format_state_json(&max_state)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

/// Stimuli of the input-limit check: DC and sines (Hz), the same set as `design_crfb.MSA_STIMULI_HZ`.
const INPUT_LIMIT_STIMULI_HZ: [f64; 7] = [0.0, 50.0, 1000.0, 5000.0, 10000.0, 15000.0, 20000.0];

/// Runs the production modulator (tile path, configured dither) at its compiled input limit `L` for DC and
/// tones of peak amplitude `L`, `config.samples` bits each, from a reset state.
fn run_input_limit(
    config: &HarnessConfig,
    modulator: &mut Modulator,
    bit_rate: DsdBitRateHz,
    f_bit_hz: u32,
) {
    const TILE_BITS: usize = 4096;
    let limit = audigma_dsp::max_stable_input_dbfs(config.order, bit_rate)
        .unwrap_or_else(|e| panic!("no input limit: {e}"));
    let amp = 10f64.powf(limit / 20.0);
    let mut tile = [0.0f64; TILE_BITS];
    let mut bytes = [0u8; TILE_BITS / 8];
    let mut rows = Vec::new();
    for &f in &INPUT_LIMIT_STIMULI_HZ {
        modulator.reset();
        let before = modulator.overload_count();
        let mut max_state = 0.0f64;
        let mut n: u64 = 0;
        for _ in 0..config.samples / TILE_BITS {
            for s in tile.iter_mut() {
                *s = if f == 0.0 {
                    amp
                } else {
                    amp * (2.0 * std::f64::consts::PI * f * n as f64 / f_bit_hz as f64).sin()
                };
                n += 1;
            }
            modulator.process_tile_bytes(&tile, &mut bytes).unwrap();
            for &x in modulator.state_slice() {
                max_state = max_state.max(x.abs());
            }
        }
        rows.push(format!(
            r#"{{"f_hz":{f},"overloads":{},"max_state":{max_state:.6}}}"#,
            modulator.overload_count() - before
        ));
    }
    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{"mode":"input-limit","rate":"{}","order":{},"limit_dbfs":{limit},"samples":{},"stimuli":[{}]}}"#,
            config.rate,
            config.order,
            config.samples,
            rows.join(",")
        );
        File::create(path)
            .unwrap()
            .write_all(json.as_bytes())
            .unwrap();
    }
}

fn run_overload(config: &HarnessConfig, modulator: &mut Modulator, _f_bit_hz: u32) {
    let n = config.samples;
    const TILE_BITS: usize = 4096;
    let mut raw_bytes = vec![0u8; n / 8];
    let mut pcm_tile = [0.0f64; TILE_BITS];
    let mut tile_bytes = [0u8; TILE_BITS / 8];

    let start = Instant::now();
    let num_tiles = n / TILE_BITS;
    let mut state_after_recovery = vec![0.0f64; modulator.state_slice().len()];

    for t_idx in 0..num_tiles {
        if t_idx == 8 || t_idx == 9 {
            // Intentionally drive integrators well into overload (> 0 dBFS amplitude 5.0)
            pcm_tile.fill(5.0);
        } else {
            // Normal low-level sine
            for (i, p) in pcm_tile.iter_mut().enumerate() {
                *p = 0.1
                    * (2.0 * PI * 1000.0 * ((t_idx * TILE_BITS + i) as f64 / 2_822_400.0)).sin();
            }
        }

        modulator
            .process_tile_bytes(&pcm_tile, &mut tile_bytes)
            .unwrap();
        raw_bytes[t_idx * (TILE_BITS / 8)..(t_idx + 1) * (TILE_BITS / 8)]
            .copy_from_slice(&tile_bytes);

        if t_idx > 12 {
            state_after_recovery = modulator.state_slice().to_vec();
        }
    }
    let elapsed = start.elapsed();

    let recovered_stable = state_after_recovery
        .iter()
        .all(|&s| s.abs() < STABILITY_THRESHOLD);

    // Overload occurred during tiles 8 and 9 (bytes: 8*512 .. 10*512)
    let overload_slice = &raw_bytes[8 * (TILE_BITS / 8)..10 * (TILE_BITS / 8)];
    let idle_0x69_count = overload_slice.iter().filter(|&&b| b == 0x69).count();
    let zero_0x00_count = overload_slice.iter().filter(|&&b| b == 0x00).count();

    if let Some(ref path) = config.out_bin {
        let mut f = File::create(path).unwrap();
        f.write_all(&raw_bytes).unwrap();
    }

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "order": {},
  "samples": {},
  "overload_count": {},
  "recovered_stable": {},
  "overload_idle_0x69_count": {},
  "overload_zero_0x00_count": {},
  "elapsed_seconds": {:.6},
  "state_after_recovery": {}
}}"#,
            config.mode,
            config.rate,
            config.order,
            n,
            modulator.overload_count(),
            recovered_stable,
            idle_0x69_count,
            zero_0x00_count,
            elapsed.as_secs_f64(),
            format_state_json(&state_after_recovery)
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}

fn run_benchmark(
    config: &HarnessConfig,
    modulator: &mut Modulator,
    cascade_stages: usize,
    baseband_fs: u32,
    pin_desc: &str,
) {
    let multiplier = 1usize << cascade_stages;
    let tile_baseband: usize = if multiplier >= 512 { 32 } else { 64 };
    let tile_expanded = tile_baseband * multiplier;

    let mut cascade = HalfBandCascade::new(
        FilterPhase::Linear,
        config.apodization,
        cascade_stages,
        tile_baseband,
    );
    let mut bb_scratch = vec![0.0f64; tile_baseband];
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

    // 20k samples: p99.9 is then the 20th-worst block, not the 2nd-worst.
    const BENCH_BLOCKS: usize = 20_000;
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

    let audio_block_duration_sec = tile_baseband as f64 / baseband_fs as f64;
    let audio_block_duration_micros = audio_block_duration_sec * 1_000_000.0;
    let total_audio_duration_sec = BENCH_BLOCKS as f64 * audio_block_duration_sec;
    let throughput_x_realtime = total_audio_duration_sec / total_elapsed.as_secs_f64();
    let p999_percent_of_budget = ((p999_ns as f64 / 1_000.0) / audio_block_duration_micros) * 100.0;

    if let Some(ref path) = config.out_json {
        let json = format!(
            r#"{{
  "mode": "{}",
  "rate": "{}",
  "order": {},
  "apodization": "{}",
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
  "p999_percent_of_budget": {:.2},
  "placement": "{}"
}}"#,
            config.mode,
            config.rate,
            config.order,
            config.apodization,
            BENCH_BLOCKS,
            tile_baseband,
            audio_block_duration_micros,
            throughput_x_realtime,
            min_ns as f64 / 1000.0,
            avg_ns as f64 / 1000.0,
            p50_ns as f64 / 1000.0,
            p90_ns as f64 / 1000.0,
            p99_ns as f64 / 1000.0,
            p999_ns as f64 / 1000.0,
            max_ns as f64 / 1000.0,
            p999_percent_of_budget,
            pin_desc
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
    }
}
