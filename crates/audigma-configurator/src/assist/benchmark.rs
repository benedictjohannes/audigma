//! Compute benchmark (`benchmark-validate`, `benchmark-discover`), extracted from
//! `audigma benchmark`.
//!
//! [`run`] measures one configuration synchronously and reports progress through a callback;
//! [`start`] runs it on a thread and hands back a [`BenchmarkHandle`] with an event channel and
//! a cancel flag (what the TUI and the web backend drive). Discovery is a loop of [`run`] calls
//! over rate tiers, owned by the caller.
//!
//! The numbers are **estimates**. This is the offline DSP gate: the cascade and modulator run
//! on the worker pool with no I/O. Live playback adds dispatch cost, so passing here is
//! necessary but not sufficient for `audigma run`.
//!
//! Cancel is checked between stages and every [`PROGRESS_EVERY`] blocks inside the two timing
//! loops. The end-to-end stage runs the engine's offline pipeline, which has no cancel hook; it
//! is bounded (the synthetic signal is [`BenchmarkParams::seconds`] long, at several times real
//! time), so a cancel during it takes effect when it ends.

use audigma_dsp::config::{DitherType, OverloadPolicy, TargetRateSpec};
use audigma_dsp::fir::HalfBandCascade;
use audigma_dsp::modulator::Modulator;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::traits::{AudioInput, DecoderInput};
use audigma_engine::{ChannelWorkerPool, MockAudioInput, OfflinePipeline, TargetDsdConfig};
use audigma_format::AudioStreamInfo;
use audigma_format::decoder::AudioDecoder;
use audigma_settings::Settings;
use schemars::JsonSchema;
use serde::Serialize;
use std::f64::consts::PI;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::Instant;

/// Frames per DSP block (the worker pool's tile).
pub const BLOCK_FRAMES: usize = 64;
/// Progress is reported every this many timed blocks.
pub const PROGRESS_EVERY: usize = 100;
/// Throughput gate (`Design.md §8`): at least this many times real time.
pub const GATE_THROUGHPUT_X: f64 = 2.0;
/// Latency gate (`Design.md §8`): p99.9 worker time at most this share of the block duration.
pub const GATE_P999_PERCENT: f64 = 70.0;

/// What to measure.
#[derive(Debug, Clone)]
pub struct BenchmarkRequest {
    /// The configuration, already validated by the caller.
    pub settings: Settings,
    /// Optional input file; without it a deterministic synthetic stereo signal is used.
    pub input: Option<PathBuf>,
    pub params: BenchmarkParams,
}

impl BenchmarkRequest {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            input: None,
            params: BenchmarkParams::default(),
        }
    }
}

/// Sizes of the measurement. The defaults are what `audigma benchmark` has always used; tests
/// shrink them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkParams {
    /// Length of the synthetic end-to-end signal.
    pub seconds: usize,
    /// Timed blocks per DSP stage.
    pub blocks: usize,
}

impl Default for BenchmarkParams {
    fn default() -> Self {
        Self {
            seconds: 10,
            blocks: 2000,
        }
    }
}

/// The three measurement stages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    EndToEnd,
    Sequential,
    Workers,
}

/// Progress and the terminal outcome of a run.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchmarkEvent {
    /// `fraction` is 0.0 to 1.0 within `stage`.
    Progress {
        stage: Stage,
        fraction: f64,
    },
    Finished(Box<BenchmarkResult>),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BenchmarkError {
    #[error("benchmark cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

fn fail<E: Display>(e: E) -> BenchmarkError {
    BenchmarkError::Failed(e.to_string())
}

/// Realtime tier by worker-pool throughput.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// At least [`GATE_THROUGHPUT_X`] times real time.
    Optimal,
    /// Faster than real time but under the gate.
    Capable,
    /// Slower than real time.
    Constrained,
}

/// Pass/fail against the two gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Gates {
    pub throughput: bool,
    pub p999: bool,
}

impl Gates {
    pub fn evaluate(worker_throughput_x: f64, p999_budget_percent: f64) -> Self {
        Self {
            throughput: worker_throughput_x >= GATE_THROUGHPUT_X,
            p999: p999_budget_percent <= GATE_P999_PERCENT,
        }
    }

    pub fn passed(self) -> bool {
        self.throughput && self.p999
    }
}

/// One benchmark's measurements. An estimate; see the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct BenchmarkResult {
    pub filter: String,
    pub input_rate_hz: u32,
    /// `Pcm` or `Dsd`, as the engine reports it.
    pub target_format: String,
    pub worker_throughput_x: f64,
    pub sequential_throughput_x: f64,
    pub speedup_factor: f64,
    pub parallel_efficiency_percent: f64,
    pub tier: Tier,
    pub end_to_end_throughput_x: f64,
    pub audio_duration_sec: f64,
    pub block_frames: usize,
    pub block_budget_micros: f64,
    pub avg_block_micros: f64,
    pub p50_block_micros: f64,
    pub p99_block_micros: f64,
    pub p999_block_micros: f64,
    pub p999_budget_percent: f64,
    pub gates: Gates,
}

impl BenchmarkResult {
    pub fn passed(&self) -> bool {
        self.gates.passed()
    }
}

/// `sorted[n * permille / 1000]`, clamped to the last element. `sorted` must not be empty.
fn percentile(sorted: &[u64], permille: usize) -> u64 {
    sorted[(sorted.len() * permille / 1000).min(sorted.len() - 1)]
}

fn tier_for(worker_throughput_x: f64) -> Tier {
    if worker_throughput_x >= GATE_THROUGHPUT_X {
        Tier::Optimal
    } else if worker_throughput_x >= 1.0 {
        Tier::Capable
    } else {
        Tier::Constrained
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), BenchmarkError> {
    if cancel.load(Ordering::Relaxed) {
        Err(BenchmarkError::Cancelled)
    } else {
        Ok(())
    }
}

fn new_modulator(
    order: u8,
    bit_rate: audigma_dsp::rates::DsdBitRateHz,
    seed: u64,
) -> Result<Modulator, BenchmarkError> {
    let mut m = Modulator::new(
        order,
        bit_rate,
        DitherType::Tpdf,
        OverloadPolicy::MuteRecover,
    )
    .map_err(fail)?;
    m.set_dither_seed(seed);
    Ok(m)
}

/// Measures `req` on the calling thread. `on_event` receives [`BenchmarkEvent::Progress`];
/// the terminal outcome is the return value (use [`start`] to get it as an event too).
///
/// Resolves the `[runtime]` placement for the run (`offline`: no real-time requests) and applies
/// the housekeeping affinity to the calling thread, as the offline commands do.
pub fn run(
    req: &BenchmarkRequest,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(BenchmarkEvent),
) -> Result<BenchmarkResult, BenchmarkError> {
    let settings = &req.settings;
    let BenchmarkParams { seconds, blocks } = req.params;
    if seconds == 0 || blocks == 0 {
        return Err(BenchmarkError::Failed(
            "benchmark seconds and blocks must be at least 1".into(),
        ));
    }
    const SAMPLE_RATE_HZ: u32 = 44_100;
    let total_frames = seconds * SAMPLE_RATE_HZ as usize;

    check_cancel(cancel)?;
    let (mut input, input_rate): (Box<dyn AudioInput>, PcmRateHz) = if let Some(path) = &req.input {
        let file = std::fs::File::open(path).map_err(fail)?;
        let decoder =
            AudioDecoder::try_new(file, path.extension().and_then(|e| e.to_str())).map_err(fail)?;
        let rate = decoder.info().sample_rate;
        (Box::new(DecoderInput::new(decoder)), rate)
    } else {
        // Deterministic synthetic stereo signal: 1 kHz coherent sine at -6 dBFS.
        let amp = 10.0f64.powf(-6.0 / 20.0);
        let signal: Vec<f64> = (0..total_frames)
            .map(|i| amp * (2.0 * PI * 1000.0 * (i as f64 / SAMPLE_RATE_HZ as f64)).sin())
            .collect();
        let info =
            AudioStreamInfo::new_pcm(PcmRateHz::PCM_44100, 2, Some(total_frames as u64), Some(24));
        (
            Box::new(MockAudioInput::new(info, signal.clone(), signal, true)),
            PcmRateHz::PCM_44100,
        )
    };

    // Stage 1: end to end through the offline pipeline into memory.
    let pipeline = {
        let placement = audigma_io::runtime::resolve(settings, 2, true).map_err(fail)?;
        audigma_io::runtime::apply_housekeeping(&placement);
        OfflinePipeline::prepare_with_workers(
            settings.clone(),
            input_rate,
            Some(audigma_io::runtime::worker_setup(&placement)),
        )
        .map_err(fail)?
    };
    let plan = pipeline.plan().clone();

    on_event(BenchmarkEvent::Progress {
        stage: Stage::EndToEnd,
        fraction: 0.0,
    });
    let start_bench = Instant::now();
    let report = pipeline
        .run(&mut *input, Box::new(std::io::Cursor::new(Vec::new())))
        .map_err(fail)?;
    let e2e_elapsed = start_bench.elapsed();
    on_event(BenchmarkEvent::Progress {
        stage: Stage::EndToEnd,
        fraction: 1.0,
    });
    check_cancel(cancel)?;

    let audio_duration_sec = report.total_input_frames as f64 / input_rate.get() as f64;
    let e2e_throughput_x = audio_duration_sec / e2e_elapsed.as_secs_f64();

    // DSP-only timing (`Design.md §8`): `blocks` blocks of 64 baseband frames through the LTI
    // cascade and the modulator.
    let target_spec = plan.stream_spec.target;
    let (stages, dsd_rate) = match target_spec {
        TargetRateSpec::Dsd { bit_rate } => {
            let mult = bit_rate.get() / input_rate.get();
            ((mult as f64).log2().round() as usize, Some(bit_rate))
        }
        TargetRateSpec::Pcm { rate, .. } => {
            let mult = rate.get() / input_rate.get();
            let stages = if mult >= 1 {
                (mult as f64).log2().round() as usize
            } else {
                0
            };
            (stages, None)
        }
    };
    let is_dsd = dsd_rate.is_some();
    let tile_expanded = BLOCK_FRAMES * (1usize << stages);
    let amp = 10.0f64.powf(-6.0 / 20.0);
    let mut bb_l = [0.0f64; BLOCK_FRAMES];
    let mut bb_r = [0.0f64; BLOCK_FRAMES];
    for i in 0..BLOCK_FRAMES {
        let t = i as f64 / input_rate.get() as f64;
        bb_l[i] = amp * (2.0 * PI * 1000.0 * t).sin();
        bb_r[i] = amp * (2.0 * PI * 2000.0 * t).sin();
    }

    // Stage 2: single-threaded sequential baseline (L and R on one thread).
    let new_cascade = || {
        HalfBandCascade::new(
            plan.dsp_config.phase,
            plan.dsp_config.apodization,
            stages,
            BLOCK_FRAMES,
        )
    };
    let mut seq_cascade_l = new_cascade();
    let mut seq_cascade_r = new_cascade();
    let mut exp_l = vec![0.0f64; tile_expanded];
    let mut exp_r = vec![0.0f64; tile_expanded];
    let mut dsd_bytes_l = vec![0u8; tile_expanded / 8];
    let mut dsd_bytes_r = vec![0u8; tile_expanded / 8];
    let mut seq_mods = match dsd_rate {
        Some(bit_rate) => Some((
            new_modulator(plan.dsp_config.modulator_order, bit_rate, 12345)?,
            new_modulator(plan.dsp_config.modulator_order, bit_rate, 12346)?,
        )),
        None => None,
    };
    let mut seq_block = || -> Result<(), BenchmarkError> {
        seq_cascade_l
            .process_tile(&bb_l, &mut exp_l)
            .map_err(fail)?;
        seq_cascade_r
            .process_tile(&bb_r, &mut exp_r)
            .map_err(fail)?;
        if let Some((ml, mr)) = &mut seq_mods {
            ml.process_tile_bytes(&exp_l, &mut dsd_bytes_l)
                .map_err(fail)?;
            mr.process_tile_bytes(&exp_r, &mut dsd_bytes_r)
                .map_err(fail)?;
        }
        Ok(())
    };
    for _ in 0..50 {
        seq_block()?;
    }
    let seq_start = Instant::now();
    for i in 0..blocks {
        seq_block()?;
        if i % PROGRESS_EVERY == 0 {
            check_cancel(cancel)?;
            on_event(BenchmarkEvent::Progress {
                stage: Stage::Sequential,
                fraction: i as f64 / blocks as f64,
            });
        }
    }
    let seq_total_elapsed = seq_start.elapsed();
    on_event(BenchmarkEvent::Progress {
        stage: Stage::Sequential,
        fraction: 1.0,
    });

    // Stage 3: the worker pool, one dedicated thread per channel.
    let target_dsd = dsd_rate.map(|bit_rate| TargetDsdConfig {
        bit_rate,
        modulator_order: plan.dsp_config.modulator_order,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        seed_base: 12345,
    });
    let mut worker_pool = ChannelWorkerPool::new(
        2,
        plan.dsp_config.phase,
        plan.dsp_config.apodization,
        stages,
        BLOCK_FRAMES,
        target_dsd,
        None,
    )
    .map_err(fail)?;
    let mut pool_block = || -> Result<(), BenchmarkError> {
        if is_dsd {
            worker_pool
                .process_tile_dsd(&[&bb_l, &bb_r])
                .map_err(fail)?;
        } else {
            worker_pool
                .process_tile_pcm(&[&bb_l, &bb_r])
                .map_err(fail)?;
        }
        Ok(())
    };
    for _ in 0..50 {
        pool_block()?;
    }
    let mut block_times_ns = Vec::with_capacity(blocks);
    let worker_start = Instant::now();
    for i in 0..blocks {
        let b_start = Instant::now();
        pool_block()?;
        block_times_ns.push(b_start.elapsed().as_nanos() as u64);
        if i % PROGRESS_EVERY == 0 {
            check_cancel(cancel)?;
            on_event(BenchmarkEvent::Progress {
                stage: Stage::Workers,
                fraction: i as f64 / blocks as f64,
            });
        }
    }
    let worker_total_elapsed = worker_start.elapsed();
    on_event(BenchmarkEvent::Progress {
        stage: Stage::Workers,
        fraction: 1.0,
    });

    block_times_ns.sort_unstable();
    let dsp_audio_duration_sec = blocks as f64 * (BLOCK_FRAMES as f64 / input_rate.get() as f64);
    let worker_throughput_x = dsp_audio_duration_sec / worker_total_elapsed.as_secs_f64();
    let seq_throughput_x = dsp_audio_duration_sec / seq_total_elapsed.as_secs_f64();
    let speedup_factor = seq_total_elapsed.as_secs_f64() / worker_total_elapsed.as_secs_f64();
    let block_budget_micros = (BLOCK_FRAMES as f64 / input_rate.get() as f64) * 1_000_000.0;
    let p999_micros = percentile(&block_times_ns, 999) as f64 / 1000.0;
    let p999_budget_percent = p999_micros / block_budget_micros * 100.0;
    let avg_ns = block_times_ns.iter().sum::<u64>() / blocks as u64;

    Ok(BenchmarkResult {
        filter: plan.dsp_config.filter_description(),
        input_rate_hz: input_rate.get(),
        target_format: format!("{:?}", report.target_format),
        worker_throughput_x,
        sequential_throughput_x: seq_throughput_x,
        speedup_factor,
        parallel_efficiency_percent: speedup_factor / 2.0 * 100.0,
        tier: tier_for(worker_throughput_x),
        end_to_end_throughput_x: e2e_throughput_x,
        audio_duration_sec,
        block_frames: BLOCK_FRAMES,
        block_budget_micros,
        avg_block_micros: avg_ns as f64 / 1000.0,
        p50_block_micros: percentile(&block_times_ns, 500) as f64 / 1000.0,
        p99_block_micros: percentile(&block_times_ns, 990) as f64 / 1000.0,
        p999_block_micros: p999_micros,
        p999_budget_percent,
        gates: Gates::evaluate(worker_throughput_x, p999_budget_percent),
    })
}

/// A benchmark running on its own thread.
///
/// `events` yields [`BenchmarkEvent::Progress`] and then exactly one terminal event
/// (`Finished`, `Failed` or `Cancelled`), after which the channel closes.
pub struct BenchmarkHandle {
    pub events: Receiver<BenchmarkEvent>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl BenchmarkHandle {
    pub fn new(
        events: Receiver<BenchmarkEvent>,
        cancel: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    ) -> Self {
        Self {
            events,
            cancel,
            join,
        }
    }

    /// Asks the run to stop at its next check (see the module docs for the granularity).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Blocks until the terminal event and returns it, dropping progress events.
    pub fn wait(mut self) -> BenchmarkEvent {
        let mut last = BenchmarkEvent::Failed("benchmark thread ended without a result".into());
        while let Ok(ev) = self.events.recv() {
            if !matches!(ev, BenchmarkEvent::Progress { .. }) {
                last = ev;
            }
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        last
    }
}

/// Starts `req` on a new thread.
pub fn start(req: BenchmarkRequest) -> BenchmarkHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let join = std::thread::Builder::new()
        .name("audigma-benchmark".into())
        .spawn(move || {
            let progress_tx = tx.clone();
            let outcome = run(&req, &flag, &mut |ev| {
                let _ = progress_tx.send(ev);
            });
            let _ = tx.send(match outcome {
                Ok(r) => BenchmarkEvent::Finished(Box::new(r)),
                Err(BenchmarkError::Cancelled) => BenchmarkEvent::Cancelled,
                Err(BenchmarkError::Failed(m)) => BenchmarkEvent::Failed(m),
            });
        })
        .expect("spawn benchmark thread");
    BenchmarkHandle {
        events: rx,
        cancel,
        join: Some(join),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small(settings: &str) -> BenchmarkRequest {
        let mut req = BenchmarkRequest::new(
            Settings::from_toml_str(&format!("schema_version = 1\n{settings}")).unwrap(),
        );
        req.params = BenchmarkParams {
            seconds: 1,
            blocks: 250,
        };
        req
    }

    const PCM: &str = "[target]\nformat = \"pcm\"\npcm_rate_hz = 88200\n";

    #[test]
    fn percentile_is_clamped_and_ordered() {
        let v: Vec<u64> = (1..=10).collect();
        assert_eq!(percentile(&v, 500), 6);
        assert_eq!(percentile(&v, 999), 10);
        assert_eq!(percentile(&[7], 999), 7);
    }

    #[test]
    fn gates_and_tiers_follow_the_thresholds() {
        assert!(Gates::evaluate(2.0, 70.0).passed());
        assert!(!Gates::evaluate(1.99, 10.0).passed());
        assert!(!Gates::evaluate(5.0, 70.1).passed());
        assert_eq!(tier_for(2.0), Tier::Optimal);
        assert_eq!(tier_for(1.5), Tier::Capable);
        assert_eq!(tier_for(0.9), Tier::Constrained);
    }

    #[test]
    fn zero_sized_params_are_rejected() {
        let mut req = small(PCM);
        req.params.blocks = 0;
        let err = run(&req, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(matches!(err, BenchmarkError::Failed(_)));
    }

    #[test]
    fn pre_cancelled_run_returns_cancelled_without_events() {
        let cancel = AtomicBool::new(true);
        let mut events = 0;
        let err = run(&small(PCM), &cancel, &mut |_| events += 1).unwrap_err();
        assert_eq!(err, BenchmarkError::Cancelled);
        assert_eq!(events, 0);
    }

    #[test]
    fn run_reports_ordered_progress_and_consistent_numbers() {
        let mut stages = Vec::new();
        let r = run(&small(PCM), &AtomicBool::new(false), &mut |ev| {
            if let BenchmarkEvent::Progress { stage, fraction } = ev {
                assert!((0.0..=1.0).contains(&fraction));
                if stages.last() != Some(&stage) {
                    stages.push(stage);
                }
            }
        })
        .unwrap();
        assert_eq!(stages, [Stage::EndToEnd, Stage::Sequential, Stage::Workers]);
        assert_eq!(r.target_format, "Pcm");
        assert_eq!(r.input_rate_hz, 44_100);
        assert_eq!(r.block_frames, BLOCK_FRAMES);
        assert!(r.worker_throughput_x > 0.0 && r.sequential_throughput_x > 0.0);
        assert!(r.p50_block_micros <= r.p99_block_micros);
        assert!(r.p99_block_micros <= r.p999_block_micros);
        assert!((r.audio_duration_sec - 1.0).abs() < 1e-9);
        assert_eq!(
            r.gates,
            Gates::evaluate(r.worker_throughput_x, r.p999_budget_percent)
        );
        assert_eq!(r.tier, tier_for(r.worker_throughput_x));
    }

    #[test]
    fn handle_delivers_one_terminal_event() {
        let handle = start(small(PCM));
        match handle.wait() {
            BenchmarkEvent::Finished(r) => assert_eq!(r.target_format, "Pcm"),
            other => panic!("expected Finished, got {other:?}"),
        }
    }

    #[test]
    fn handle_cancel_stops_the_run() {
        let mut req = small(PCM);
        req.params.blocks = 5_000_000; // would take minutes; cancel must cut it short
        req.params.seconds = 1;
        let handle = start(req);
        loop {
            match handle.events.recv().expect("terminal event before close") {
                BenchmarkEvent::Progress {
                    stage: Stage::Sequential,
                    ..
                } => break,
                BenchmarkEvent::Progress { .. } => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        handle.cancel();
        assert_eq!(handle.wait(), BenchmarkEvent::Cancelled);
    }
}
