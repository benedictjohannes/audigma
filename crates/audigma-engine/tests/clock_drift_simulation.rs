use audigma_dsp::fir::PolyphaseResampler;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::asrc::{AdaptiveClockDriftController, ClockDriftConfig};
use audigma_engine::queue::{AudioSpscQueue, TaggedChunk};

#[test]
fn test_simulated_clock_drift_and_generation_filtering() {
    let chunk_frames = 256;
    let channels = 2;
    let chunk_samples = chunk_frames * channels;

    // 16-slot ring buffer
    let (mut prod, mut cons) = AudioSpscQueue::new_empty(16);

    let drift_cfg = ClockDriftConfig {
        target_fill: 0.5,
        max_correction_ppm: 500.0,
        kp: 300.0,
        ki: 10.0,
        min_fill_threshold: 0.05,
        max_fill_threshold: 0.95,
        integral_clamp_ppm: 300.0,
    };
    let mut drift_ctrl = AdaptiveClockDriftController::new(drift_cfg);

    // Low-rate baseband ASRC resampler: nominal 48000 -> 48000
    let mut asrc = PolyphaseResampler::new_asrc(
        PcmRateHz::new(48000).unwrap(),
        PcmRateHz::new(48000).unwrap(),
    );

    let mut current_generation = 1u64;
    let mut producer_chunk = TaggedChunk::new(chunk_samples);

    // Initial pre-roll: fill 8 slots (50%)
    for _ in 0..8 {
        producer_chunk.set(current_generation, channels, &vec![0.1; chunk_samples]);
        prod.push(producer_chunk.clone()).unwrap();
    }

    let mut processed_frames = 0;
    let mut dropped_stale_chunks = 0;

    // Simulate 200 cycles where producer produces samples and consumer drains
    for cycle in 0..200 {
        // Once at cycle 100, trigger format reconfiguration (bump generation tag)
        if cycle == 100 {
            current_generation = 2;
        }

        // Producer pushes a chunk if space is available
        if prod.slots() > 0 {
            producer_chunk.set(current_generation, channels, &vec![0.2; chunk_samples]);
            prod.push(producer_chunk.clone()).unwrap();
        }

        // Consumer reads a chunk
        if let Ok(chunk) = cons.pop() {
            // Verify generation tag filtering
            if chunk.generation != current_generation {
                dropped_stale_chunks += 1;
            } else {
                // Process through ASRC
                let mut out_pcm = vec![0.0; chunk_samples * 2];
                let (consumed, produced) = asrc.process(chunk.as_slice(), &mut out_pcm);
                assert!(consumed > 0);
                processed_frames += produced / channels;
            }
        }

        // Update drift controller
        let occupied = cons.slots();
        let total = 16;
        let dt = chunk_frames as f64 / 48000.0;
        let drift_ppm = drift_ctrl
            .update(occupied, total, dt)
            .expect("Must remain within safe bounds");

        // Pass calculated PPM to baseband ASRC resampler
        asrc.set_drift_ppm(drift_ppm);
    }

    // Must have successfully dropped stale generation 1 chunks remaining after cycle 100
    assert!(
        dropped_stale_chunks > 0,
        "Stale chunks should be filtered out by generation tag"
    );
    assert!(
        processed_frames > 0,
        "Should have processed frames through low-rate ASRC"
    );
    assert!(
        drift_ctrl.last_drift_ppm().abs() <= 500.0,
        "Drift correction must remain bounded within 500 PPM"
    );
}
