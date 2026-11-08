//! Integration test proving bit-exact output equivalence between sequential
//! and persistent real-time ChannelWorkerPool DSP execution (`Design.md §1.2`).

use audigma_dsp::DcBlocker;
use audigma_dsp::config::{Apodization, DitherType, FilterPhase, OverloadPolicy};
use audigma_dsp::fir::HalfBandCascade;
use audigma_dsp::modulator::Modulator;
use audigma_dsp::rates::DsdBitRateHz;
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};
use std::f64::consts::PI;

#[test]
fn test_worker_pool_stereo_dsd64_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD64, 6, 100, 5);
}

#[test]
fn test_worker_pool_stereo_dsd128_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD128, 7, 50, 5);
}

#[test]
fn test_worker_pool_stereo_dsd256_reference_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD256, 8, 20, 5);
}

#[test]
fn test_worker_pool_stereo_dsd128_order7_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD128, 7, 50, 7);
}

#[test]
fn test_worker_pool_stereo_dsd256_order7_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD256, 8, 20, 7);
}

#[test]
fn test_worker_pool_stereo_dsd512_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD512, 9, 10, 5);
}

#[test]
fn test_worker_pool_stereo_dsd512_order7_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD512, 9, 10, 7);
}

#[test]
fn test_worker_pool_stereo_dsd512_48_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD512_48, 9, 10, 5);
}

#[test]
fn test_dsd_subtiles_partial_tiles_modes_and_reset() {
    {
        for order in [5, 7] {
            for (phase, apodization) in [
                (FilterPhase::Linear, Apodization::Off),
                (FilterPhase::Linear, Apodization::PassEdge19000),
                (FilterPhase::Minimum, Apodization::Off),
                (FilterPhase::Minimum, Apodization::PassEdge20000),
            ] {
                let rate = if phase == FilterPhase::Linear {
                    DsdBitRateHz::DSD512
                } else {
                    DsdBitRateHz::DSD512_48
                };
                let dither = if apodization == Apodization::Off {
                    DitherType::Tpdf
                } else {
                    DitherType::None
                };
                let config = TargetDsdConfig {
                    bit_rate: rate,
                    modulator_order: order,
                    dither,
                    overload_policy: OverloadPolicy::MuteRecover,
                    seed_base: 424242,
                };
                let mut pool =
                    ChannelWorkerPool::new(1, phase, apodization, 9, 512, Some(config), None)
                        .unwrap();
                let mut cascade = HalfBandCascade::new(phase, apodization, 9, 512);
                let mut modulator =
                    Modulator::new(order, rate, dither, OverloadPolicy::MuteRecover).unwrap();
                modulator.set_dither_seed(424242);
                // DSD-configured pools remove DC ahead of the cascade.
                let mut blocker = DcBlocker::production(rate.get() as f64 / 512.0);
                let mut blocked = vec![0.0; 512];
                let input: Vec<f64> = (0..512)
                    .map(|i| 0.05 + 0.3 * (i as f64 * 0.17).sin())
                    .collect();
                let mut pcm = vec![0.0; 512 * 512];
                let mut dsd = vec![0; 512 * 512 / 8];
                assert_eq!(
                    pool.total_group_delay_output_samples(),
                    cascade.total_group_delay_output_samples()
                );
                for _ in 0..2 {
                    for (block, frames) in [0, 1, 31, 32, 33, 255, 256, 511, 512]
                        .into_iter()
                        .enumerate()
                    {
                        let pcm = &mut pcm[..frames * 512];
                        let dsd = &mut dsd[..frames * 512 / 8];
                        blocker.process(&input[..frames], &mut blocked[..frames]);
                        cascade.process_tile(&blocked[..frames], pcm).unwrap();
                        // DSD-configured workers also serve PCM peak analysis.
                        if block % 3 == 0 {
                            pool.process_tile_pcm(&[&input[..frames]]).unwrap();
                        } else {
                            modulator.process_tile_bytes(pcm, dsd).unwrap();
                            pool.process_tile_dsd(&[&input[..frames]]).unwrap();
                            assert_eq!(pool.channel_dsd(0), dsd);
                        }
                        for (actual, expected) in pool.channel_pcm(0).iter().zip(pcm) {
                            assert_eq!(actual.to_bits(), expected.to_bits());
                        }
                        assert_eq!(pool.channel_overload_count(0), modulator.overload_count());
                    }
                    pool.reset().unwrap();
                    blocker.reset();
                    cascade.reset();
                    modulator.reset();
                }
            }
        }
    }
}

#[test]
fn test_worker_pool_stereo_pcm_bit_exact() {
    const CHANNELS: usize = 2;
    const STAGES: usize = 4; // 16x
    const FRAMES: usize = 256;
    const NUM_BLOCKS: usize = 20;

    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPhase::Linear,
        Apodization::Off,
        STAGES,
        FRAMES,
        None,
        None,
    )
    .expect("worker pool creation failed");

    let mut seq_cascade_l =
        HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);
    let mut seq_cascade_r =
        HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, STAGES, FRAMES);

    let mult = pool.rate_multiplier();
    let req_pcm = FRAMES * mult;
    let mut seq_pcm_l = vec![0.0f64; req_pcm];
    let mut seq_pcm_r = vec![0.0f64; req_pcm];

    let mut bb_l = vec![0.0f64; FRAMES];
    let mut bb_r = vec![0.0f64; FRAMES];

    for b in 0..NUM_BLOCKS {
        for i in 0..FRAMES {
            let t = (b * FRAMES + i) as f64 / 44100.0;
            bb_l[i] = 0.5 * (2.0 * PI * 440.0 * t).sin();
            bb_r[i] = 0.5 * (2.0 * PI * 880.0 * t).sin();
        }

        // Sequential
        seq_cascade_l.process_tile(&bb_l, &mut seq_pcm_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut seq_pcm_r).unwrap();

        // Worker Pool
        pool.process_tile_pcm(&[&bb_l, &bb_r]).unwrap();

        assert_eq!(
            seq_pcm_l.as_slice(),
            pool.channel_pcm(0),
            "PCM Left channel mismatch in block {b}"
        );
        assert_eq!(
            seq_pcm_r.as_slice(),
            pool.channel_pcm(1),
            "PCM Right channel mismatch in block {b}"
        );
    }
}

#[test]
fn test_worker_pool_7_1_surround_channels() {
    const CHANNELS: usize = 8;
    const STAGES: usize = 2; // 4x
    const FRAMES: usize = 128;

    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPhase::Linear,
        Apodization::Off,
        STAGES,
        FRAMES,
        None,
        None,
    )
    .expect("7.1 worker pool creation failed");

    assert_eq!(pool.channels(), 8);

    let channel_data: Vec<Vec<f64>> = (0..CHANNELS)
        .map(|ch| vec![(ch as f64 + 1.0) * 0.1; FRAMES])
        .collect();
    let channel_slices: Vec<&[f64]> = channel_data.iter().map(|v| v.as_slice()).collect();

    pool.process_tile_pcm(&channel_slices).unwrap();

    for ch in 0..CHANNELS {
        let out = pool.channel_pcm(ch);
        assert_eq!(out.len(), FRAMES * 4);
        assert!(out.iter().any(|&s| s != 0.0));
    }
}

fn verify_stereo_dsd_bit_exact(
    bit_rate: DsdBitRateHz,
    stages: usize,
    num_blocks: usize,
    order: u8,
) {
    const CHANNELS: usize = 2;
    const FRAMES: usize = 256;
    const SEED_BASE: u64 = 424242;

    let dsd_cfg = TargetDsdConfig {
        bit_rate,
        modulator_order: order,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        seed_base: SEED_BASE,
    };

    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        FilterPhase::Linear,
        Apodization::Off,
        stages,
        FRAMES,
        Some(dsd_cfg.clone()),
        None,
    )
    .expect("worker pool creation failed");

    let mut seq_cascade_l =
        HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, stages, FRAMES);
    let mut seq_cascade_r =
        HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, stages, FRAMES);
    let mut seq_mod_l =
        Modulator::new(order, bit_rate, dsd_cfg.dither, dsd_cfg.overload_policy).unwrap();
    seq_mod_l.set_dither_seed(SEED_BASE);
    let mut seq_mod_r =
        Modulator::new(order, bit_rate, dsd_cfg.dither, dsd_cfg.overload_policy).unwrap();
    seq_mod_r.set_dither_seed(SEED_BASE.wrapping_add(1));
    let baseband_hz = bit_rate.get() as f64 / (1usize << stages) as f64;
    let mut seq_dc_l = DcBlocker::production(baseband_hz);
    let mut seq_dc_r = DcBlocker::production(baseband_hz);
    let mut dc_l = vec![0.0f64; FRAMES];
    let mut dc_r = vec![0.0f64; FRAMES];

    let mult = pool.rate_multiplier();
    let req_pcm = FRAMES * mult;
    let req_bytes = req_pcm / 8;
    let mut seq_pcm_l = vec![0.0f64; req_pcm];
    let mut seq_pcm_r = vec![0.0f64; req_pcm];
    let mut seq_dsd_l = vec![0u8; req_bytes];
    let mut seq_dsd_r = vec![0u8; req_bytes];

    let mut bb_l = vec![0.0f64; FRAMES];
    let mut bb_r = vec![0.0f64; FRAMES];

    for b in 0..num_blocks {
        for i in 0..FRAMES {
            let t = (b * FRAMES + i) as f64 / 44100.0;
            bb_l[i] = 0.4 * (2.0 * PI * 1000.0 * t).sin();
            bb_r[i] = 0.3 * (2.0 * PI * 2500.0 * t).sin();
        }

        // Sequential
        seq_dc_l.process(&bb_l, &mut dc_l);
        seq_dc_r.process(&bb_r, &mut dc_r);
        seq_cascade_l.process_tile(&dc_l, &mut seq_pcm_l).unwrap();
        seq_cascade_r.process_tile(&dc_r, &mut seq_pcm_r).unwrap();
        seq_mod_l
            .process_tile_bytes(&seq_pcm_l, &mut seq_dsd_l)
            .unwrap();
        seq_mod_r
            .process_tile_bytes(&seq_pcm_r, &mut seq_dsd_r)
            .unwrap();

        // Parallel Worker Pool
        pool.process_tile_dsd(&[&bb_l, &bb_r]).unwrap();

        assert_eq!(
            seq_pcm_l.as_slice(),
            pool.channel_pcm(0),
            "PCM Left mismatch in block {b} for rate {bit_rate}"
        );
        assert_eq!(
            seq_pcm_r.as_slice(),
            pool.channel_pcm(1),
            "PCM Right mismatch in block {b} for rate {bit_rate}"
        );
        assert_eq!(
            seq_dsd_l.as_slice(),
            pool.channel_dsd(0),
            "DSD Left byte mismatch in block {b} for rate {bit_rate}"
        );
        assert_eq!(
            seq_dsd_r.as_slice(),
            pool.channel_dsd(1),
            "DSD Right byte mismatch in block {b} for rate {bit_rate}"
        );
    }
}
