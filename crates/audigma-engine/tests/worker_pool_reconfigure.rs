//! A persistent `ChannelWorkerPool` reconfigured for a new rate must keep its
//! worker threads and produce output identical to a freshly built pool.

use audigma_dsp::config::{Apodization, DitherType, FilterPhase, FilterPreset, OverloadPolicy};
use audigma_dsp::rates::DsdBitRateHz;
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const CHANNELS: usize = 2;
const FRAMES: usize = 256;

#[derive(Clone)]
struct Cfg {
    stages: usize,
    dsd: Option<DsdBitRateHz>,
    preset: FilterPreset,
}

fn dsd_cfg(rate: DsdBitRateHz) -> TargetDsdConfig {
    TargetDsdConfig {
        bit_rate: rate,
        modulator_order: 5,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        seed_base: 7,
    }
}

fn build(cfg: &Cfg, hook: Option<audigma_engine::ThreadInitHook>) -> ChannelWorkerPool {
    ChannelWorkerPool::new(
        CHANNELS,
        cfg.preset,
        FilterPhase::Linear,
        Apodization::Off,
        cfg.stages,
        FRAMES,
        cfg.dsd.map(dsd_cfg),
        hook.map(|hook| audigma_engine::WorkerSetup {
            placements: Vec::new(),
            hook: Some(hook),
            ..Default::default()
        }),
    )
    .unwrap()
}

fn reconfigure(pool: &mut ChannelWorkerPool, cfg: &Cfg) {
    pool.reconfigure(
        cfg.preset,
        FilterPhase::Linear,
        Apodization::Off,
        cfg.stages,
        FRAMES,
        cfg.dsd.map(dsd_cfg),
        None,
    )
    .unwrap();
}

fn signal(block: usize, ch: usize) -> Vec<f64> {
    (0..FRAMES)
        .map(|i| {
            let t = (block * FRAMES + i) as f64;
            0.4 * (t * 0.031 * (ch as f64 + 1.0)).sin()
        })
        .collect()
}

/// Runs `blocks` tiles and returns every channel's output bytes (DSD) or PCM bits.
fn run(pool: &mut ChannelWorkerPool, dsd: bool, blocks: usize) -> Vec<Vec<u64>> {
    let mut out = vec![Vec::new(); CHANNELS];
    for b in 0..blocks {
        let data: Vec<Vec<f64>> = (0..CHANNELS).map(|c| signal(b, c)).collect();
        let slices: Vec<&[f64]> = data.iter().map(|v| v.as_slice()).collect();
        if dsd {
            pool.process_tile_dsd(&slices).unwrap();
        } else {
            pool.process_tile_pcm(&slices).unwrap();
        }
        for (c, o) in out.iter_mut().enumerate() {
            if dsd {
                o.extend(pool.channel_dsd(c).iter().map(|&x| x as u64));
            } else {
                o.extend(pool.channel_pcm(c).iter().map(|x| x.to_bits()));
            }
        }
    }
    out
}

#[test]
fn reconfigure_keeps_threads_and_matches_fresh_pool() {
    let started = Arc::new(AtomicUsize::new(0));
    let hook: audigma_engine::ThreadInitHook = {
        let started = Arc::clone(&started);
        Arc::new(move |_, _| {
            started.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };

    let first = Cfg {
        stages: 6,
        dsd: Some(DsdBitRateHz::DSD64),
        preset: FilterPreset::Balanced,
    };
    let sequence = [
        Cfg {
            stages: 8,
            dsd: Some(DsdBitRateHz::DSD256),
            preset: FilterPreset::Reference,
        },
        Cfg {
            stages: 3,
            dsd: None,
            preset: FilterPreset::Balanced,
        },
        Cfg {
            stages: 9,
            dsd: Some(DsdBitRateHz::DSD512),
            preset: FilterPreset::Balanced,
        },
        // Back to a smaller shape: buffers shrink again.
        first.clone(),
    ];

    let mut pool = build(&first, Some(hook));
    // Dirty the state so a missed reset would show up.
    run(&mut pool, true, 5);

    for cfg in &sequence {
        reconfigure(&mut pool, cfg);
        assert_eq!(pool.rate_multiplier(), 1 << cfg.stages);
        let got = run(&mut pool, cfg.dsd.is_some(), 6);
        let mut fresh = build(cfg, None);
        let want = run(&mut fresh, cfg.dsd.is_some(), 6);
        assert_eq!(got, want, "reconfigured pool differs from fresh pool");
        assert_eq!(pool.total_overload_count(), fresh.total_overload_count());
    }

    // Hook ran once per worker thread for the whole pool lifetime.
    assert_eq!(started.load(Ordering::SeqCst), CHANNELS);
}

#[test]
fn each_worker_receives_its_own_placement_and_non_spinning_pool_still_works() {
    use audigma_engine::{ThreadInitHook, ThreadPlacement, WorkerSetup};
    use std::sync::Mutex;

    let seen: Arc<Mutex<Vec<(usize, ThreadPlacement)>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let hook: ThreadInitHook = Arc::new(move |ch, p| {
        sink.lock().unwrap().push((ch, p.clone()));
        Ok(())
    });
    let placements: Vec<ThreadPlacement> = (0..CHANNELS)
        .map(|i| ThreadPlacement {
            cpus: Some(vec![i]),
            rt_priority: Some(18),
            spin: false,
        })
        .collect();
    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPreset::Balanced,
        FilterPhase::Linear,
        Apodization::Off,
        6,
        FRAMES,
        None,
        Some(WorkerSetup {
            placements: placements.clone(),
            hook: Some(hook),
            ..Default::default()
        }),
    )
    .unwrap();
    let input = vec![0.1f64; FRAMES];
    let refs: Vec<&[f64]> = (0..CHANNELS).map(|_| input.as_slice()).collect();
    pool.process_tile_pcm(&refs).unwrap();
    drop(pool);
    let mut got = seen.lock().unwrap().clone();
    got.sort_by_key(|(ch, _)| *ch);
    assert_eq!(got.len(), CHANNELS);
    for (ch, p) in got {
        assert_eq!(p, placements[ch]);
    }
}

fn failing_setup(require: bool) -> audigma_engine::WorkerSetup {
    audigma_engine::WorkerSetup {
        workers: None,
        placements: Vec::new(),
        hook: Some(Arc::new(|ch, _| {
            if ch == 1 {
                Err("no realtime".to_string())
            } else {
                Ok(())
            }
        })),
        require_hook_ok: require,
    }
}

fn build_with(
    setup: audigma_engine::WorkerSetup,
) -> Result<ChannelWorkerPool, audigma_engine::traits::EngineError> {
    ChannelWorkerPool::new(
        CHANNELS,
        FilterPreset::Balanced,
        FilterPhase::Linear,
        Apodization::Off,
        6,
        FRAMES,
        None,
        Some(setup),
    )
}

#[test]
fn required_hook_failure_aborts_pool_construction() {
    match build_with(failing_setup(true)) {
        Err(audigma_engine::traits::EngineError::WorkerThread { channel, reason }) => {
            assert_eq!(channel, 1);
            assert!(reason.contains("no realtime"), "{reason}");
        }
        other => panic!("expected worker setup failure, got ok={}", other.is_ok()),
    }
}

#[test]
fn optional_hook_failure_keeps_pool_running() {
    let mut pool = build_with(failing_setup(false)).unwrap();
    let input = vec![0.1f64; FRAMES];
    let refs: Vec<&[f64]> = (0..CHANNELS).map(|_| input.as_slice()).collect();
    pool.process_tile_pcm(&refs).unwrap();
}
