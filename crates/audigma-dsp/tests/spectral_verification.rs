//! Integration test for audigma-dsp spectral and stability verification (`Design.md §8`).

use std::f64::consts::PI;

use audigma_dsp::{
    CrfbModulator, DitherType, DsdBitRateHz, FilterPreset, HalfBandCascade, OverloadPolicy,
    ScopedFtzDaz, get_crfb_coefficients,
};

#[test]
fn test_all_rates_silence_stability() {
    let _ftz = ScopedFtzDaz::new();
    let rates = [
        DsdBitRateHz::DSD64,
        DsdBitRateHz::DSD128,
        DsdBitRateHz::DSD256,
        DsdBitRateHz::DSD64_48,
        DsdBitRateHz::DSD128_48,
        DsdBitRateHz::DSD256_48,
    ];

    for rate in rates {
        let coeffs = *get_crfb_coefficients(rate)
            .unwrap_or_else(|| panic!("Missing coefficients for rate {:?}", rate));
        let mut modulator =
            CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        const SAMPLES: usize = 65_536;
        let silence = [0.0f64; 4096];
        let mut bytes = [0u8; 512];
        let mut total_ones = 0;

        for _ in 0..(SAMPLES / 4096) {
            modulator.process_tile_bytes(&silence, &mut bytes).unwrap();
            total_ones += bytes.iter().map(|b| b.count_ones() as usize).sum::<usize>();
        }

        assert_eq!(
            modulator.overload_count(),
            0,
            "Rate {:?} overloaded on silence",
            rate
        );
        for &s in &modulator.state() {
            assert!(
                s.abs() < CrfbModulator::STABILITY_THRESHOLD,
                "Rate {:?} state exploded: {:?}",
                rate,
                modulator.state()
            );
        }

        let ratio = total_ones as f64 / SAMPLES as f64;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "Rate {:?} ones ratio {} deviated from 0.5 on silence",
            rate,
            ratio
        );
    }
}

#[test]
fn test_all_rates_dc_bias_stability() {
    let _ftz = ScopedFtzDaz::new();
    let rates = [
        DsdBitRateHz::DSD64,
        DsdBitRateHz::DSD128,
        DsdBitRateHz::DSD256,
        DsdBitRateHz::DSD64_48,
        DsdBitRateHz::DSD128_48,
        DsdBitRateHz::DSD256_48,
    ];

    for rate in rates {
        let coeffs = *get_crfb_coefficients(rate).unwrap();
        let mut modulator =
            CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        let dc_in = [0.5f64; 4096];
        let mut bytes = [0u8; 512];
        let mut total_ones = 0;

        for _ in 0..8 {
            modulator.process_tile_bytes(&dc_in, &mut bytes).unwrap();
            total_ones += bytes.iter().map(|b| b.count_ones() as usize).sum::<usize>();
        }

        assert_eq!(modulator.overload_count(), 0);
        for &s in &modulator.state() {
            assert!(s.abs() < CrfbModulator::STABILITY_THRESHOLD);
        }
        let ratio = total_ones as f64 / 32_768.0;
        assert!(
            ratio > 0.5,
            "Positive DC should bias ones ratio above 0.5 (got {})",
            ratio
        );
    }
}

#[test]
fn test_overload_policy_recovery() {
    let _ftz = ScopedFtzDaz::new();
    let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();

    // Error policy
    let mut mod_err = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::Error);
    let overload = [10.0f64; 32];
    let mut bits = [0u8; 32];
    assert!(mod_err.process_tile_bits(&overload, &mut bits).is_err());

    // MuteRecover policy
    let mut mod_rec = CrfbModulator::new(coeffs, DitherType::None, OverloadPolicy::MuteRecover);
    let mut out_words = [0u32; 1];
    assert!(mod_rec.process_tile_u32(&overload, &mut out_words).is_ok());
    assert!(mod_rec.overload_count() > 0);

    // After overload, process normal sine tone and verify recovery to stable state
    let normal_sine = [0.1f64; 4096];
    let mut norm_bytes = [0u8; 512];
    assert!(
        mod_rec
            .process_tile_bytes(&normal_sine, &mut norm_bytes)
            .is_ok()
    );
    for &s in &mod_rec.state() {
        assert!(s.abs() < CrfbModulator::STABILITY_THRESHOLD);
    }
}

#[test]
fn test_cascade_and_modulator_end_to_end_presets() {
    let _ftz = ScopedFtzDaz::new();
    let presets = [
        FilterPreset::Balanced,
        FilterPreset::Reference,
        FilterPreset::LowLatency,
    ];

    for preset in presets {
        let mut cascade = HalfBandCascade::new(preset, 6, 64); // 64x to DSD64
        let coeffs = *get_crfb_coefficients(DsdBitRateHz::DSD64).unwrap();
        let mut modulator =
            CrfbModulator::new(coeffs, DitherType::Tpdf, OverloadPolicy::MuteRecover);

        let mut bb_input = [0.0f64; 64];
        let amp = 10.0_f64.powf(-6.0 / 20.0);
        for (i, s) in bb_input.iter_mut().enumerate() {
            let t = i as f64 / 44_100.0;
            *s = amp * (2.0 * PI * 1000.0 * t).sin();
        }

        let mut exp_scratch = vec![0.0f64; 64 * 64];
        let mut tile_bytes = vec![0u8; (64 * 64) / 8];

        for _ in 0..20 {
            cascade.process_tile(&bb_input, &mut exp_scratch).unwrap();
            modulator
                .process_tile_bytes(&exp_scratch, &mut tile_bytes)
                .unwrap();
        }

        assert_eq!(modulator.overload_count(), 0);
        for &s in &modulator.state() {
            assert!(s.abs() < CrfbModulator::STABILITY_THRESHOLD);
        }
    }
}
