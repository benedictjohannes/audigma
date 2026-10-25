//! CLI volume argument parsing and validation.

/// Parses a decibel volume argument (e.g. `-6`, `-3.2`, `0`, `0.0`, `+1.5`, `-6dB`).
///
/// Percentages (`80%`) are handled by `parse_volume_arg`, not here: `run_set`
/// forwards them over IPC so the daemon can apply the curve matching the active
/// volume mode, instead of freezing the software curve client-side.
pub fn parse_volume_to_db(arg: &str) -> Result<f64, String> {
    let s = arg.trim();
    if s.is_empty() {
        return Err("volume argument cannot be empty".to_string());
    }

    let clean = s
        .strip_suffix("dB")
        .or_else(|| s.strip_suffix("db"))
        .unwrap_or(s)
        .trim();
    clean
        .parse()
        .map_err(|e| format!("invalid decibel volume '{arg}': {e}"))
}

/// A volume argument parsed from the CLI, shared by `audigma set volume` and `audigma play -v`.
#[derive(Debug, PartialEq)]
pub enum VolumeArg {
    /// Explicit decibels, already stripped of the optional `dB` suffix.
    Db(f64),
    /// Perceptual percentage; the consumer applies the appropriate ALSA curve.
    Percent(f64),
}

/// Parses the shared CLI volume syntax: `<number>` / `<number>dB` for decibels,
/// `<number>%` for a perceptual percentage.
///
/// `run_set` forwards percentages over IPC so the daemon can apply the curve that
/// matches the active volume mode; `run_play` resolves them with the software curve.
pub fn parse_volume_arg(arg: &str) -> Result<VolumeArg, String> {
    let s = arg.trim();
    if s.is_empty() {
        return Err("volume argument cannot be empty".to_string());
    }

    if let Some(pct_str) = s.strip_suffix('%') {
        let p: f64 = pct_str
            .trim()
            .parse()
            .map_err(|e| format!("invalid percentage volume '{arg}': {e}"))?;
        if p < 0.0 {
            return Err("percentage volume cannot be negative".to_string());
        }
        return Ok(VolumeArg::Percent(p));
    }

    parse_volume_to_db(arg).map(VolumeArg::Db)
}

/// Validates a decibel volume against the range the ALSA volume curve can represent.
///
/// Explicit dB values are rejected rather than silently clamped, so a typo like
/// `80` (meaning `80%`) fails loudly instead of quietly boosting by +12 dB.
pub fn validate_volume_db(db: f64) -> Result<f64, String> {
    if !db.is_finite() {
        return Err(format!("invalid volume '{db}' dB"));
    }
    if !(audigma_dsp::MIN_VOLUME_DB..=audigma_dsp::MAX_VOLUME_DB).contains(&db) {
        let hint = if (0.0..=100.0).contains(&db) {
            format!(" (did you mean '{db}%'? percentages require the % suffix)")
        } else {
            String::new()
        };
        return Err(format!(
            "volume {db} dB is outside the supported range {}..={} dB{hint}",
            audigma_dsp::MIN_VOLUME_DB,
            audigma_dsp::MAX_VOLUME_DB
        ));
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_volume_to_db_decibels() {
        assert_eq!(parse_volume_to_db("0").unwrap(), 0.0);
        assert_eq!(parse_volume_to_db("0.0").unwrap(), 0.0);
        assert_eq!(parse_volume_to_db("-6").unwrap(), -6.0);
        assert_eq!(parse_volume_to_db("-3.5").unwrap(), -3.5);
        assert_eq!(parse_volume_to_db("+2.0").unwrap(), 2.0);
        assert_eq!(parse_volume_to_db("-12dB").unwrap(), -12.0);
        assert_eq!(parse_volume_to_db("-10.5 dB").unwrap(), -10.5);
        assert_eq!(parse_volume_to_db("0dB").unwrap(), 0.0);
    }

    #[test]
    fn test_parse_volume_to_db_rejects_percentages() {
        // Percentages are daemon-owned: `run_set` forwards them via
        // `SetVolumePercent` so the active hardware/software ALSA curve is
        // applied at runtime instead of being frozen client-side.
        assert!(parse_volume_to_db("0%").is_err());
        assert!(parse_volume_to_db("80%").is_err());
    }

    #[test]
    fn test_parse_volume_arg_shared_syntax() {
        assert_eq!(parse_volume_arg("-6").unwrap(), VolumeArg::Db(-6.0));
        assert_eq!(parse_volume_arg("-6dB").unwrap(), VolumeArg::Db(-6.0));
        assert_eq!(parse_volume_arg("0").unwrap(), VolumeArg::Db(0.0));
        assert_eq!(parse_volume_arg("+1.5").unwrap(), VolumeArg::Db(1.5));
        assert_eq!(parse_volume_arg("80%").unwrap(), VolumeArg::Percent(80.0));
        assert_eq!(
            parse_volume_arg(" 50 % ").unwrap(),
            VolumeArg::Percent(50.0)
        );

        assert!(parse_volume_arg("-5%").is_err());
        assert!(parse_volume_arg("abc%").is_err());
        assert!(parse_volume_arg("").is_err());
        assert!(parse_volume_arg("xyz").is_err());
    }

    #[test]
    fn test_validate_volume_db_range() {
        assert_eq!(validate_volume_db(-120.0).unwrap(), -120.0);
        assert_eq!(validate_volume_db(0.0).unwrap(), 0.0);
        assert_eq!(validate_volume_db(12.0).unwrap(), 12.0);

        assert!(validate_volume_db(-120.1).is_err());
        assert!(validate_volume_db(12.1).is_err());
        assert!(validate_volume_db(f64::INFINITY).is_err());
        assert!(validate_volume_db(f64::NAN).is_err());

        let err = validate_volume_db(80.0).unwrap_err();
        assert!(err.contains("80%"), "hint missing from error: {err}");
    }

    #[test]
    fn test_parse_volume_to_db_invalid() {
        assert!(parse_volume_to_db("").is_err());
        assert!(parse_volume_to_db("   ").is_err());
        assert!(parse_volume_to_db("xyz").is_err());
        assert!(parse_volume_to_db("1.2.3").is_err());
        assert!(parse_volume_to_db("dB").is_err());
    }
}
