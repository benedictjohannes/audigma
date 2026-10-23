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

    let dt = chunk_frames as f64 / 48000.0;
    let drift_cfg = ClockDriftConfig {
        target_fill_secs: 8.0 * dt,
        max_correction_ppm: 500.0,
        kp: 300.0,
        ki: 10.0,
        min_fill_secs: dt,
        max_fill_secs: 15.0 * dt,
        integral_clamp_ppm: 300.0,
        ..Default::default()
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
        let fill_secs = cons.slots() as f64 * dt;
        let drift_ppm = drift_ctrl
            .update(fill_secs, dt)
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

#[test]
fn test_closed_loop_plant_recovers_from_buffer_disturbance() {
    let cfg = ClockDriftConfig {
        target_fill_secs: 0.060,
        max_correction_ppm: 500.0,
        kp: 600.0,
        ki: 50.0,
        min_fill_secs: 0.010,
        max_fill_secs: 0.200,
        integral_clamp_ppm: 200.0,
        ..Default::default()
    };

    let sample_rate = 48000.0;
    let dt = 0.005; // 5 ms control step

    // Directionality test: verify sign physics on the plant. Use a fresh
    // controller per direction so smoothing carryover cannot mask the sign.
    let mut low_controller = AdaptiveClockDriftController::new(cfg.clone());
    let low_fill = 0.040; // 40 ms < 60 ms target
    let low_ppm = low_controller.update(low_fill, dt).unwrap();
    assert!(
        low_ppm < 0.0,
        "Low buffer must request negative PPM to slow consumption, got {low_ppm}"
    );

    // Negative PPM must cause input buffer to accumulate (increase)
    let input_arrived = sample_rate * dt;
    let input_consumed = sample_rate * dt * (1.0 + low_ppm / 1_000_000.0);
    assert!(
        input_arrived > input_consumed,
        "Negative PPM must reduce consumption below arrival rate to restore low buffer"
    );

    let mut high_controller = AdaptiveClockDriftController::new(cfg);
    let high_fill = 0.080; // 80 ms > 60 ms target
    let high_ppm = high_controller.update(high_fill, dt).unwrap();
    assert!(
        high_ppm > 0.0,
        "High buffer must request positive PPM to accelerate consumption, got {high_ppm}"
    );
    let input_consumed_high = sample_rate * dt * (1.0 + high_ppm / 1_000_000.0);
    assert!(
        input_consumed_high > input_arrived,
        "Positive PPM must increase consumption above arrival rate to drain high buffer"
    );

    // Closed-loop simulation with hardware clock oscillator offset (+50 PPM drift).
    // Using critically-damped test gains (ζ ≈ 0.707) so the loop settles within 60s
    // without requiring an 8-minute real-time simulation.
    let mut controller = AdaptiveClockDriftController::new(ClockDriftConfig {
        target_fill_secs: 0.060,
        max_correction_ppm: 500.0,
        kp: 6000.0,
        ki: 300.0,
        min_fill_secs: 0.010,
        max_fill_secs: 0.200,
        integral_clamp_ppm: 300.0,
        ..Default::default()
    });
    let mut buffered_frames = 0.060 * sample_rate;
    let producer_drift_ppm = 50.0;
    let producer_rate = sample_rate * (1.0 + producer_drift_ppm / 1_000_000.0);

    // Simulate 80 seconds (16,000 steps of 5 ms)
    for _ in 0..16_000 {
        let fill_secs = buffered_frames / sample_rate;
        let ppm = controller.update(fill_secs, dt).unwrap();
        let arrived = producer_rate * dt;
        let consumed = sample_rate * dt * (1.0 + ppm / 1_000_000.0);
        buffered_frames += arrived - consumed;
    }

    let final_fill_secs = buffered_frames / sample_rate;
    let final_ppm = controller.last_drift_ppm();

    // The PI controller must lock onto the +50 PPM drift and stabilize the buffer around 60 ms
    assert!(
        (final_ppm - 50.0).abs() < 2.0,
        "Controller should track +50 PPM drift (got {final_ppm:.1} PPM)"
    );
    assert!(
        (final_fill_secs - 0.060).abs() < 0.002,
        "Buffer should remain locked at ~60 ms under drift (got {:.3} ms)",
        final_fill_secs * 1000.0
    );
}

#[test]
fn test_default_gains_track_slow_clock_drift() {
    // The production defaults are deliberately slow to avoid audible pitch
    // modulation. Validate that they still converge on a steady +50 ppm
    // oscillator offset over a long horizon (1200 s at 5 ms steps).
    let sample_rate = 48_000.0;
    let dt = 0.005;
    let mut controller = AdaptiveClockDriftController::new(ClockDriftConfig::default());

    let mut buffered_frames = 0.060 * sample_rate;
    let producer_drift_ppm = 50.0;
    let producer_rate = sample_rate * (1.0 + producer_drift_ppm / 1_000_000.0);

    for _ in 0..240_000 {
        let fill_secs = buffered_frames / sample_rate;
        let ppm = controller
            .update(fill_secs, dt)
            .expect("default gains must not trip the safety rails under 50 ppm drift");
        let arrived = producer_rate * dt;
        let consumed = sample_rate * dt * (1.0 + ppm / 1_000_000.0);
        buffered_frames += arrived - consumed;
    }

    let final_fill_secs = buffered_frames / sample_rate;
    let final_ppm = controller.last_drift_ppm();

    assert!(
        (final_ppm - producer_drift_ppm).abs() < 2.0,
        "Default gains should track +50 PPM (got {final_ppm:.1} PPM)"
    );
    assert!(
        (final_fill_secs - 0.060).abs() < 0.001,
        "Default gains should hold the 60 ms target (got {:.3} ms)",
        final_fill_secs * 1000.0
    );
}
