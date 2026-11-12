//! The state the web frontend edits: one draft, the guided flow, the last probe and the last
//! "validate on this machine" result.
//!
//! Everything here is synchronous and free of HTTP and threads, so the API layer (`api.rs`) and
//! the tests drive it directly. The server wraps one [`Session`] in a mutex: the configurator
//! has exactly one user and one file.

use std::path::{Path, PathBuf};

use audigma_settings::meta::{Catalog, ConfigValue, FieldKind, UseCase};
use schemars::JsonSchema;
use schemars::{Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assist::benchmark::{BenchmarkEvent, BenchmarkResult};
use crate::assist::capability::{self, CapabilityFinding};
use crate::assist::probe::ProbeReport;
use crate::draft::{Draft, DraftError, Issue, LoadError};
use crate::guided::{DiscoveryNext, Guided, Outcome, StepView, TierState};
use crate::writer::{CommentLevel, WriteOptions, prepare, save};

/// A "validate on this machine" result and the draft it measured.
#[derive(Debug, Clone)]
pub struct Validated {
    pub draft: Draft,
    pub result: BenchmarkResult,
}

pub struct Session {
    pub catalog: Catalog,
    pub path: PathBuf,
    /// The file existed when the session opened. Guided setup is for new files only
    /// (Decision 27).
    pub existed: bool,
    pub draft: Draft,
    pub use_case: Option<UseCase>,
    pub guided: Option<Guided>,
    /// The name of the file whose text replaced the draft (`POST /api/load`), if any.
    pub loaded_from: Option<String>,
    pub probe: Option<ProbeReport>,
    pub validated: Option<Validated>,
    /// A benchmark is running; edits that would invalidate it are refused.
    pub busy: bool,
}

/// The values a draft sets, as a nested object: section tables whose leaves are scalars and
/// whose `schema_version` is a number. Only the schema uses this type (the response carries a
/// `serde_json::Value`); a test checks real responses against its shape.
pub struct ValueTree;

impl JsonSchema for ValueTree {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ValueTree".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "additionalProperties": {
                "anyOf": [
                    generator.subschema_for::<ConfigValue>(),
                    generator.subschema_for::<ValueTree>(),
                ]
            }
        })
    }
}

/// What `GET /api/draft` returns.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DraftView {
    pub path: String,
    /// The file exists on disk right now (a save then updates it in place after a backup).
    pub exists: bool,
    /// The file did not exist when the session opened.
    pub is_new: bool,
    pub use_case: Option<UseCase>,
    pub guided_active: bool,
    /// The name of an uploaded file the draft was loaded from (`POST /api/load`); guided setup
    /// is refused after one.
    pub loaded_from: Option<String>,
    /// The values the draft sets, as a nested object (what the file would hold).
    #[schemars(with = "ValueTree")]
    pub values: Value,
    /// Dotted paths the user (or guided setup) touched.
    pub touched: Vec<String>,
    pub issues: Vec<Issue>,
    pub findings: Vec<CapabilityFinding>,
    pub validation: Option<ValidationView>,
}

/// The last "validate on this machine" result.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ValidationView {
    pub result: BenchmarkResult,
    /// The draft changed after the measurement.
    pub stale: bool,
}

/// A rate tier in the guided discovery sweep.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TierView {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub state: TierState,
}

/// The discovery sweep so far.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DiscoveryView {
    pub tiers: Vec<TierView>,
    pub progress: f64,
    pub active: bool,
    /// Quality settings changed since the sweep ran.
    pub stale: bool,
}

/// What `GET /api/guided/state` returns.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GuidedState {
    pub active: bool,
    pub step: Option<StepView>,
    pub can_go_back: bool,
    pub use_case: Option<UseCase>,
    pub discovery: DiscoveryView,
}

/// Where a frontend goes after a guided answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuidedNext {
    /// Render the new step.
    Step,
    /// The probe ran (fresh) before this step; render it.
    Probed,
    /// A benchmark sweep started; follow it on the events endpoint.
    Benchmark,
    /// Flow over, draft filled: show the review page.
    Review,
    /// Flow over (offline use case): show the form at `[offline]`.
    FormOffline,
}

/// Body of `PUT /api/draft`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftEdit {
    /// Dotted path to the new value.
    #[serde(default)]
    #[schemars(with = "std::collections::BTreeMap<String, ConfigValue>")]
    pub set: serde_json::Map<String, Value>,
    #[serde(default)]
    pub unset: Vec<String>,
    /// `Some(None)` clears the use case; absent leaves it.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<UseCase>")]
    pub use_case: Option<Option<UseCase>>,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// Body of `POST /api/save` and `POST /api/preview`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveRequest {
    /// Defaults to the path the session opened.
    pub path: Option<String>,
    /// Defaults to `true`.
    pub only_changed: Option<bool>,
    /// Defaults to `brief`.
    pub comment_level: Option<CommentLevel>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct LoadRequest {
    /// The TOML text of the file.
    pub text: String,
    /// The file's name, for error messages and `DraftView::loaded_from`.
    #[serde(default)]
    pub name: Option<String>,
}

/// What `POST /api/save` returns.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SaveResponse {
    pub path: String,
    pub created: bool,
    pub unchanged: bool,
    pub backup: Option<String>,
    /// Commands to try next, with the saved path filled in.
    pub next: Vec<String>,
}

/// What `POST /api/preview` returns: what a save with the same options would write.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PreviewResponse {
    /// The resolved target, as a save reports it.
    pub path: String,
    /// The target exists right now.
    pub exists: bool,
    /// The target's current text; null when it does not exist.
    pub existing: Option<String>,
    /// The full file text a save would write.
    pub text: String,
    /// `text` equals `existing`, so a save would write nothing.
    pub unchanged: bool,
    /// The file name of `path`, for a client-side download.
    pub filename: String,
}

/// A request the session cannot honour; `path` names the field when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ApiError {
    pub error: String,
    pub path: Option<String>,
}

impl ApiError {
    pub fn new(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            path: None,
        }
    }
}

impl Session {
    /// Opens the session on `path`; a missing file is a new one.
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        let (draft, existed) = Draft::load(path)?;
        Ok(Self::with_draft(path, draft, existed))
    }

    pub fn with_draft(path: &Path, draft: Draft, existed: bool) -> Self {
        Self {
            catalog: Catalog::load().expect("the metadata catalog is valid"),
            path: path.to_path_buf(),
            existed,
            draft,
            use_case: None,
            guided: None,
            loaded_from: None,
            probe: None,
            validated: None,
            busy: false,
        }
    }

    /// Keeps what a single probe found, so capability findings and guided steps can use it.
    pub fn probe_part(
        &mut self,
        alsa: Option<Result<audigma_io::AlsaProbeReport, String>>,
        pipewire: Option<Result<audigma_io::PipeWireProbeReport, String>>,
    ) {
        let probe = self.probe.get_or_insert_with(|| ProbeReport {
            alsa: Err("not probed yet".into()),
            pipewire: Err("not probed yet".into()),
        });
        if let Some(a) = alsa {
            probe.alsa = a;
        }
        if let Some(p) = pipewire {
            probe.pipewire = p;
        }
    }

    // ---- draft -------------------------------------------------------------------------

    pub fn findings(&self) -> Vec<CapabilityFinding> {
        let Some(Ok(alsa)) = self.probe.as_ref().map(|p| &p.alsa) else {
            return Vec::new();
        };
        if self.draft.has_errors() {
            return Vec::new();
        }
        capability::check(self.draft.settings(), alsa)
    }

    pub fn validation(&self) -> Option<ValidationView> {
        self.validated.as_ref().map(|v| ValidationView {
            result: v.result.clone(),
            stale: v.draft != self.draft,
        })
    }

    pub fn draft_view(&self) -> DraftView {
        DraftView {
            path: self.path.display().to_string(),
            exists: self.path.exists(),
            is_new: !self.existed,
            use_case: self.use_case,
            guided_active: self.guided.is_some(),
            loaded_from: self.loaded_from.clone(),
            values: self
                .draft
                .values()
                .ok()
                .and_then(|t| serde_json::to_value(t).ok())
                .unwrap_or(Value::Null),
            touched: self.draft.touched().iter().cloned().collect(),
            issues: self.draft.issues(),
            findings: self.findings(),
            validation: self.validation(),
        }
    }

    /// Applies an edit all or nothing: the first key that cannot be set restores the draft.
    pub fn edit(&mut self, edit: &DraftEdit) -> Result<(), ApiError> {
        if self.busy {
            return Err(ApiError::new(
                "a benchmark is running; cancel it before editing",
            ));
        }
        if self.guided.is_some() && (!edit.set.is_empty() || !edit.unset.is_empty()) {
            return Err(ApiError::new(
                "guided setup is running; finish or skip it before editing keys",
            ));
        }
        let before = self.draft.clone();
        for (path, value) in &edit.set {
            if let Err(e) = self.draft.set(path, value) {
                self.draft = before;
                return Err(ApiError {
                    error: e.to_string(),
                    path: Some(path.clone()),
                });
            }
        }
        for path in &edit.unset {
            if let Err(e) = self.draft.unset(path) {
                self.draft = before;
                return Err(ApiError {
                    error: draft_error_text(&e),
                    path: Some(path.clone()),
                });
            }
        }
        if let Some(u) = edit.use_case {
            self.use_case = u;
        }
        Ok(())
    }

    /// Replaces the draft with uploaded TOML text. The text must be valid as a whole (the same
    /// rule as `-c PATH`); on error nothing changes. Every key the text sets counts as touched,
    /// so a save onto an existing file writes the uploaded values, not only later edits. The
    /// save target and the use case are kept; guided setup is refused afterwards (Decision 27).
    pub fn load_text(&mut self, req: &LoadRequest) -> Result<(), ApiError> {
        if self.busy {
            return Err(ApiError::new(
                "a benchmark is running; cancel it before loading a file",
            ));
        }
        if self.guided.is_some() {
            return Err(ApiError::new(
                "guided setup is running; finish or skip it before loading a file",
            ));
        }
        let name = req
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("uploaded file");
        let mut draft = Draft::from_file_text(Path::new(name), &req.text)
            .map_err(|e| ApiError::new(e.to_string()))?;
        let values = draft
            .values()
            .map_err(|e| ApiError::new(draft_error_text(&e)))?;
        let present: Vec<String> = self
            .catalog
            .fields()
            .iter()
            .filter(|f| f.kind == FieldKind::Value && lookup(&values, &f.path).is_some())
            .map(|f| f.path.clone())
            .collect();
        for path in &present {
            draft.touch(path);
        }
        self.draft = draft;
        self.validated = None;
        self.loaded_from = Some(name.to_string());
        Ok(())
    }

    // ---- save --------------------------------------------------------------------------

    fn target_and_options(&self, req: &SaveRequest) -> Result<(PathBuf, WriteOptions), ApiError> {
        if self.draft.has_errors() {
            return Err(ApiError::new("fix the errors before saving"));
        }
        let path = req
            .path
            .as_deref()
            .map_or_else(|| self.path.clone(), PathBuf::from);
        let defaults = WriteOptions::default();
        let opts = WriteOptions {
            only_changed: req.only_changed.unwrap_or(defaults.only_changed),
            comment_level: req.comment_level.unwrap_or(defaults.comment_level),
        };
        Ok((path, opts))
    }

    /// Renders and verifies exactly as [`Session::save`] does, and writes nothing.
    pub fn preview(&self, req: &SaveRequest) -> Result<PreviewResponse, ApiError> {
        let (path, opts) = self.target_and_options(req)?;
        let p = prepare(&self.draft, &path, &opts).map_err(|e| ApiError::new(e.to_string()))?;
        Ok(PreviewResponse {
            filename: path.file_name().map_or_else(
                || "config.toml".into(),
                |n| n.to_string_lossy().into_owned(),
            ),
            path: path.display().to_string(),
            exists: p.existing.is_some(),
            unchanged: p.unchanged(),
            existing: p.existing,
            text: p.text,
        })
    }

    pub fn save(&self, req: &SaveRequest) -> Result<SaveResponse, ApiError> {
        let (path, opts) = self.target_and_options(req)?;
        let report = save(&self.draft, &path, &opts).map_err(|e| ApiError::new(e.to_string()))?;
        let shown = path.display().to_string();
        Ok(SaveResponse {
            next: vec![
                format!("audigma play -i <file> -c {shown}"),
                format!("audigma run -c {shown}"),
            ],
            created: report.created,
            unchanged: report.unchanged,
            backup: report.backup.map(|b| b.display().to_string()),
            path: shown,
        })
    }

    // ---- guided ------------------------------------------------------------------------

    pub fn discovery_view(&self) -> DiscoveryView {
        let Some(g) = &self.guided else {
            return DiscoveryView {
                tiers: Vec::new(),
                progress: 0.0,
                active: false,
                stale: false,
            };
        };
        DiscoveryView {
            tiers: g
                .discovery
                .tiers
                .iter()
                .map(|(k, s)| TierView {
                    key: k.clone(),
                    label: g.tier_label(k),
                    state: s.clone(),
                })
                .collect(),
            progress: g.discovery.progress,
            active: g.discovery.active,
            stale: g.discovery_stale(&self.draft),
        }
    }

    pub fn guided_state(&self) -> GuidedState {
        GuidedState {
            active: self.guided.is_some(),
            step: self
                .guided
                .as_ref()
                .map(|g| g.view(&self.catalog, &self.draft, self.probe.as_ref())),
            can_go_back: self.guided.as_ref().is_some_and(Guided::can_go_back),
            use_case: self.guided.as_ref().and_then(|g| g.use_case),
            discovery: self.discovery_view(),
        }
    }

    /// Starts guided setup. Only a new file takes it (Decision 27).
    pub fn guided_start(&mut self) -> Result<(), ApiError> {
        if self.existed {
            return Err(ApiError::new(
                "guided setup is for new files; this file already exists",
            ));
        }
        if self.loaded_from.is_some() {
            return Err(ApiError::new(
                "guided setup is for new files; a file was loaded into this draft",
            ));
        }
        if self.busy {
            return Err(ApiError::new("a benchmark is running"));
        }
        self.draft = Draft::empty();
        self.use_case = None;
        self.validated = None;
        self.guided = Some(Guided::new());
        Ok(())
    }

    /// Whether the step the flow is on needs a probe nobody has run yet.
    pub fn guided_waits_for_probe(&self) -> bool {
        self.guided_state()
            .step
            .is_some_and(|s| s.waiting_for_probe)
    }

    /// Answers the current step. [`Outcome::NeedsProbe`] and [`Outcome::StartBenchmark`] are
    /// returned for the caller to act on (the API layer probes, or starts the sweep).
    pub fn guided_answer(&mut self, key: &str) -> Result<Outcome, ApiError> {
        if self.busy {
            return Err(ApiError::new("a benchmark is running"));
        }
        let g = self
            .guided
            .as_mut()
            .ok_or_else(|| ApiError::new("guided setup is not running"))?;
        let out = g
            .answer(&self.catalog, &mut self.draft, self.probe.as_ref(), key)
            .map_err(ApiError::new)?;
        self.use_case = g.use_case;
        if out == Outcome::Done {
            self.guided = None;
        }
        Ok(out)
    }

    pub fn guided_back(&mut self) -> Result<(), ApiError> {
        if self.busy {
            return Err(ApiError::new("a benchmark is running"));
        }
        let g = self
            .guided
            .as_mut()
            .ok_or_else(|| ApiError::new("guided setup is not running"))?;
        if !g.back(&mut self.draft) {
            return Err(ApiError::new("this is the first step"));
        }
        self.use_case = g.use_case;
        Ok(())
    }

    /// Leaves guided setup for the form with the answers so far.
    pub fn guided_skip(&mut self) -> Result<(), ApiError> {
        if self.busy {
            return Err(ApiError::new("a benchmark is running"));
        }
        if self.guided.take().is_none() {
            return Err(ApiError::new("guided setup is not running"));
        }
        Ok(())
    }

    /// Where the frontend goes once the flow is over.
    pub fn after_done(&self) -> GuidedNext {
        if self.use_case == Some(UseCase::OfflineConversion) {
            GuidedNext::FormOffline
        } else {
            GuidedNext::Review
        }
    }

    /// Feeds one benchmark event of a discovery sweep.
    pub fn on_discovery(&mut self, ev: &BenchmarkEvent) -> DiscoveryNext {
        match self.guided.as_mut() {
            Some(g) => g.on_discovery(&self.catalog, &self.draft, self.probe.as_ref(), ev),
            None => DiscoveryNext::Finished,
        }
    }
}

fn lookup<'a>(table: &'a toml::Table, path: &str) -> Option<&'a toml::Value> {
    let mut parts = path.split('.');
    let mut cur = table.get(parts.next()?)?;
    for p in parts {
        cur = cur.as_table()?.get(p)?;
    }
    Some(cur)
}

fn draft_error_text(e: &DraftError) -> String {
    e.to_string()
}
