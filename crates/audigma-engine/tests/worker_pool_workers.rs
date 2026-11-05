//! Worker count is decoupled from channel count: channels map round-robin onto workers
//! (`channel % workers`) and the output is identical to one worker per channel.

use audigma_dsp::config::{Apodization, DitherType, FilterPhase, FilterPreset, OverloadPolicy};
use audigma_dsp::rates::DsdBitRateHz;
use audigma_engine::timing::{MaxSlot, TimingTracker};
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};
use audigma_engine::{ThreadInitHook, WorkerSetup};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const CHANNELS: usize = 5;
const FRAMES: usize = 256;

fn dsd_cfg() -> TargetDsdConfig {
    TargetDsdConfig {
        bit_rate: DsdBitRateHz::DSD64,
        modulator_order: 5,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        seed_base: 11,
    }
}

fn build(workers: Option<usize>, dsd: bool, hook: Option<ThreadInitHook>) -> ChannelWorkerPool {
    ChannelWorkerPool::new(
        CHANNELS,
        FilterPreset::Balanced,
        FilterPhase::Linear,
        Apodization::Off,
        6,
        FRAMES,
        dsd.then(dsd_cfg),
        Some(WorkerSetup {
            workers,
            hook,
            ..Default::default()
        }),
    )
    .unwrap()
}

fn signal(block: usize, ch: usize) -> Vec<f64> {
    (0..FRAMES)
        .map(|i| 0.4 * (((block * FRAMES + i) as f64) * 0.017 * (ch as f64 + 1.0)).sin())
        .collect()
}

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
fn any_worker_count_matches_one_worker_per_channel() {
    for dsd in [false, true] {
        let mut reference = build(None, dsd, None);
        assert_eq!(reference.worker_count(), CHANNELS);
        let want = run(&mut reference, dsd, 6);
        for workers in [1, 2, 3, 4, 5, 9] {
            let mut pool = build(Some(workers), dsd, None);
            assert_eq!(pool.channels(), CHANNELS);
            assert_eq!(pool.worker_count(), workers.min(CHANNELS));
            assert_eq!(
                run(&mut pool, dsd, 6),
                want,
                "workers={workers} dsd={dsd} differs"
            );
            assert_eq!(
                pool.total_overload_count(),
                reference.total_overload_count()
            );
            for c in 0..CHANNELS {
                assert_eq!(
                    pool.channel_overload_count(c),
                    reference.channel_overload_count(c)
                );
            }
        }
    }
}

#[test]
fn reconfigure_with_fewer_workers_matches_fresh_pool_and_keeps_threads() {
    let started = Arc::new(AtomicUsize::new(0));
    let hook: ThreadInitHook = {
        let started = Arc::clone(&started);
        Arc::new(move |_, _| {
            started.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    };
    let mut pool = build(Some(2), true, Some(hook));
    run(&mut pool, true, 4);
    pool.reconfigure(
        FilterPreset::Reference,
        FilterPhase::Linear,
        Apodization::Off,
        7,
        FRAMES,
        Some(dsd_cfg()),
        None,
    )
    .unwrap();
    let got = run(&mut pool, true, 5);
    let mut fresh = ChannelWorkerPool::new(
        CHANNELS,
        FilterPreset::Reference,
        FilterPhase::Linear,
        Apodization::Off,
        7,
        FRAMES,
        Some(dsd_cfg()),
        None,
    )
    .unwrap();
    assert_eq!(got, run(&mut fresh, true, 5));
    assert_eq!(
        started.load(Ordering::SeqCst),
        2,
        "hook runs once per worker"
    );
}

#[test]
fn timings_are_per_worker_not_per_channel() {
    let mut pool = build(Some(2), false, None);
    let mut tracker = TimingTracker::new(pool.timings(), MaxSlot::Health);
    run(&mut pool, false, 7);
    let r = tracker.window();
    assert_eq!(r.per_worker.len(), 2);
    assert!(r.per_worker.iter().all(|s| s.count == 7));
    assert_eq!(r.dispatch.count, 7);
}

#[test]
fn inline_mode_matches_threaded_pool_and_reconfigures() {
    for dsd in [false, true] {
        let mut reference = build(None, dsd, None);
        let want = run(&mut reference, dsd, 6);
        let mut pool = build(Some(0), dsd, None);
        assert_eq!(pool.worker_count(), 0);
        assert_eq!(pool.channels(), CHANNELS);
        assert_eq!(run(&mut pool, dsd, 6), want, "inline dsd={dsd} differs");
        for c in 0..CHANNELS {
            assert_eq!(
                pool.channel_overload_count(c),
                reference.channel_overload_count(c)
            );
        }
        // One timing sample per tile, attributed to the single inline "worker".
        let t = pool.timings();
        assert_eq!(t.worker_count(), 1);
    }

    // Reconfigure equals a fresh inline pool; reset works.
    let mut pool = build(Some(0), true, None);
    run(&mut pool, true, 3);
    pool.reconfigure(
        FilterPreset::Reference,
        FilterPhase::Linear,
        Apodization::Off,
        7,
        FRAMES,
        Some(dsd_cfg()),
        None,
    )
    .unwrap();
    let after = run(&mut pool, true, 3);
    let mut fresh = ChannelWorkerPool::new(
        CHANNELS,
        FilterPreset::Reference,
        FilterPhase::Linear,
        Apodization::Off,
        7,
        FRAMES,
        Some(dsd_cfg()),
        Some(WorkerSetup {
            workers: Some(0),
            ..Default::default()
        }),
    )
    .unwrap();
    assert_eq!(after, run(&mut fresh, true, 3));
    pool.reset().unwrap();
}
