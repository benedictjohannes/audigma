//! Psychoacoustic volume conversions and mapping curves (`alsa-utils volume_mapping.c`).

/// Default software volume lower floor in decibels (-60.0 dBFS).
pub const DEFAULT_SOFTWARE_MIN_DB: f64 = -60.0;

/// Converts a perceptual volume percentage [0.0, 100.0] to decibels using ALSA's `volume_mapping.c` curve.
///
/// Uses `min_db` as the lower boundary (e.g. -50.0 dB for JA11, or -60.0 dB for software volume).
/// At 0%, returns -120.0 dB (silence/mute).
pub fn alsamixer_percentage_to_db(pct: f64, min_db: f64) -> f64 {
    if pct <= 0.0 {
        return -120.0;
    }
    let p = pct / 100.0;
    let min_norm = 10.0_f64.powf(min_db / 60.0);
    let val = min_norm + p * (1.0 - min_norm);
    if val <= 1e-6 {
        return -120.0;
    }
    (60.0 * val.log10()).clamp(min_db, 12.0)
}

/// Converts decibels back to a perceptual volume percentage [0.0, 100.0] using ALSA's `volume_mapping.c` curve.
pub fn alsamixer_db_to_percentage(db: f64, min_db: f64) -> f64 {
    if db <= min_db || db <= -120.0 {
        return 0.0;
    }
    if db >= 0.0 {
        return 100.0;
    }
    let min_norm = 10.0_f64.powf(min_db / 60.0);
    let norm = 10.0_f64.powf(db / 60.0);
    let p = (norm - min_norm) / (1.0 - min_norm);
    (p * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ja11_hardware_alsamixer_values() {
        let min_db = -50.0;

        // 100% -> 0 dB
        let db_100 = alsamixer_percentage_to_db(100.0, min_db);
        assert!((db_100 - 0.0).abs() < 1e-3);
        assert!((alsamixer_db_to_percentage(db_100, min_db) - 100.0).abs() < 1e-2);

        // 93% -> -1.5 dB
        let pct_93 = alsamixer_db_to_percentage(-1.5, min_db);
        assert_eq!(pct_93.round() as i64, 93);

        // 87% -> -3.0 dB
        let pct_87 = alsamixer_db_to_percentage(-3.0, min_db);
        assert_eq!(pct_87.round() as i64, 87);

        // 76% -> -6.0 dB
        let pct_76 = alsamixer_db_to_percentage(-6.0, min_db);
        assert_eq!(pct_76.round() as i64, 76);

        // 53% -> -13.5 dB
        let pct_53 = alsamixer_db_to_percentage(-13.5, min_db);
        assert_eq!(pct_53.round() as i64, 53);

        // 0% -> -120 dB
        let db_0 = alsamixer_percentage_to_db(0.0, min_db);
        assert_eq!(db_0, -120.0);
    }

    #[test]
    fn test_software_alsamixer_values() {
        let min_db = DEFAULT_SOFTWARE_MIN_DB;

        // 100% -> 0 dB
        assert!((alsamixer_percentage_to_db(100.0, min_db) - 0.0).abs() < 1e-3);

        // 50% -> around -15.58 dB
        let db_50 = alsamixer_percentage_to_db(50.0, min_db);
        assert!((db_50 - (-15.58)).abs() < 0.1);

        // 10% -> around -43.27 dB (still audible background music)
        let db_10 = alsamixer_percentage_to_db(10.0, min_db);
        assert!((db_10 - (-43.27)).abs() < 0.1);

        // Roundtrip test across 1..99%
        for p in 1..=99 {
            let pct = p as f64;
            let db = alsamixer_percentage_to_db(pct, min_db);
            let recovered = alsamixer_db_to_percentage(db, min_db);
            assert!(
                (recovered - pct).abs() < 0.1,
                "Failed roundtrip for pct={pct}, db={db}, recovered={recovered}"
            );
        }
    }
}
