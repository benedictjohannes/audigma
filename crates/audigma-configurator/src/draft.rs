//! The configuration being edited: a [`Settings`] plus the set of keys the user touched.
//!
//! The writer needs both: the values to write, and which keys to leave alone when it updates an
//! existing file in place (a key the user never touched keeps its bytes, even if the draft
//! would render it differently).

use std::collections::BTreeSet;

use audigma_settings::{Settings, SettingsError};

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

/// Settings in progress with per-key "touched" tracking.
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

    /// A draft from a config file's text; nothing touched.
    pub fn from_toml_str(text: &str) -> Result<Self, DraftError> {
        Ok(Self::new(Settings::from_toml_str(text)?))
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
        self.settings = Settings::from_toml_table(table)?;
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
    fn invalid_edit_leaves_the_draft_unchanged() {
        let mut d = base();
        let before = d.clone();
        assert!(d.set("runtime.realtime_priority", &json!(1)).is_err());
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
