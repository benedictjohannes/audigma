//! The pool blocks through the `Waiter` its `WorkerSetup` carries (`Design.md §3.4`), and the
//! waiter does not affect the output.

use audigma_dsp::config::{Apodization, FilterPhase};
use audigma_engine::worker::ChannelWorkerPool;
use audigma_engine::{ThreadPlacement, Waiter, WorkerSetup};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

const CHANNELS: usize = 2;
const FRAMES: usize = 256;
const BLOCKS: usize = 50;

static WAITS: AtomicUsize = AtomicUsize::new(0);
static WAKES: AtomicUsize = AtomicUsize::new(0);

fn counting_wait(_a: &AtomicU32, _expected: u32, _timeout: Duration) {
    WAITS.fetch_add(1, Ordering::Relaxed);
    std::thread::yield_now();
}

fn counting_wake(_a: &AtomicU32) {
    WAKES.fetch_add(1, Ordering::Relaxed);
}

fn run(waiter: Waiter) -> Vec<Vec<f64>> {
    let no_spin = ThreadPlacement {
        spin: false,
        ..ThreadPlacement::default()
    };
    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPhase::Linear,
        Apodization::Off,
        4,
        FRAMES,
        None,
        Some(WorkerSetup {
            placements: vec![no_spin; CHANNELS],
            waiter,
            ..Default::default()
        }),
    )
    .unwrap();
    let mut out = vec![Vec::new(); CHANNELS];
    for b in 0..BLOCKS {
        let data: Vec<Vec<f64>> = (0..CHANNELS)
            .map(|c| {
                (0..FRAMES)
                    .map(|i| 0.3 * (((b * FRAMES + i) as f64) * 0.013 * (c as f64 + 1.0)).sin())
                    .collect()
            })
            .collect();
        let slices: Vec<&[f64]> = data.iter().map(|v| v.as_slice()).collect();
        pool.process_tile_pcm(&slices).unwrap();
        for (c, o) in out.iter_mut().enumerate() {
            o.extend_from_slice(pool.channel_pcm(c));
        }
    }
    out
}

#[test]
fn pool_blocks_through_the_supplied_waiter() {
    let counted = run(Waiter::new(counting_wait, counting_wake));

    // Every dispatch wakes every worker, and every worker wakes the coordinator once done.
    let wakes = WAKES.load(Ordering::Relaxed);
    assert!(
        wakes >= 2 * BLOCKS * CHANNELS,
        "expected >= {} wakes, got {wakes}",
        2 * BLOCKS * CHANNELS
    );
    // Spinning is disabled, so waiting for a tile must go through the waiter.
    assert!(WAITS.load(Ordering::Relaxed) > 0);

    assert_eq!(counted, run(Waiter::PORTABLE));
}
