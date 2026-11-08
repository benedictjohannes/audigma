use audigma_dsp::fir::PolyphaseResampler;
use audigma_dsp::rates::PcmRateHz;
use audigma_engine::asrc::{ClockDriftConfig, ClockDriftController};
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
        min_fill_secs: dt,
        max_fill_secs: 15.0 * dt,
        ..Default::default()
    };
    let mut drift_ctrl = ClockDriftController::new(drift_cfg, true);

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
        drift_ctrl.applied_ppm().abs() <= 500.0,
        "Drift correction must remain bounded within 500 PPM"
    );
}

// Closed-loop plants. The consumer reads one 256-frame tile per step and the ASRC stretches it
// by the applied correction, so a tile lasts `256 / (rate * (1 + ppm))` of DAC time. The
// producer delivers whole PipeWire-sized chunks (1024 frames, 23 ms at 44.1 kHz), which gives
// the queue the same sawtooth as the real sink.

const RATE: f64 = 44_100.0;
const TILE: f64 = 256.0;
const CHUNK: f64 = 1024.0;

/// How a free-running input reports its fill (`AudioInput::buffer_fill_secs`).
#[derive(Clone, Copy)]
enum FillReport {
    /// The `FreeRunning` contract: queued frames plus the producer's progress interpolated at
    /// the nominal rate since the last chunk arrival (capped at one chunk).
    Continuous,
    /// Queued frames only. Chunk and tile are commensurate, so the fill lives on a 256-frame
    /// lattice and only steps when the producer has gained a whole tile.
    Queued,
}

enum Producer {
    /// Own clock, offset by `drift_ppm(dac_secs)` from the DAC.
    FreeRunning {
        drift_ppm: fn(f64) -> f64,
        report: FillReport,
        /// Frames produced but not yet delivered as a chunk.
        pending: f64,
        /// DAC time of the last chunk arrival.
        last_arrival: f64,
    },
    /// Pulled whenever the fill drops below the cushion (the PipeWire sink's demand gate).
    Gated { cushion_secs: f64 },
}

struct Plant {
    producer: Producer,
    buffered: f64,
    dac_secs: f64,
}

impl Plant {
    fn new(producer: Producer) -> Self {
        Self {
            producer,
            buffered: 0.090 * RATE,
            dac_secs: 0.0,
        }
    }

    /// Advances one tile with `ppm` applied; returns `(fill_secs, dt_secs)` as the live loop
    /// passes them (dt measured in consumed input frames).
    fn step(&mut self, ppm: f64) -> (f64, f64) {
        let start = self.dac_secs;
        let tile_dac_secs = TILE / (RATE * (1.0 + ppm * 1e-6));
        self.dac_secs += tile_dac_secs;
        self.buffered -= TILE;
        let fill = match &mut self.producer {
            Producer::FreeRunning {
                drift_ppm,
                report,
                pending,
                last_arrival,
            } => {
                let rate = RATE * (1.0 + drift_ppm(start) * 1e-6);
                let mut t = start;
                while *pending + rate * (self.dac_secs - t) >= CHUNK {
                    t += (CHUNK - *pending) / rate;
                    *pending = 0.0;
                    *last_arrival = t;
                    self.buffered += CHUNK;
                }
                *pending += rate * (self.dac_secs - t);
                match report {
                    FillReport::Continuous => {
                        self.buffered + ((self.dac_secs - *last_arrival) * RATE).min(CHUNK)
                    }
                    FillReport::Queued => self.buffered,
                }
            }
            Producer::Gated { cushion_secs } => {
                while self.buffered < *cushion_secs * RATE {
                    self.buffered += CHUNK;
                }
                self.buffered
            }
        };
        (fill / RATE, TILE / RATE)
    }
}

/// Runs `secs` of DAC time, calling `check(elapsed, controller)` after every tile; returns the
/// mean reported fill over the last 10 s.
fn run(
    plant: &mut Plant,
    ctrl: &mut ClockDriftController,
    secs: f64,
    mut check: impl FnMut(f64, &ClockDriftController),
) -> f64 {
    let mut ppm = ctrl.applied_ppm();
    let (mut tail_sum, mut tail_n) = (0.0, 0.0);
    let start = plant.dac_secs;
    while plant.dac_secs - start < secs {
        let (fill, dt) = plant.step(ppm);
        ppm = ctrl.update(fill, dt).expect("no rail in a healthy plant");
        check(plant.dac_secs - start, ctrl);
        if plant.dac_secs - start > secs - 10.0 {
            tail_sum += fill;
            tail_n += 1.0;
        }
    }
    tail_sum / tail_n
}

fn free_running(drift_ppm: fn(f64) -> f64, report: FillReport) -> Plant {
    Plant::new(Producer::FreeRunning {
        drift_ppm,
        report,
        pending: 0.0,
        last_arrival: 0.0,
    })
}

#[test]
fn free_running_offset_is_measured_and_corrected() {
    for (drift, f) in [
        (200.0, (|_| 200.0) as fn(f64) -> f64),
        (-50.0, |_| -50.0),
        (450.0, |_| 450.0),
    ] {
        let mut plant = free_running(f, FillReport::Continuous);
        let mut ctrl = ClockDriftController::new(ClockDriftConfig::default(), true);
        let mean_fill = run(&mut plant, &mut ctrl, 900.0, |t, c| {
            if t > 120.0 {
                let est = c.estimate_ppm().unwrap();
                assert!(
                    (est - drift).abs() < 2.0,
                    "{drift} ppm: estimate {est:.2} at {t:.0} s"
                );
            }
        });
        // Converged: the trim is back inside (or at the edge of) its dead band.
        let applied = ctrl.applied_ppm();
        assert!(
            (applied - drift).abs() < 6.0,
            "{drift} ppm: applied {applied:.2}"
        );
        let setpoint = ctrl.setpoint_secs().unwrap();
        assert!(
            (mean_fill - setpoint).abs() < 0.0065,
            "{drift} ppm: fill {:.1} ms vs setpoint {:.1} ms",
            mean_fill * 1e3,
            setpoint * 1e3
        );
    }
}

#[test]
fn free_running_step_change_is_followed() {
    let mut plant = free_running(
        |t| if t < 100.0 { 0.0 } else { 300.0 },
        FillReport::Continuous,
    );
    let mut ctrl = ClockDriftController::new(ClockDriftConfig::default(), true);
    run(&mut plant, &mut ctrl, 300.0, |t, c| {
        if t > 200.0 {
            let est = c.estimate_ppm().unwrap();
            assert!((est - 300.0).abs() < 2.0, "estimate {est:.2} at {t:.0} s");
        }
    });
}

#[test]
fn queued_only_fill_cannot_resolve_drift() {
    // Why `FreeRunning` inputs must report a continuous fill: on the 256-frame lattice the
    // 32 s window sees about one step at 200 ppm, so the estimate swings by hundreds of ppm.
    let mut plant = free_running(|_| 200.0, FillReport::Queued);
    let mut ctrl = ClockDriftController::new(ClockDriftConfig::default(), false);
    let mut worst: f64 = 0.0;
    run(&mut plant, &mut ctrl, 300.0, |t, c| {
        if t > 60.0 {
            worst = worst.max((c.estimate_ppm().unwrap() - 200.0).abs());
        }
    });
    assert!(
        worst > 100.0,
        "lattice estimate unexpectedly accurate: {worst:.1} ppm"
    );
}

#[test]
fn gated_input_observed_reads_zero() {
    // The PipeWire sink case with `drift_correction = "on"`: estimate only, nothing applied.
    let mut plant = Plant::new(Producer::Gated {
        cushion_secs: 0.090,
    });
    let mut ctrl = ClockDriftController::new(ClockDriftConfig::default(), false);
    run(&mut plant, &mut ctrl, 600.0, |t, c| {
        assert_eq!(c.applied_ppm(), 0.0);
        // Valid after the 2 s warm-up plus 16 blocks of 0.5 s.
        if t > 10.5 {
            let est = c.estimate_ppm().unwrap();
            assert!(est.abs() < 5.0, "estimate {est:.2} at {t:.0} s");
        }
    });
}

#[test]
fn gated_input_misdeclared_as_free_running_stays_bounded() {
    // Applying the estimate to a gated producer is circular (it measures its own output), which
    // is why the engine never does it; if an input misdeclares its clock the correction must
    // still not creep towards the limit like the old PI loop.
    let mut plant = Plant::new(Producer::Gated {
        cushion_secs: 0.090,
    });
    let mut ctrl = ClockDriftController::new(ClockDriftConfig::default(), true);
    run(&mut plant, &mut ctrl, 600.0, |t, c| {
        assert!(
            c.applied_ppm().abs() < 50.0,
            "{:.1} ppm at {t:.0} s",
            c.applied_ppm()
        );
    });
}
