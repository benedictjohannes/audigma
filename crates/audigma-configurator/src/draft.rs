//! The configuration being edited: a [`Settings`] plus the set of keys the user touched.
//!
//! The writer needs both: the values to write, and which keys to leave alone when it updates an
//! existing file in place (a key the user never touched keeps its bytes, even if the draft
//! would render it differently).

use std::collections::BTreeSet;

use std::path::{Path, PathBuf};

use audigma_settings::{Settings, SettingsError};
use schemars::JsonSchema;
use serde::Serialize;

/// Failure while changing a draft.
#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("`{0}`: this value cannot be written to a config file")]
    Unrepresentable(String),
    #[error("`{0}` is not a key of a section")]
    BadPath(String),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error("draft cannot be serialized: {0}")]
    Serialize(String),
}

/// A finding about the draft, attributed to a key when the settings crate knows which one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Issue {
    /// Absolute dotted path, `None` for a finding that belongs to no single key.
    pub path: Option<String>,
    pub message: String,
    /// An error blocks saving; a warning does not.
    pub error: bool,
}

/// Failure while loading the file a draft starts from.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

/// Settings in progress with per-key "touched" tracking.
///
/// Edits only have to keep the draft *well-formed* (known keys, right types and enum values);
/// the cross-field rules are checked by [`Draft::issues`], so a form can pass through states
/// like `backend = "alsa"` before `device` is set.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    settings: Settings,
    touched: BTreeSet<String>,
}

impl Draft {
    /// A draft with nothing touched (e.g. just loaded from a file).
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            touched: BTreeSet::new(),
        }
    }

    /// A draft with no sections set (a new file).
    pub fn empty() -> Self {
        let settings = Settings::from_toml_str_unvalidated(&format!(
            "schema_version = {}",
            audigma_settings::SCHEMA_VERSION_V1
        ))
        .expect("a bare schema_version parses");
        Self::new(settings)
    }

    /// A draft from a config file's text, which must be valid as a whole; nothing touched.
    /// Parses `text`, the contents of `path`, into a draft; the error names the file and the
    /// field (or the TOML line and column).
    pub fn from_file_text(path: &Path, text: &str) -> Result<Self, LoadError> {
        Self::from_toml_str(text).map_err(|e| LoadError::Invalid {
            path: path.to_path_buf(),
            message: match &e {
                DraftError::Settings(SettingsError::TomlParse(t)) => {
                    t.to_string().trim_end().to_string()
                }
                other => other.to_string(),
            },
        })
    }

    /// Reads `path` into a draft. A missing file is a new one: `(empty draft, false)`.
    pub fn load(path: &Path) -> Result<(Self, bool), LoadError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok((Self::from_file_text(path, &text)?, true)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::empty(), false)),
            Err(source) => Err(LoadError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn from_toml_str(text: &str) -> Result<Self, DraftError> {
        Ok(Self::new(Settings::from_toml_str(text)?))
    }

    /// Everything wrong with the draft right now: the first violated rule (an error), or, when
    /// the draft is valid, the warnings. Save is allowed when no entry is an error.
    pub fn issues(&self) -> Vec<Issue> {
        match self.settings.validate() {
            Err(e) => vec![Issue {
                path: e.path().map(str::to_string),
                message: match &e {
                    SettingsError::Validation { message, .. } => (*message).to_string(),
                    other => other.to_string(),
                },
                error: true,
            }],
            Ok(()) => self
                .settings
                .warnings()
                .into_iter()
                .map(|w| Issue {
                    path: Some(w.path.to_string()),
                    message: w.message,
                    error: false,
                })
                .collect(),
        }
    }

    /// Whether [`Draft::issues`] contains an error.
    pub fn has_errors(&self) -> bool {
        self.settings.validate().is_err()
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Dotted paths the user changed since the draft was created.
    pub fn touched(&self) -> &BTreeSet<String> {
        &self.touched
    }

    pub fn is_touched(&self, path: &str) -> bool {
        self.touched.contains(path)
    }

    /// Marks `path` as touched without changing its value (e.g. "write this key explicitly").
    pub fn touch(&mut self, path: &str) {
        self.touched.insert(path.to_string());
    }

    /// The values the draft carries, as a TOML table (keys that are `None` are absent).
    pub fn values(&self) -> Result<toml::Table, DraftError> {
        toml::Table::try_from(&self.settings).map_err(|e| DraftError::Serialize(e.to_string()))
    }

    /// Sets `path` (e.g. `output.backend`) to `value`, validates the whole draft, and marks the
    /// key touched. On error the draft is unchanged.
    pub fn set(&mut self, path: &str, value: &serde_json::Value) -> Result<(), DraftError> {
        let v = toml::Value::try_from(value)
            .map_err(|_| DraftError::Unrepresentable(path.to_string()))?;
        self.edit(path, Some(v))
    }

    /// Removes `path` from the draft (back to unset) and marks it touched.
    pub fn unset(&mut self, path: &str) -> Result<(), DraftError> {
        self.edit(path, None)
    }

    fn edit(&mut self, path: &str, value: Option<toml::Value>) -> Result<(), DraftError> {
        let mut table = self.values()?;
        let parts: Vec<&str> = path.split('.').collect();
        let Some((leaf, parents)) = parts.split_last().filter(|_| parts.len() >= 2) else {
            return Err(DraftError::BadPath(path.to_string()));
        };
        let mut cur = &mut table;
        for p in parents {
            let slot = cur
                .entry(p.to_string())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            cur = slot
                .as_table_mut()
                .ok_or_else(|| DraftError::BadPath(path.to_string()))?;
        }
        match value {
            Some(v) => {
                cur.insert(leaf.to_string(), v);
            }
            None => {
                cur.remove(*leaf);
            }
        }
        self.settings = Settings::from_toml_table_unvalidated(table)?;
        self.touched.insert(path.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Draft {
        Draft::from_toml_str(
            "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n",
        )
        .unwrap()
    }

    #[test]
    fn set_and_unset_track_touched_keys() {
        let mut d = base();
        assert!(d.touched().is_empty());
        d.set("resampler.max_clock_drift_ppm", &json!(300)).unwrap();
        let drift = |d: &Draft| d.settings().resampler.as_ref().unwrap().max_clock_drift_ppm;
        assert_eq!(drift(&d), Some(300));
        assert!(d.is_touched("resampler.max_clock_drift_ppm"));
        d.unset("resampler.max_clock_drift_ppm").unwrap();
        assert_eq!(drift(&d), None);
    }

    #[test]
    fn edits_may_pass_through_invalid_states() {
        let mut d = Draft::empty();
        assert_eq!(d.issues()[0].path.as_deref(), Some("target"));
        d.set("target.format", &json!("pcm")).unwrap();
        d.set("target.pcm_rate_hz", &json!(192000)).unwrap();
        assert!(d.issues().is_empty());
        d.set("output.backend", &json!("alsa")).unwrap();
        let issues = d.issues();
        assert_eq!(issues.len(), 1);
        assert!(issues[0].error);
        assert_eq!(issues[0].path.as_deref(), Some("output.device"));
        assert!(d.has_errors());
        d.set("output.device", &json!("hw:CARD=K17,DEV=0")).unwrap();
        assert!(!d.has_errors());
    }

    #[test]
    fn a_malformed_edit_leaves_the_draft_unchanged() {
        let mut d = base();
        let before = d.clone();
        assert!(d.set("output.backend", &json!("nonsense")).is_err());
        assert_eq!(d, before);
    }

    #[test]
    fn rejects_paths_without_a_section() {
        let mut d = base();
        assert!(matches!(
            d.set("schema_version", &json!(1)),
            Err(DraftError::BadPath(_))
        ));
    }
}
