//! `benchmark-pipeline` mode of `measure_harness`: the production worker input stage
//! (`TileProcessor::process_input_*`) timed per tile for one input/output combination.
//!
//! One channel, executed inline on the calling thread (`WorkerSetup { workers: Some(0) }`): no
//! worker hand-off, one core. Per tile this covers what the live loop runs in its workers:
//! raw-DSD unpack and decimation, the pre-modulator gain, rational SRC, the half-band cascade
//! and, for DSD targets, the modulator. Reading the input and writing the output (DoP packing,
//! sample-format conversion, transport) is outside the timed region.
//!
//! The topology, input stage and tile size are the ones `OfflinePipeline` / `LivePipeline` build;
//! only the stimulus is synthetic (1 kHz-class tone at -6 dBFS, periodic over a short loop).

use std::f64::consts::TAU;
use std::fs::File;
use std::hint::black_box;
use std::io::Write;
use std::time::Instant;

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, FilterPhase, OverloadPolicy, PcmRateHz, SampleFormat,
    TargetRateSpec,
};
use audigma_engine::{
    DsdDecodeConfig, FrameGain, InputDspConfig, InputResampling, InputTileControls,
    PipelineTopology, TargetDsdConfig, TileProcessor, WorkerSetup,
};

/// Modes handled by this module.
pub const MODES: &[&str] = &["benchmark-pipeline"];

/// Baseband tile of the live loop (`live_pipeline.rs`, `TILE_BASEBAND_FRAMES`).
pub const TILE_BASEBAND_FRAMES: usize = 256;
/// Distinct input tiles in the stimulus loop; the tone is periodic over the whole loop.
const LOOP_TILES: usize = 64;
const WARMUP_TILES: usize = 200;
const TONE_AMPLITUDE: f64 = 0.5;
const TONE_HZ: f64 = 1000.0;

struct Args {
    input: String,
    output: String,
    order: u8,
    phase: FilterPhase,
    apodization: Apodization,
    blocks: usize,
    pin: bool,
    out_json: Option<String>,
}

fn parse(args: &[String]) -> Args {
    let mut a = Args {
        input: "pcm44".to_string(),
        output: "dsd256".to_string(),
        order: 5,
        phase: FilterPhase::Linear,
        apodization: Apodization::Off,
        blocks: crate::DEFAULT_BENCH_BLOCKS,
        pin: false,
        out_json: None,
    };
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => i += 1,
            "--input" => {
                i += 1;
                a.input = args[i].clone();
            }
            "--output" => {
                i += 1;
                a.output = args[i].clone();
            }
            "--order" => {
                i += 1;
                a.order = args[i].parse().expect("Invalid order");
            }
            "--phase" => {
                i += 1;
                a.phase = match args[i].as_str() {
                    "linear" => FilterPhase::Linear,
                    "minimum" => FilterPhase::Minimum,
                    other => panic!("Unknown phase: {other}"),
                };
            }
            "--apodization" => {
                i += 1;
                a.apodization = Apodization::from_name(&args[i])
                    .unwrap_or_else(|| panic!("Unknown apodization: {}", args[i]));
            }
            "--blocks" => {
                i += 1;
                a.blocks = args[i].parse().expect("Invalid blocks");
                assert!(a.blocks > 0, "--blocks must be positive");
            }
            "--pin" => a.pin = true,
            "--out-json" => {
                i += 1;
                a.out_json = Some(args[i].clone());
            }
            other => panic!("Unknown argument: {other}"),
        }
        i += 1;
    }
    a
}

pub fn dispatch(args: &[String]) -> bool {
    let is_mine = args
        .windows(2)
        .any(|w| w[0] == "--mode" && MODES.contains(&w[1].as_str()));
    if !is_mine {
        return false;
    }
    run(&parse(args));
    true
}

/// Looped, tile-aligned stimulus in the input's native form.
enum Stimulus {
    Pcm(Vec<f64>),
    /// MSB-first packed DSD bytes.
    Dsd { bytes: Vec<u8>, tile_bytes: usize },
}

fn pcm_stimulus(rate: PcmRateHz) -> Vec<f64> {
    let frames = LOOP_TILES * TILE_BASEBAND_FRAMES;
    // Whole number of cycles over the loop, so the wrap is seamless.
    let cycles = (TONE_HZ * frames as f64 / rate.get() as f64).round();
    (0..frames)
        .map(|n| TONE_AMPLITUDE * (TAU * cycles * n as f64 / frames as f64).sin())
        .collect()
}

/// First-order delta-sigma encoding of the tone. Only the decode cost matters here, not the
/// encoder's noise shaping.
fn dsd_stimulus(bit_rate: DsdBitRateHz, tile_bytes: usize) -> Vec<u8> {
    let bits = LOOP_TILES * tile_bytes * 8;
    let cycles = (TONE_HZ * bits as f64 / bit_rate.get() as f64).round();
    let mut bytes = vec![0u8; bits / 8];
    let mut integrator = 0.0f64;
    for n in 0..bits {
        let x = TONE_AMPLITUDE * (TAU * cycles * n as f64 / bits as f64).sin();
        let y = if integrator >= 0.0 { 1.0 } else { -1.0 };
        integrator += x - y;
        if y > 0.0 {
            bytes[n / 8] |= 0x80 >> (n % 8);
        }
    }
    bytes
}

fn run(a: &Args) {
    let pin_desc = if a.pin {
        crate::pin_like_runtime()
    } else {
        "unpinned (not requested)".to_string()
    };

    // DSD input is decoded to the intermediate rate `AudioStreamInfo::new_dsd` advertises.
    let (input_rate, dsd_input) = match a.input.as_str() {
        "pcm44" => (PcmRateHz::PCM_44100, None),
        "pcm96" => (PcmRateHz::PCM_96000, None),
        "dsd64" => (PcmRateHz::PCM_176400, Some(DsdBitRateHz::DSD64)),
        other => panic!("Unknown input: {other} (pcm44|pcm96|dsd64)"),
    };
    let target = match a.output.as_str() {
        "pcm384" => TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_384000,
            sample_format: SampleFormat::S32Le,
        },
        "dsd256" => TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256,
        },
        "dsd512" => TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD512,
        },
        other => panic!("Unknown output: {other} (pcm384|dsd256|dsd512)"),
    };

    let topology = PipelineTopology::resolve(input_rate, target)
        .unwrap_or_else(|e| panic!("Unsupported {} -> {}: {e}", a.input, a.output));
    // Same derivation as `OfflinePipeline::prepare_with_dsd_input`.
    let resampling = if topology.requires_rational_src {
        InputResampling::Rational {
            input_rate,
            output_rate: topology.intermediate_rate,
        }
    } else {
        InputResampling::None
    };
    let dsd = dsd_input.map(|bit_rate| DsdDecodeConfig {
        decimation_stages: bit_rate
            .power_of_two_decimation_stages_to(input_rate)
            .expect("DSD rate decimates to the intermediate rate") as usize,
        gain_db: 0.0,
    });
    let is_dsd_output = matches!(target, TargetRateSpec::Dsd { .. });
    let target_dsd = match target {
        TargetRateSpec::Dsd { bit_rate } => Some(TargetDsdConfig {
            bit_rate,
            modulator_order: a.order,
            dither: DitherType::Tpdf,
            // A silently muted modulator would be timed as a fast one: fail instead.
            overload_policy: OverloadPolicy::Error,
            seed_base: 12345,
        }),
        TargetRateSpec::Pcm { .. } => None,
    };

    let mut processor = TileProcessor::new_with_input(
        1,
        a.phase,
        a.apodization,
        topology,
        TILE_BASEBAND_FRAMES,
        target_dsd,
        false,
        InputDspConfig {
            pcm_decimation_stages: topology.decimation_stages,
            dsd,
            resampling,
        },
        Some(WorkerSetup {
            workers: Some(0),
            ..Default::default()
        }),
    )
    .unwrap_or_else(|e| panic!("Failed to build the processor: {e}"));

    assert_eq!(processor.input_frame_capacity(), TILE_BASEBAND_FRAMES);
    let stimulus = match dsd_input {
        None => Stimulus::Pcm(pcm_stimulus(input_rate)),
        Some(bit_rate) => {
            let tile_bytes = processor.input_dsd_byte_capacity();
            Stimulus::Dsd {
                bytes: dsd_stimulus(bit_rate, tile_bytes),
                tile_bytes,
            }
        }
    };
    let gains = vec![FrameGain::Preserve; TILE_BASEBAND_FRAMES];

    let mut process_tile = |tile: usize| {
        let tile = tile % LOOP_TILES;
        match &stimulus {
            Stimulus::Pcm(samples) => {
                let block = &samples[tile * TILE_BASEBAND_FRAMES..][..TILE_BASEBAND_FRAMES];
                let baseband = processor.pcm_baseband_frames(block.len()).unwrap();
                let controls = InputTileControls {
                    frame_gains: &gains[..baseband],
                    linear_gain: 1.0,
                    drift_ppm: 0.0,
                };
                processor
                    .process_input_pcm(&[black_box(block)], controls)
                    .unwrap();
            }
            Stimulus::Dsd { bytes, tile_bytes } => {
                let block = &bytes[tile * tile_bytes..][..*tile_bytes];
                let baseband = processor.dsd_baseband_frames(block.len()).unwrap();
                let controls = InputTileControls {
                    frame_gains: &gains[..baseband],
                    linear_gain: 1.0,
                    drift_ppm: 0.0,
                };
                processor
                    .process_input_dsd(&[black_box(block)], controls)
                    .unwrap();
            }
        }
        if is_dsd_output {
            black_box(processor.channel_dsd(0));
        } else {
            black_box(processor.channel_pcm(0));
        }
    };

    for tile in 0..WARMUP_TILES {
        process_tile(tile);
    }

    let mut block_times_ns = Vec::with_capacity(a.blocks);
    let total_start = Instant::now();
    for tile in 0..a.blocks {
        let block_start = Instant::now();
        process_tile(tile);
        block_times_ns.push(block_start.elapsed().as_nanos() as u64);
    }
    let total_elapsed = total_start.elapsed();

    block_times_ns.sort_unstable();
    let blocks = a.blocks;
    let min_ns = block_times_ns[0];
    let max_ns = block_times_ns[blocks - 1];
    let p50_ns = block_times_ns[blocks * 50 / 100];
    let p90_ns = block_times_ns[blocks * 90 / 100];
    let p99_ns = block_times_ns[blocks * 99 / 100];
    let p999_ns = block_times_ns[blocks * 999 / 1000];
    let avg_ns = block_times_ns.iter().sum::<u64>() / blocks as u64;

    // Budget: the input time one tile spans (the live loop's per-tile deadline).
    let audio_block_duration_sec = TILE_BASEBAND_FRAMES as f64 / input_rate.get() as f64;
    let audio_block_duration_micros = audio_block_duration_sec * 1_000_000.0;
    let throughput_x_realtime =
        blocks as f64 * audio_block_duration_sec / total_elapsed.as_secs_f64();
    let percent_of_budget = |ns: u64| ns as f64 / 1_000.0 / audio_block_duration_micros * 100.0;

    if let Some(path) = &a.out_json {
        let json = format!(
            r#"{{
  "mode": "benchmark-pipeline",
  "input": "{}",
  "output": "{}",
  "order": {},
  "phase": "{}",
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
  "p99_percent_of_budget": {:.2},
  "p999_percent_of_budget": {:.2},
  "placement": "{}"
}}"#,
            a.input,
            a.output,
            a.order,
            if a.phase == FilterPhase::Minimum {
                "minimum"
            } else {
                "linear"
            },
            a.apodization,
            blocks,
            TILE_BASEBAND_FRAMES,
            audio_block_duration_micros,
            throughput_x_realtime,
            min_ns as f64 / 1000.0,
            avg_ns as f64 / 1000.0,
            p50_ns as f64 / 1000.0,
            p90_ns as f64 / 1000.0,
            p99_ns as f64 / 1000.0,
            p999_ns as f64 / 1000.0,
            max_ns as f64 / 1000.0,
            percent_of_budget(p99_ns),
            percent_of_budget(p999_ns),
            pin_desc,
        );
        let mut f = File::create(path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }
}
