//! Tests tying the metadata to the real configuration types through the generated JSON Schema,
//! so a new `Settings` key without an entry (or an entry for a removed key) fails here.

use super::*;
use crate::Settings;
use serde_json::json;

fn catalog() -> Catalog {
    Catalog::load().expect("embedded metadata must load")
}

fn root_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(Settings)).unwrap()
}

/// Follows a local `$ref`.
fn deref<'a>(root: &'a Value, node: &'a Value) -> &'a Value {
    match node.get("$ref").and_then(Value::as_str) {
        Some(r) => {
            let mut cur = root;
            for part in r.strip_prefix("#/").expect("local ref").split('/') {
                cur = &cur[part];
            }
            deref(root, cur)
        }
        None => node,
    }
}

/// The non-null alternatives of a (possibly `Option`al, possibly `anyOf`) schema node.
fn variants<'a>(root: &'a Value, node: &'a Value) -> Vec<&'a Value> {
    let node = deref(root, node);
    for key in ["anyOf", "oneOf"] {
        if let Some(arms) = node.get(key).and_then(Value::as_array) {
            return arms.iter().flat_map(|a| variants(root, a)).collect();
        }
    }
    if node.get("type") == Some(&json!("null")) {
        return Vec::new();
    }
    vec![node]
}

/// Every configuration key (a leaf of the schema) with its schema node. `schema_version` is the
/// file's own header, not a setting.
fn leaves(root: &Value) -> BTreeMap<String, &Value> {
    fn walk<'a>(
        root: &'a Value,
        node: &'a Value,
        prefix: &str,
        out: &mut BTreeMap<String, &'a Value>,
    ) {
        let objects: Vec<_> = variants(root, node)
            .into_iter()
            .filter(|v| v.get("properties").is_some())
            .collect();
        if objects.is_empty() {
            out.insert(prefix.to_string(), node);
            return;
        }
        for obj in objects {
            for (k, sub) in obj["properties"].as_object().unwrap() {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                walk(root, sub, &path, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, "", &mut out);
    out.remove("schema_version");
    out
}

/// Text of every enumerated value a leaf accepts, and whether it also accepts a free integer.
fn accepted(root: &Value, node: &Value) -> (BTreeSet<String>, bool) {
    let mut texts = BTreeSet::new();
    let mut free_integer = false;
    for v in variants(root, node) {
        if let Some(values) = v.get("enum").and_then(Value::as_array) {
            texts.extend(values.iter().filter_map(option_text));
        } else if let Some(c) = v.get("const") {
            texts.extend(option_text(c));
        } else if v
            .get("type")
            .is_some_and(|t| t == "integer" || t == &json!(["integer", "null"]))
        {
            free_integer = true;
        }
    }
    (texts, free_integer)
}

#[test]
fn catalog_value_entries_equal_the_settings_schema() {
    let root = root_schema();
    let schema_keys: BTreeSet<String> = leaves(&root).into_keys().collect();
    let catalog_keys: BTreeSet<String> = catalog()
        .fields()
        .iter()
        .filter(|f| f.kind == FieldKind::Value)
        .map(|f| f.path.clone())
        .collect();
    let missing: Vec<_> = schema_keys.difference(&catalog_keys).collect();
    let stale: Vec<_> = catalog_keys.difference(&schema_keys).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "settings keys without metadata: {missing:?}; metadata entries without a settings key: {stale:?}"
    );
}

#[test]
fn groups_are_schema_objects() {
    let root = root_schema();
    let leaf_paths: BTreeSet<String> = leaves(&root).into_keys().collect();
    for f in catalog()
        .fields()
        .iter()
        .filter(|f| f.kind == FieldKind::Group)
    {
        let prefix = format!("{}.", f.path);
        assert!(
            leaf_paths.iter().any(|p| p.starts_with(&prefix)),
            "group `{}` has no keys in the schema",
            f.path
        );
    }
}

/// Values the schema accepts that the metadata deliberately does not list as options.
const UNLISTED: &[(&str, &[&str])] = &[
    // `gentle` is the alias of 19000; the options list the alias.
    ("resampler.apodization", &["19000"]),
    // Packed DSD words are not PCM sample formats a user picks here.
    ("input.sample_format", &["f64", "dsd_u32_be", "dsd_u32_le"]),
    ("target.sample_format", &["f64", "dsd_u32_be", "dsd_u32_le"]),
];

#[test]
fn options_equal_the_values_the_schema_accepts() {
    let root = root_schema();
    let leaf = leaves(&root);
    let c = catalog();
    let mut checked = 0;
    for f in c.fields().iter().filter(|f| !f.options.is_empty()) {
        let (values, free_integer) = accepted(&root, leaf[&f.path]);
        if values.is_empty() {
            continue; // numeric or boolean enum-like keys have no schema enum
        }
        checked += 1;
        let options: BTreeSet<String> = f.options.iter().map(|o| o.key.clone()).collect();
        let unlisted: BTreeSet<String> = UNLISTED
            .iter()
            .find(|(p, _)| *p == f.path)
            .map(|(_, v)| v.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let expected: BTreeSet<String> = values.difference(&unlisted).cloned().collect();
        // `gentle` is an input alias the schema lists too, so it is part of `values`.
        assert_eq!(
            options, expected,
            "`{}`: options differ from the schema's accepted values",
            f.path
        );
        assert_eq!(
            f.also_integer.is_some(),
            free_integer,
            "`{}`: also_integer must match whether the schema accepts an integer",
            f.path
        );
    }
    assert!(checked >= 15, "only {checked} enum keys were compared");
}

// ----- default equivalence: a key set to its documented default equals omitting it -----

/// Defaults whose resolution is not in `audigma-settings`: the consumer applies them. They have
/// no probe here; each is covered where it is applied.
const DEFAULT_APPLIED_ELSEWHERE: &[&str] = &[
    "input.sample_format",     // convert/play stdin reader
    "logging.format",          // audigma-log, at startup
    "output.buffer_ms",        // run (PipeWire queue) and the ALSA output
    "output.period_ms",        // ALSA output
    "offline.tail_policy",     // offline pipeline
    "offline.write_report",    // convert
    "target.dither_tpdf_seed", // PCM quantizer / modulator
];

/// The value consumers see for `path`, resolved from the parsed settings.
fn resolved(s: &Settings, path: &str) -> Option<Value> {
    let to = |v: &dyn erased::Ser| v.to_json();
    let input = s.input.as_ref()?;
    let output = s.output.as_ref()?;
    Some(match path {
        "input.backend" => to(&input.backend()),
        "input.channels" => json!(s.input_channels()),
        "input.rate_mode" => to(&input.rate_mode()),
        "input.allowed_rates" => json!(input.resolved_allowed_rates()),
        "input.allow_dsd" => json!(input.allow_dsd()),
        "input.buffer_ms" => json!(input.buffer_ms()),
        "input.quantum_frames" => json!(input.quantum_frames().frames()),
        "input.dsd.gain_db" => json!(input.dsd_gain_db()),
        "output.quantum_frames" => json!(output.quantum_frames().frames()),
        "output.volume_control" => to(&output.volume_control()),
        "output.dop_sample_format" => to(&output.dop_sample_format()),
        "output.device_wait_ms" => json!(output.device_wait_ms()),
        "output.device_wait_expiry" => to(&output.device_wait_expiry()),
        "output.keep_audio_sink_ms" => json!(output.keep_audio_sink_ms()),
        "resampler.drift_correction" => to(&s.drift_correction()),
        "resampler.max_clock_drift_ppm" => json!(s.resolved_max_clock_drift_ppm()),
        "logging.rt_report_interval_ms" => json!(s.logging.as_ref()?.rt_report_interval_ms()),
        "target.sample_format" => {
            let plan = s.explain_for_input_rate(crate::PcmRateHz::PCM_44100).ok()?;
            match plan.stream_spec.target {
                crate::TargetRateSpec::Pcm { sample_format, .. } => to(&sample_format),
                _ => return None,
            }
        }
        "target.dither" => {
            let plan = s.explain_for_input_rate(crate::PcmRateHz::PCM_44100).ok()?;
            to(&plan.dsp_config.dither)
        }
        // Keys held in a non-optional field with a serde default: the parsed value is the default.
        _ => {
            let (section, key) = path.split_once('.')?;
            let table = match section {
                "gain" => serde_json::to_value(s.gain.as_ref()?),
                "resampler" => serde_json::to_value(s.resampler.as_ref()?),
                "target" => serde_json::to_value(s.target.as_ref()?),
                "offline" => serde_json::to_value(s.offline.as_ref()?),
                "runtime" => serde_json::to_value(s.runtime_config()),
                _ => return None,
            }
            .ok()?;
            table.get(key)?.clone()
        }
    })
}

mod erased {
    pub trait Ser {
        fn to_json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap()
        }
    }
}

/// Numbers compare by value, so `6` equals `6.0`.
fn normalize(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.as_f64().unwrap()),
        Value::Array(a) => Value::Array(a.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

/// A minimal valid configuration with every section present (and empty).
fn base_table() -> toml::Table {
    toml::from_str(
        r#"
schema_version = 1
[input]
[output]
[target]
format = "pcm"
pcm_rate_hz = 96000
[gain]
[resampler]
[offline]
[runtime]
[logging]
"#,
    )
    .unwrap()
}

fn set(table: &mut toml::Table, path: &str, value: &Value) {
    let mut cur = table;
    let parts: Vec<&str> = path.split('.').collect();
    for part in &parts[..parts.len() - 1] {
        cur = cur
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap();
    }
    // Integral floats are written as integers' float form by `toml`; both parse for `f64` keys.
    cur.insert(
        parts[parts.len() - 1].to_string(),
        toml::Value::try_from(value).unwrap(),
    );
}

#[test]
fn a_key_set_to_its_documented_default_equals_omitting_it() {
    let c = catalog();
    let mut probed = 0;
    for f in c.fields().iter().filter(|f| f.kind == FieldKind::Value) {
        let Some(default) = &f.default else { continue };
        if DEFAULT_APPLIED_ELSEWHERE.contains(&f.path.as_str()) {
            continue;
        }
        // Omitted: the consumers' view must be the documented default.
        let omitted = Settings::from_toml_table(base_table()).unwrap();
        let seen = resolved(&omitted, &f.path).unwrap_or_else(|| {
            panic!(
                "`{}`: no resolver; add one or list it as applied elsewhere",
                f.path
            )
        });
        assert_eq!(
            normalize(&seen),
            normalize(default),
            "`{}`: documented default differs from the resolved value when the key is omitted",
            f.path
        );
        // Explicit: writing the default changes nothing the consumers can see.
        let mut table = base_table();
        set(&mut table, &f.path, default);
        let explicit = Settings::from_toml_table(table)
            .unwrap_or_else(|e| panic!("`{}` = {default}: {e}", f.path));
        assert_eq!(
            normalize(&resolved(&explicit, &f.path).unwrap()),
            normalize(default),
            "`{}`: explicit default resolves differently",
            f.path
        );
        assert_eq!(
            explicit
                .explain_for_input_rate(crate::PcmRateHz::PCM_44100)
                .unwrap(),
            omitted
                .explain_for_input_rate(crate::PcmRateHz::PCM_44100)
                .unwrap(),
            "`{}`: explicit default changes the resolved plan",
            f.path
        );
        probed += 1;
    }
    assert!(probed >= 20, "only {probed} defaults were probed");
}

#[test]
fn every_exempt_default_is_a_real_catalog_default() {
    let c = catalog();
    for path in DEFAULT_APPLIED_ELSEWHERE {
        let f = c
            .field(path)
            .unwrap_or_else(|| panic!("`{path}` is not in the catalog"));
        assert!(
            f.default.is_some(),
            "`{path}` has no plain default to exempt"
        );
    }
}
