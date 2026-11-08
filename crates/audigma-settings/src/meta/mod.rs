//! Typed configurator metadata (`crates/audigma-settings/meta/*.toml`).
//!
//! The TOML files describe, per configuration key, everything a configurator UI needs beyond the
//! type: prose, defaults, visibility rules, recommendations per use case, per-option text, and
//! which assists (probe / benchmark / check) attach to the key. The types in this module **are**
//! the specification of those files; every struct rejects unknown keys, so a typo fails at load.
//!
//! # File layout
//!
//! One file per `Settings` section (`input.toml`, `output.toml`, …) plus `assists.toml`. The
//! file name is the section; `key` is relative to it, so `key = "dsd.gain_db"` in `input.toml`
//! has the absolute path `input.dsd.gain_db`.
//!
//! ```toml
//! [[field]]
//! key = "dsd"                        # a group: no value of its own, hides its children
//! kind = "group"
//! summary = "DSD input processing."
//! show_if = { key = "input.allow_dsd", eq = true }
//!
//! [[field]]
//! key = "dsd.gain_db"                # a leaf; its parent group must exist
//! summary = "Level alignment applied after DSD decimation."
//! description = "Paragraphs of prose."
//! default = 6.0                      # or `default_text` when the default is computed
//! range = [-30.0, 30.0]
//! show_if = { key = "input.allow_dsd", eq = true }
//! required_if = { key = "output.backend", eq = "alsa" }   # required when this holds
//! required_for = ["run"]             # required when running one of these commands
//! applies_to = ["run", "convert"]    # commands the key affects (informational / filtering)
//! assist = ["alsa-detect"]           # ids from assists.toml
//! recommended.interactive-desktop = { value = 6.0, why = "keeps DSD and PCM level-matched." }
//!
//! [[field.options]]                  # enum-like keys only: one entry per allowed value
//! key = "alsa"                     # numbers and booleans are written as their text: "7", "true"
//! label = "Exclusive ALSA"
//! summary = "One line."
//! description = "Longer text."
//! caution = "Shown as a warning while this option is selected."
//! ```
//!
//! Conditions are structured objects (see [`Condition`]); keys are absolute paths and evaluate
//! against *resolved* values (unset keys resolve to their default).

mod condition;

pub use condition::{Condition, Resolver};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Absolute dotted config path, e.g. `output.backend`.
pub type KeyPath = String;

/// Typical deployment, used to pick recommendations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UseCase {
    InteractiveDesktop,
    HeadlessAppliance,
    OfflineConversion,
}

/// A CLI command that reads the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Command {
    Run,
    Play,
    Convert,
}

/// What an assist does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistKind {
    /// Reads system state (devices, sinks).
    Probe,
    /// Runs DSP load on this machine.
    Benchmark,
    /// Compares configuration with probe results without load.
    Check,
    /// Offers named bundles of values (`Assist::presets`); nothing is probed or run.
    Preset,
}

/// Whether an entry carries a value or only groups other entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    #[default]
    Value,
    /// Heading with no value of its own. Its `show_if` hides all children.
    Group,
}

/// A recommended value for one use case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recommendation {
    pub value: Value,
    pub why: String,
}

/// Text for one allowed value of an enum-like key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionMeta {
    /// The value as written in the config file.
    pub key: String,
    pub label: String,
    pub summary: String,
    #[serde(default)]
    pub description: String,
    /// Warning shown while this option is selected.
    #[serde(default)]
    pub caution: Option<String>,
    /// Hides this option when false.
    #[serde(default)]
    pub show_if: Option<Condition>,
}

/// Metadata for one configuration key or group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Key relative to the section (the file name).
    pub key: String,
    /// Absolute path; filled in by the loader.
    #[serde(skip_deserializing)]
    pub path: KeyPath,
    #[serde(default)]
    pub kind: FieldKind,
    /// One line: TUI label, web heading.
    pub summary: String,
    #[serde(default)]
    pub description: String,
    /// Default value when it is a plain value.
    #[serde(default)]
    pub default: Option<Value>,
    /// Human description of the default when it is computed or context-dependent.
    #[serde(default)]
    pub default_text: Option<String>,
    /// Inclusive numeric bounds.
    #[serde(default)]
    pub range: Option<[f64; 2]>,
    /// Hidden when this evaluates to false (omitted = always shown).
    #[serde(default)]
    pub show_if: Option<Condition>,
    /// Required when this evaluates to true.
    #[serde(default)]
    pub required_if: Option<Condition>,
    /// Required when running one of these commands.
    #[serde(default)]
    pub required_for: Vec<Command>,
    /// Commands this key affects.
    #[serde(default)]
    pub applies_to: Vec<Command>,
    /// Assist ids from `assists.toml`.
    #[serde(default)]
    pub assist: Vec<String>,
    #[serde(default)]
    pub recommended: BTreeMap<UseCase, Recommendation>,
    /// Allowed values for enum-like keys.
    #[serde(default)]
    pub options: Vec<OptionMeta>,
}

/// An assist: a probe, benchmark or check a UI can offer on a key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assist {
    pub id: String,
    pub kind: AssistKind,
    pub label: String,
    /// Absolute paths a result may propose values for.
    #[serde(default)]
    pub fills: Vec<KeyPath>,
    /// Named value bundles (`kind = "preset"` only). Each bundle only sets keys listed in `fills`;
    /// the bundle name is never stored in the configuration.
    #[serde(default)]
    pub presets: Vec<AssistPreset>,
}

/// One named bundle of values offered by a `preset` assist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistPreset {
    pub key: String,
    pub label: String,
    pub summary: String,
    /// Absolute path -> value to set.
    pub values: BTreeMap<KeyPath, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionFile {
    #[serde(default)]
    section: Option<SectionMeta>,
    #[serde(default)]
    field: Vec<Field>,
}

/// Title and prose for a whole section (`[section]` table at the top of a section file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionMeta {
    /// Section name (the file name), filled in by the loader.
    #[serde(skip_deserializing)]
    pub name: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub description: String,
    /// Commands the section affects; empty means all.
    #[serde(default)]
    pub applies_to: Vec<Command>,
    /// Rarely needs changing; a UI may fold it away by default.
    #[serde(default)]
    pub advanced: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssistsFile {
    #[serde(default)]
    assist: Vec<Assist>,
}

/// Metadata loading or validation failure.
#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("{file}: {source}")]
    Parse {
        file: String,
        source: toml::de::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

/// The text an option `key` is compared against: strings as is, integers and booleans by their
/// decimal / `true`/`false` text, so numeric enums (`modulator_order` 5 or 7) can list options.
fn option_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => n.as_i64().map(|i| i.to_string()).or_else(|| {
            n.as_f64().map(|f| {
                if f.fract() == 0.0 {
                    format!("{}", f as i64)
                } else {
                    f.to_string()
                }
            })
        }),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

const SECTION_SOURCES: &[(&str, &str)] = &[
    ("input", include_str!("../../meta/input.toml")),
    ("output", include_str!("../../meta/output.toml")),
    ("target", include_str!("../../meta/target.toml")),
    ("gain", include_str!("../../meta/gain.toml")),
    ("resampler", include_str!("../../meta/resampler.toml")),
    ("offline", include_str!("../../meta/offline.toml")),
    ("runtime", include_str!("../../meta/runtime.toml")),
    ("logging", include_str!("../../meta/logging.toml")),
];
const ASSISTS_SOURCE: &str = include_str!("../../meta/assists.toml");

/// All metadata, validated.
#[derive(Debug, Clone)]
pub struct Catalog {
    fields: Vec<Field>,
    index: BTreeMap<KeyPath, usize>,
    section_meta: BTreeMap<String, SectionMeta>,
    /// Section names in the canonical UI order (the order of `SECTION_SOURCES`).
    section_order: Vec<String>,
    assists: BTreeMap<String, Assist>,
}

fn invalid<T>(msg: String) -> Result<T, MetaError> {
    Err(MetaError::Invalid(msg))
}

impl Catalog {
    /// Loads the embedded metadata.
    pub fn load() -> Result<Self, MetaError> {
        Self::from_sources(SECTION_SOURCES, ASSISTS_SOURCE)
    }

    /// Builds a catalog from `(section, toml)` pairs and the assists file.
    pub fn from_sources(sections: &[(&str, &str)], assists: &str) -> Result<Self, MetaError> {
        let mut fields = Vec::new();
        let mut section_meta = BTreeMap::new();
        let mut section_order = Vec::new();
        for (section, src) in sections {
            section_order.push((*section).to_string());
            let file: SectionFile = toml::from_str(src).map_err(|source| MetaError::Parse {
                file: format!("{section}.toml"),
                source,
            })?;
            if let Some(mut m) = file.section {
                m.name = (*section).to_string();
                if m.title.trim().is_empty() || m.summary.trim().is_empty() {
                    return invalid(format!(
                        "`{section}`: section title and summary must not be empty"
                    ));
                }
                section_meta.insert(m.name.clone(), m);
            }
            for mut f in file.field {
                f.path = format!("{section}.{}", f.key);
                fields.push(f);
            }
        }
        let assists_file: AssistsFile =
            toml::from_str(assists).map_err(|source| MetaError::Parse {
                file: "assists.toml".to_string(),
                source,
            })?;
        let mut assist_map = BTreeMap::new();
        for a in assists_file.assist {
            let id = a.id.clone();
            if assist_map.insert(id.clone(), a).is_some() {
                return invalid(format!("assists.toml: duplicate assist id `{id}`"));
            }
        }
        let mut index = BTreeMap::new();
        for (i, f) in fields.iter().enumerate() {
            if index.insert(f.path.clone(), i).is_some() {
                return invalid(format!("duplicate field `{}`", f.path));
            }
        }
        let catalog = Self {
            fields,
            index,
            section_meta,
            section_order,
            assists: assist_map,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// All entries in file order.
    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Entry for an absolute path.
    pub fn field(&self, path: &str) -> Option<&Field> {
        self.index.get(path).map(|&i| &self.fields[i])
    }

    /// Assist by id.
    pub fn assist(&self, id: &str) -> Option<&Assist> {
        self.assists.get(id)
    }

    /// All assists.
    pub fn assists(&self) -> impl Iterator<Item = &Assist> {
        self.assists.values()
    }

    /// Title and prose of a section, if its file has a `[section]` table.
    pub fn section(&self, name: &str) -> Option<&SectionMeta> {
        self.section_meta.get(name)
    }

    /// Loaded sections in canonical UI order: the order of `SECTION_SOURCES` (signal path first,
    /// advanced sections last). Frontends use this order as is and do not sort.
    pub fn sections(&self) -> Vec<&str> {
        self.section_order.iter().map(String::as_str).collect()
    }

    /// Direct children of a group (or the section root when `parent` is a bare section name).
    pub fn children<'a>(&'a self, parent: &'a str) -> impl Iterator<Item = &'a Field> + 'a {
        self.fields
            .iter()
            .filter(move |f| f.path.rsplit_once('.').is_some_and(|(p, _)| p == parent))
    }

    /// Whether `path` is shown: its own `show_if` and those of all ancestor groups must hold.
    pub fn is_visible(&self, path: &str, r: &dyn Resolver) -> bool {
        let Some(field) = self.field(path) else {
            return false;
        };
        if field.show_if.as_ref().is_some_and(|e| !e.eval(r)) {
            return false;
        }
        match path.rsplit_once('.') {
            Some((parent, _)) if self.field(parent).is_some() => self.is_visible(parent, r),
            _ => true,
        }
    }

    /// Whether `path` must be set for `command` given the current values. Hidden keys are never
    /// required.
    pub fn is_required(&self, path: &str, command: Command, r: &dyn Resolver) -> bool {
        let Some(field) = self.field(path) else {
            return false;
        };
        if !self.is_visible(path, r) {
            return false;
        }
        field.required_for.contains(&command)
            || field.required_if.as_ref().is_some_and(|e| e.eval(r))
    }

    fn known_section(&self, path: &str) -> bool {
        path.split('.')
            .next()
            .is_some_and(|s| self.section_order.iter().any(|n| n == s))
    }

    /// Structural checks; run by every constructor.
    fn validate(&self) -> Result<(), MetaError> {
        for f in &self.fields {
            let p = &f.path;
            if f.key.is_empty()
                || !f
                    .key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.'))
            {
                return invalid(format!("`{p}`: key must be lowercase [a-z0-9_.]"));
            }
            if f.summary.trim().is_empty() {
                return invalid(format!("`{p}`: summary must not be empty"));
            }
            // Parent chain: every intermediate prefix is an existing group.
            let mut acc = String::new();
            let parts: Vec<&str> = p.split('.').collect();
            for (n, part) in parts.iter().enumerate().take(parts.len() - 1) {
                if n > 0 {
                    acc.push('.');
                }
                acc.push_str(part);
                if n == 0 {
                    continue; // the section itself
                }
                match self.field(&acc) {
                    Some(parent) if parent.kind == FieldKind::Group => {}
                    Some(_) => return invalid(format!("`{p}`: parent `{acc}` is not a group")),
                    None => return invalid(format!("`{p}`: missing parent group `{acc}`")),
                }
            }

            if f.kind == FieldKind::Group
                && (f.default.is_some()
                    || f.default_text.is_some()
                    || f.range.is_some()
                    || !f.options.is_empty()
                    || !f.recommended.is_empty()
                    || !f.assist.is_empty()
                    || !f.required_for.is_empty()
                    || f.required_if.is_some())
            {
                return invalid(format!(
                    "`{p}`: a group may only have summary, description and show_if"
                ));
            }
            if f.default.is_some() && f.default_text.is_some() {
                return invalid(format!(
                    "`{p}`: set either default or default_text, not both"
                ));
            }
            if let Some([lo, hi]) = f.range {
                if lo > hi {
                    return invalid(format!("`{p}`: range min exceeds max"));
                }
            }
            for id in &f.assist {
                if !self.assists.contains_key(id) {
                    return invalid(format!("`{p}`: unknown assist `{id}`"));
                }
            }

            // Options: unique keys; default and recommendations must name an option.
            let option_keys: BTreeSet<&str> = f.options.iter().map(|o| o.key.as_str()).collect();
            if option_keys.len() != f.options.len() {
                return invalid(format!("`{p}`: duplicate option key"));
            }
            for o in &f.options {
                if o.label.trim().is_empty() || o.summary.trim().is_empty() {
                    return invalid(format!(
                        "`{p}` option `{}`: label and summary required",
                        o.key
                    ));
                }
            }
            if !option_keys.is_empty() {
                let check = |what: &str, v: &Value| -> Result<(), MetaError> {
                    match option_text(v) {
                        Some(s) if option_keys.contains(s.as_str()) => Ok(()),
                        _ => invalid(format!("`{p}`: {what} {v} is not one of the options")),
                    }
                };
                if let Some(d) = &f.default {
                    check("default", d)?;
                }
                for (uc, r) in &f.recommended {
                    check(&format!("recommended.{uc:?}"), &r.value)?;
                }
            }
            for (uc, r) in &f.recommended {
                if r.why.trim().is_empty() {
                    return invalid(format!("`{p}`: recommended.{uc:?} needs a reason"));
                }
            }
        }

        for a in self.assists.values() {
            if a.label.trim().is_empty() {
                return invalid(format!("assist `{}`: label must not be empty", a.id));
            }
            for path in &a.fills {
                if self.known_section(path) && self.field(path).is_none() {
                    return invalid(format!("assist `{}` fills unknown path `{path}`", a.id));
                }
            }
            if a.kind != AssistKind::Preset && !a.presets.is_empty() {
                return invalid(format!(
                    "assist `{}`: only `preset` assists carry presets",
                    a.id
                ));
            }
            if a.kind == AssistKind::Preset && a.presets.is_empty() {
                return invalid(format!(
                    "assist `{}`: a `preset` assist needs presets",
                    a.id
                ));
            }
            let mut keys = BTreeSet::new();
            for pr in &a.presets {
                if !keys.insert(pr.key.as_str()) {
                    return invalid(format!("assist `{}`: duplicate preset `{}`", a.id, pr.key));
                }
                if pr.label.trim().is_empty() || pr.summary.trim().is_empty() {
                    return invalid(format!(
                        "assist `{}` preset `{}`: label and summary must not be empty",
                        a.id, pr.key
                    ));
                }
                if pr.values.keys().cloned().collect::<BTreeSet<_>>()
                    != a.fills.iter().cloned().collect::<BTreeSet<_>>()
                {
                    return invalid(format!(
                        "assist `{}` preset `{}`: values must set exactly the `fills` keys",
                        a.id, pr.key
                    ));
                }
                for (path, v) in &pr.values {
                    let Some(f) = self.field(path) else { continue };
                    let lit = option_text(v).unwrap_or_else(|| v.to_string());
                    if !f.options.is_empty() && !f.options.iter().any(|o| o.key == lit) {
                        return invalid(format!(
                            "assist `{}` preset `{}`: `{path}` = {lit} is not an option",
                            a.id, pr.key
                        ));
                    }
                }
            }
        }

        // Condition keys must name real values; compared literals must be real options.
        let conditions = self.fields.iter().flat_map(|f| {
            f.show_if
                .iter()
                .chain(f.required_if.iter())
                .chain(f.options.iter().filter_map(|o| o.show_if.as_ref()))
                .map(move |e| (f.path.as_str(), e))
        });
        for (owner, e) in conditions {
            for (path, lits) in e.comparisons() {
                if !self.known_section(path) {
                    continue; // section metadata not written yet
                }
                let Some(target) = self.field(path) else {
                    return invalid(format!(
                        "`{owner}`: condition references unknown path `{path}`"
                    ));
                };
                if target.kind == FieldKind::Group {
                    return invalid(format!("`{owner}`: `{path}` is a group, not a value"));
                }
                if !target.options.is_empty() {
                    for lit in lits {
                        let ok = option_text(lit)
                            .is_some_and(|s| target.options.iter().any(|o| o.key == s));
                        if !ok {
                            return invalid(format!(
                                "`{owner}`: {lit} is not an option of `{path}`"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InputConfig, OutputConfig, TargetConfig};
    use serde_json::json;

    fn catalog() -> Catalog {
        Catalog::load().expect("embedded metadata must load")
    }

    fn env(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// A config setting every key of the sections that have metadata. Parsing it into the real
    /// config types (`deny_unknown_fields`) proves each key exists; the leaf set is compared with
    /// the catalog in both directions.
    /// TODO(Session 2): replace with the generated JSON Schema so new `Settings` keys are caught.
    const MAXIMAL: &str = r#"
[input]
backend = "pipewire"
node_name = "audigma_sink"
sample_format = "s32le"
rate_hz = 48000
channels = 2
channel_positions = ["FL", "FR"]
rate_mode = "dynamic"
allowed_rates = [44100, 48000]
allow_dsd = true

[input.dsd]
max_intermediate_rate_hz = 192000
gain_db = 6.0

[output]
backend = "alsa"
target_node = "x"
device = "hw:CARD=K17,DEV=0"
transport = "native"
container = "dsf"
dop_sample_format = "s32le"
reserve_device = true
buffer_ms = 40
period_ms = 5
volume_control = "software"
mixer_control = "PCM"
device_wait_ms = -1
keep_audio_sink_ms = 5000

[target]
format = "pcm"
rate_family = "source"
pcm_rate_hz = 192000
dsd_rate = "dsd256"
sample_format = "s32le"
dither = "tpdf"
dither_tpdf_seed = 12345

[gain]
preamp_db = -3.0
peak_analysis = "single-pass"
overload_policy = "mute-recover"
dsd_safety_margin_db = 0.25

[resampler]
phase = "linear"
apodization = "off"
modulator_order = 5
drift_correction = "auto"
max_clock_drift_ppm = 500

[offline]
tail_policy = "trim-to-duration"
write_report = true
dsf_pad_mode = "spec-zero"

[runtime]
dsp_threads = 2
realtime = "auto"
realtime_priority = 20
core_selection = "performance"
smt_siblings = "avoid"
loop_cpu = 1
dsp_cpus = [2, 3]

[logging]
format = "text"
level = "info"
late_tile_report_interval_ms = 10000
"#;

    fn leaves(prefix: &str, v: &toml::Value, out: &mut BTreeSet<String>) {
        match v {
            toml::Value::Table(t) => {
                for (k, v) in t {
                    let p = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    leaves(&p, v, out);
                }
            }
            _ => {
                out.insert(prefix.to_string());
            }
        }
    }

    #[test]
    fn embedded_metadata_loads_and_validates() {
        let c = catalog();
        for section in [
            "input",
            "output",
            "target",
            "gain",
            "resampler",
            "offline",
            "runtime",
            "logging",
        ] {
            assert!(c.sections().contains(&section), "{section}");
            let m = c
                .section(section)
                .unwrap_or_else(|| panic!("{section}: no [section]"));
            assert!(!m.title.is_empty() && !m.summary.is_empty() && !m.description.is_empty());
        }
        assert_eq!(
            c.sections(),
            [
                "input",
                "output",
                "target",
                "gain",
                "resampler",
                "offline",
                "runtime",
                "logging"
            ]
        );
        // Every non-advanced section comes before every advanced one.
        let adv: Vec<bool> = c
            .sections()
            .iter()
            .map(|n| c.section(n).unwrap().advanced)
            .collect();
        assert!(
            adv.windows(2).all(|w| !w[0] || w[1]),
            "advanced sections must come last"
        );
        assert!(c.section("offline").unwrap().advanced);
        assert!(!c.section("target").unwrap().advanced);
        assert!(c.assist("alsa-detect").is_some());
        let q = c.assist("quality-starting-point").unwrap();
        let names: Vec<_> = q.presets.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(names, ["neutral", "apodizing", "low-latency"]);
    }

    #[test]
    fn catalog_matches_settings_keys() {
        let value: toml::Value = toml::from_str(MAXIMAL).unwrap();
        let table = value.as_table().unwrap();
        // Every key exists in the real types.
        table["input"].clone().try_into::<InputConfig>().unwrap();
        table["output"].clone().try_into::<OutputConfig>().unwrap();
        table["target"].clone().try_into::<TargetConfig>().unwrap();
        table["gain"]
            .clone()
            .try_into::<crate::GainConfig>()
            .unwrap();
        table["resampler"]
            .clone()
            .try_into::<crate::ResamplerConfig>()
            .unwrap();
        table["offline"]
            .clone()
            .try_into::<crate::OfflineConfig>()
            .unwrap();
        table["runtime"]
            .clone()
            .try_into::<crate::RuntimeConfig>()
            .unwrap();
        table["logging"]
            .clone()
            .try_into::<crate::LoggingConfig>()
            .unwrap();

        let mut config_leaves = BTreeSet::new();
        leaves("", &value, &mut config_leaves);
        let catalog_leaves: BTreeSet<String> = catalog()
            .fields()
            .iter()
            .filter(|f| f.kind == FieldKind::Value)
            .map(|f| f.path.clone())
            .collect();
        assert_eq!(
            config_leaves, catalog_leaves,
            "metadata entries and config keys must match exactly"
        );
    }

    #[test]
    fn defaults_in_metadata_match_code() {
        let c = catalog();
        let d = |p: &str| c.field(p).unwrap().default.clone();
        assert_eq!(d("input.channels"), Some(json!(2)));
        assert_eq!(d("input.rate_mode"), Some(json!("dynamic")));
        assert_eq!(d("input.allow_dsd"), Some(json!(false)));
        assert_eq!(d("input.dsd.gain_db"), Some(json!(6.0)));
        assert_eq!(d("output.buffer_ms"), Some(json!(40)));
        assert_eq!(d("output.period_ms"), Some(json!(5)));
        assert_eq!(d("output.device_wait_ms"), Some(json!(-1)));
        assert_eq!(d("output.keep_audio_sink_ms"), Some(json!(5000)));
        assert_eq!(
            d("output.volume_control"),
            Some(json!("hardware-software-fallback"))
        );
        assert_eq!(d("target.rate_family"), Some(json!("source")));
        assert_eq!(d("target.sample_format"), Some(json!("s32le")));
        assert_eq!(d("target.dither"), Some(json!("tpdf")));
        assert_eq!(d("target.dither_tpdf_seed"), Some(json!(12345)));
        assert_eq!(
            d("input.allowed_rates"),
            Some(json!(InputConfig::default_allowed_rates()))
        );
    }

    #[test]
    fn dsd_group_hides_children_unless_allowed() {
        let c = catalog();
        let off = env(&[("input.allow_dsd", json!(false))]);
        let on = env(&[("input.allow_dsd", json!(true))]);
        assert!(!c.is_visible("input.dsd", &off));
        assert!(!c.is_visible("input.dsd.gain_db", &off));
        assert!(c.is_visible("input.dsd.gain_db", &on));
    }

    #[test]
    fn visibility_uses_resolved_values() {
        let c = catalog();
        assert!(c.is_visible(
            "input.allowed_rates",
            &env(&[("input.rate_mode", json!("dynamic"))])
        ));
        assert!(!c.is_visible(
            "input.allowed_rates",
            &env(&[("input.rate_mode", json!("fixed"))])
        ));
        let alsa = env(&[("output.backend", json!("alsa"))]);
        let pw = env(&[("output.backend", json!("pipewire"))]);
        assert!(c.is_visible("output.device", &alsa));
        assert!(!c.is_visible("output.device", &pw));
        assert!(c.is_visible("output.target_node", &pw));
    }

    #[test]
    fn target_rate_keys_follow_format() {
        let c = catalog();
        let pcm = env(&[("target.format", json!("pcm"))]);
        let dsd = env(&[("target.format", json!("dsd"))]);
        assert!(c.is_visible("target.pcm_rate_hz", &pcm));
        assert!(!c.is_visible("target.dsd_rate", &pcm));
        assert!(c.is_required("target.dsd_rate", Command::Run, &dsd));
        assert!(!c.is_required("target.pcm_rate_hz", Command::Run, &dsd));
        assert!(c.is_required("target.format", Command::Convert, &pcm));
        // dither seed hides when dither is off; unset dither counts as the default (tpdf)
        let off = env(&[("target.dither", json!("none"))]);
        assert!(!c.is_visible("target.dither_tpdf_seed", &off));
        assert!(c.is_visible(
            "target.dither_tpdf_seed",
            &env(&[("target.dither", json!("tpdf"))])
        ));
    }

    #[test]
    fn required_rules() {
        let c = catalog();
        let alsa = env(&[("output.backend", json!("alsa"))]);
        let pw = env(&[("output.backend", json!("pipewire"))]);
        assert!(c.is_required("output.device", Command::Run, &alsa));
        assert!(!c.is_required("output.device", Command::Run, &pw));
        assert!(c.is_required("output.backend", Command::Run, &pw));
        assert!(!c.is_required("output.backend", Command::Convert, &pw));
    }

    #[test]
    fn recommendations_reference_valid_options() {
        let c = catalog();
        let b = c.field("output.backend").unwrap();
        assert_eq!(
            b.recommended[&UseCase::InteractiveDesktop].value,
            json!("alsa")
        );
        assert_eq!(b.options.len(), 2);
        assert!(
            b.options
                .iter()
                .any(|o| o.key == "pipewire" && o.caution.is_some())
        );
    }

    fn load_err(section: &str, assists: &str) -> String {
        Catalog::from_sources(&[("input", section)], assists)
            .unwrap_err()
            .to_string()
    }

    const NO_ASSISTS: &str = "";

    #[test]
    fn rejects_structural_mistakes() {
        let e = |s: &str| load_err(s, NO_ASSISTS);
        // unknown TOML key
        assert!(e("[[field]]\nkey='a'\nsummary='s'\nbogus=1").contains("bogus"));
        // empty summary
        assert!(e("[[field]]\nkey='a'\nsummary=' '").contains("summary"));
        // duplicate
        assert!(
            e("[[field]]\nkey='a'\nsummary='s'\n[[field]]\nkey='a'\nsummary='s'")
                .contains("duplicate field")
        );
        // missing parent group
        assert!(e("[[field]]\nkey='g.a'\nsummary='s'").contains("missing parent group"));
        // unknown assist
        assert!(e("[[field]]\nkey='a'\nsummary='s'\nassist=['nope']").contains("unknown assist"));
        // malformed condition (a string, and two operators at once)
        assert!(e("[[field]]\nkey='a'\nsummary='s'\nshow_if='a == 1'").contains("show_if"));
        assert!(
            e("[[field]]\nkey='a'\nsummary='s'\nshow_if={key='a',eq=1,neq=2}")
                .contains("exactly one")
        );
        // reference to unknown path in a loaded section
        assert!(
            e("[[field]]\nkey='a'\nsummary='s'\nshow_if={key='input.zzz',eq='x'}")
                .contains("unknown path")
        );
        // default not an option
        assert!(e("[[field]]\nkey='a'\nsummary='s'\ndefault='x'\n[[field.options]]\nkey='y'\nlabel='Y'\nsummary='y'")
            .contains("not one of the options"));
        // numeric option keys match numbers by their text
        assert!(
            Catalog::from_sources(
                &[(
                    "input",
                    "[[field]]\nkey='a'\nsummary='s'\ndefault=5\n[[field.options]]\nkey='5'\nlabel='Five'\nsummary='5'\n[[field.options]]\nkey='7'\nlabel='Seven'\nsummary='7'"
                )],
                NO_ASSISTS
            )
            .is_ok()
        );
        // a number that is not an option is still rejected
        assert!(e("[[field]]\nkey='a'\nsummary='s'\ndefault=6\n[[field.options]]\nkey='5'\nlabel='Five'\nsummary='5'")
            .contains("not one of the options"));
        // literal not an option
        assert!(e("[[field]]\nkey='a'\nsummary='s'\n[[field.options]]\nkey='y'\nlabel='Y'\nsummary='y'\n[[field]]\nkey='b'\nsummary='s'\nshow_if={key='input.a',eq='z'}")
            .contains("not an option"));
        // default and default_text together
        assert!(
            e("[[field]]\nkey='a'\nsummary='s'\ndefault=1\ndefault_text='x'").contains("not both")
        );
        // group with a value
        assert!(e("[[field]]\nkey='g'\nkind='group'\nsummary='s'\ndefault=1").contains("group"));
    }

    #[test]
    fn rejects_assist_fills_to_unknown_path() {
        let assists = "[[assist]]\nid='x'\nkind='probe'\nlabel='X'\nfills=['input.zzz']";
        assert!(
            load_err("[[field]]\nkey='a'\nsummary='s'", assists).contains("fills unknown path")
        );
        // paths in sections without metadata yet are tolerated
        let ok = "[[assist]]\nid='x'\nkind='probe'\nlabel='X'\nfills=['target.zzz']";
        assert!(Catalog::from_sources(&[("input", "[[field]]\nkey='a'\nsummary='s'")], ok).is_ok());
    }
}
