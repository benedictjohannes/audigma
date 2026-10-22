//! Integration test proving bit-exact output equivalence between sequential
//! and persistent real-time ChannelWorkerPool DSP execution (`Design.md §1.2`, Phase 5 Step 5.3).

use audigma_dsp::config::{Apodization, DitherType, FilterPhase, FilterPreset, OverloadPolicy};
use audigma_dsp::fir::HalfBandCascade;
use audigma_dsp::modulator::CrfbModulator;
use audigma_dsp::presets::{PRESET_BALANCED, PRESET_REFERENCE};
use audigma_dsp::rates::DsdBitRateHz;
use audigma_dsp::tables;
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};
use std::f64::consts::PI;

#[test]
fn test_worker_pool_stereo_dsd64_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD64, 6, PRESET_BALANCED.preset, 100);
}

#[test]
fn test_worker_pool_stereo_dsd128_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD128, 7, PRESET_BALANCED.preset, 50);
}

#[test]
fn test_worker_pool_stereo_dsd256_reference_bit_exact() {
    verify_stereo_dsd_bit_exact(DsdBitRateHz::DSD256, 8, PRESET_REFERENCE.preset, 20);
}

#[test]
fn test_worker_pool_stereo_pcm_bit_exact() {
    const CHANNELS: usize = 2;
    const STAGES: usize = 4; // 16x
    const FRAMES: usize = 256;
    const NUM_BLOCKS: usize = 20;

    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        PRESET_BALANCED.preset,
        FilterPhase::Linear,
        Apodization::Off,
        STAGES,
        FRAMES,
        None,
        None,
    )
    .expect("worker pool creation failed");

    let mut seq_cascade_l = HalfBandCascade::new(
        PRESET_BALANCED.preset,
        FilterPhase::Linear,
        Apodization::Off,
        STAGES,
        FRAMES,
    );
    let mut seq_cascade_r = HalfBandCascade::new(
        PRESET_BALANCED.preset,
        FilterPhase::Linear,
        Apodization::Off,
        STAGES,
        FRAMES,
    );

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
        PRESET_BALANCED.preset,
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
    preset: FilterPreset,
    num_blocks: usize,
) {
    const CHANNELS: usize = 2;
    const FRAMES: usize = 256;
    const SEED_BASE: u64 = 424242;

    let dsd_cfg = TargetDsdConfig {
        bit_rate,
        dither: DitherType::Tpdf,
        overload_policy: OverloadPolicy::MuteRecover,
        seed_base: SEED_BASE,
    };

    let mut pool = ChannelWorkerPool::new(
        CHANNELS,
        preset,
        FilterPhase::Linear,
        Apodization::Off,
        stages,
        FRAMES,
        Some(dsd_cfg.clone()),
        None,
    )
    .expect("worker pool creation failed");

    let mut seq_cascade_l = HalfBandCascade::new(
        preset,
        FilterPhase::Linear,
        Apodization::Off,
        stages,
        FRAMES,
    );
    let mut seq_cascade_r = HalfBandCascade::new(
        preset,
        FilterPhase::Linear,
        Apodization::Off,
        stages,
        FRAMES,
    );
    let coeffs = tables::get_crfb_coefficients(bit_rate).unwrap();
    let mut seq_mod_l = CrfbModulator::new(*coeffs, dsd_cfg.dither, dsd_cfg.overload_policy);
    seq_mod_l.set_dither_seed(SEED_BASE);
    let mut seq_mod_r = CrfbModulator::new(*coeffs, dsd_cfg.dither, dsd_cfg.overload_policy);
    seq_mod_r.set_dither_seed(SEED_BASE.wrapping_add(1));

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
        seq_cascade_l.process_tile(&bb_l, &mut seq_pcm_l).unwrap();
        seq_cascade_r.process_tile(&bb_r, &mut seq_pcm_r).unwrap();
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
