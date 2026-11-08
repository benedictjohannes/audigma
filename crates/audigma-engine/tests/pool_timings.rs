//! Worker execution and dispatch histograms: one sample per compute tile, persisted
//! across `reconfigure`, none for reset/reconfigure commands.

use audigma_dsp::config::{Apodization, FilterPhase};
use audigma_engine::timing::{MaxSlot, TimingTracker};
use audigma_engine::worker::ChannelWorkerPool;

const CHANNELS: usize = 3;
const FRAMES: usize = 256;

#[test]
fn pool_records_one_sample_per_worker_and_dispatch_per_tile() {
    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPhase::Linear,
        Apodization::Off,
        2,
        FRAMES,
        None,
        None,
    )
    .unwrap();
    let timings = pool.timings();
    assert_eq!(timings.worker_count(), CHANNELS);
    let mut tracker = TimingTracker::new(timings.clone(), MaxSlot::Health);

    let block = vec![0.1f64; FRAMES];
    let inputs: Vec<&[f64]> = (0..CHANNELS).map(|_| block.as_slice()).collect();
    for _ in 0..10 {
        pool.process_tile_pcm(&inputs).unwrap();
    }
    pool.reset().unwrap(); // not a compute tile

    let r = tracker.window();
    assert_eq!(r.dispatch.count, 10);
    assert_eq!(r.worker.count, 10 * CHANNELS as u64);
    assert!(r.per_worker.iter().all(|s| s.count == 10));
    assert!(r.worker.max_us >= r.worker.p50_us);
    assert!(r.dispatch.max_us >= r.worker.p50_us.min(1));

    pool.reconfigure(FilterPhase::Linear, Apodization::Off, 3, FRAMES, None, None)
        .unwrap();
    pool.process_tile_pcm(&inputs).unwrap();
    let r = tracker.window();
    assert_eq!(r.dispatch.count, 1, "reconfigure is not a timed tile");
    assert_eq!(r.worker_cumulative.count, 11 * CHANNELS as u64);
}
