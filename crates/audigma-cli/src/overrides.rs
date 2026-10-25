//! Ad-hoc TOML configuration override parsing and manipulation.

use audigma_engine::traits::EngineError;

/// Recursively sets a dotted key (e.g. "output.transport" or "gain.preamp_db") on a `toml::Table`.
pub fn apply_toml_override(
    root: &mut toml::Table,
    key_path: &str,
    raw_value: &str,
) -> Result<(), EngineError> {
    let parts: Vec<&str> = key_path.split('.').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err(EngineError::Pipeline(format!(
            "invalid override key '{key_path}' (expected e.g. output.device)"
        )));
    }

    let parsed_val = parse_override_value(raw_value);

    let mut current = root;
    for (i, &part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            current.insert(part.to_string(), parsed_val);
            return Ok(());
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        match entry {
            toml::Value::Table(tbl) => current = tbl,
            _ => {
                return Err(EngineError::Pipeline(format!(
                    "cannot set nested key '{key_path}': segment '{part}' is not a table"
                )));
            }
        }
    }

    Ok(())
}

/// Parses a CLI override value as a TOML literal, so that an override behaves
/// exactly as if the same value had been written in `config.toml`:
///
/// - `-O gain.preamp_db=-3`    → integer (widened to `f64` by the target field)
/// - `-O gain.preamp_db=-3.0`  → float
/// - `-O output.device='"2"'`  → string `"2"` (quote values that look numeric)
/// - `-O output.device=hw:CARD=K17,DEV=0` → not valid TOML, so treated as a string
/// - `-O input.allowed_rates=[44100, 48000]` → array
///
/// Whether the parsed value fits the target field is decided later by
/// `Settings` deserialization; this function never guesses a type.
pub fn parse_override_value(raw_value: &str) -> toml::Value {
    let trimmed = raw_value.trim();
    if !trimmed.is_empty() {
        // `v = <literal>` keeps the literal's native TOML type. Only `v` is
        // extracted, so extra keys in the fragment cannot leak into the table.
        if let Ok(toml::Value::Table(mut doc)) =
            toml::from_str::<toml::Value>(&format!("v = {trimmed}"))
        {
            if let Some(value) = doc.remove("v") {
                return value;
            }
        }
    }
    toml::Value::String(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_override_value_uses_toml_syntax() {
        assert_eq!(parse_override_value("-3"), toml::Value::Integer(-3));
        assert_eq!(parse_override_value("-3.0"), toml::Value::Float(-3.0));
        assert_eq!(parse_override_value("true"), toml::Value::Boolean(true));
        assert_eq!(
            parse_override_value("\"2\""),
            toml::Value::String("2".to_string())
        );
        assert_eq!(
            parse_override_value("hw:CARD=K17,DEV=0"),
            toml::Value::String("hw:CARD=K17,DEV=0".to_string())
        );
        assert_eq!(
            parse_override_value("[44100, 48000]"),
            toml::Value::Array(vec![
                toml::Value::Integer(44100),
                toml::Value::Integer(48000),
            ])
        );
        // Bare words fall back to strings, so unquoted presets keep working.
        assert_eq!(
            parse_override_value("two-pass"),
            toml::Value::String("two-pass".to_string())
        );
        // Empty value is a string, not an error.
        assert_eq!(parse_override_value(""), toml::Value::String(String::new()));
    }

    #[test]
    fn test_apply_toml_override_nested() {
        let mut root = toml::Table::new();
        apply_toml_override(&mut root, "output.device", "hw:CARD=K17,DEV=0").unwrap();
        apply_toml_override(&mut root, "gain.preamp_db", "-3").unwrap();

        assert_eq!(root["output"]["device"].as_str(), Some("hw:CARD=K17,DEV=0"));
        assert_eq!(root["gain"]["preamp_db"], toml::Value::Integer(-3));
    }

    #[test]
    fn test_apply_toml_override_integer_widens_to_f64_on_deserialize() {
        #[derive(serde::Deserialize)]
        struct Probe {
            preamp_db: f64,
        }

        let mut root = toml::Table::new();
        apply_toml_override(&mut root, "preamp_db", "-3").unwrap();
        let merged = toml::to_string(&root).unwrap();
        let probe: Probe = toml::from_str(&merged).unwrap();
        assert_eq!(probe.preamp_db, -3.0);
    }

    #[test]
    fn test_apply_toml_override_rejects_empty_key_segment() {
        let mut root = toml::Table::new();
        assert!(apply_toml_override(&mut root, "output..device", "x").is_err());
        assert!(apply_toml_override(&mut root, " ", "x").is_err());
    }
}
