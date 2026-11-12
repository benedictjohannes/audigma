//! The form's state machine. Pure: key events go in, state changes and [`Effect`]s come out.
//! Probes and benchmarks run elsewhere (see `tui::run`) and report back through
//! [`App::on_probe`] / [`App::on_bench`], so everything here is testable without a terminal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use audigma_settings::meta::{AssistKind, Catalog, Command, Field, FieldKind, UseCase};
use audigma_settings::{Settings, SettingsError};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

use crate::assist::benchmark::{BenchmarkEvent, BenchmarkResult, Stage};
use crate::assist::capability::{self, CapabilityFinding};
use crate::assist::probe::ProbeReport;
use crate::draft::{Draft, Issue};
use crate::writer::text::{option_text, show_value};
use crate::writer::{CommentLevel, SaveReport, WriteOptions, save};

/// Work the runner has to do for the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Run both probes and report with [`App::on_probe`].
    Probe,
    /// Benchmark exactly this configuration; report with [`App::on_bench`].
    StartBenchmark(Box<Settings>),
    CancelBenchmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Start,
    Form,
    Review,
    Save,
}

/// Which commands the form shows keys for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    All,
    /// `run` and `play`.
    Live,
    /// `convert`.
    Offline,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::All => "all",
            Mode::Live => "live",
            Mode::Offline => "offline",
        }
    }

    fn commands(self) -> &'static [Command] {
        match self {
            Mode::All => &[Command::Run, Command::Play, Command::Convert],
            Mode::Live => &[Command::Run, Command::Play],
            Mode::Offline => &[Command::Convert],
        }
    }

    fn next(self) -> Self {
        match self {
            Mode::All => Mode::Live,
            Mode::Live => Mode::Offline,
            Mode::Offline => Mode::All,
        }
    }

    /// The mode a use case starts the form in.
    pub fn for_use_case(u: Option<UseCase>) -> Self {
        match u {
            None => Mode::All,
            Some(UseCase::OfflineConversion) => Mode::Offline,
            Some(_) => Mode::Live,
        }
    }
}

pub fn use_case_label(u: UseCase) -> &'static str {
    match u {
        UseCase::InteractiveDesktop => "interactive desktop",
        UseCase::HeadlessAppliance => "headless appliance",
        UseCase::OfflineConversion => "offline conversion",
    }
}

pub const USE_CASES: [(UseCase, &str); 3] = [
    (
        UseCase::InteractiveDesktop,
        "A daemon that stays ready on a desktop; apps, browsers and MPD play into it.",
    ),
    (
        UseCase::HeadlessAppliance,
        "A dedicated music box with no desktop sound server.",
    ),
    (
        UseCase::OfflineConversion,
        "Converting files with `audigma convert`.",
    ),
];

/// A row of the form's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Section(String),
    /// A key or a group heading, by absolute path.
    Field(String),
}

/// What choosing a list item does.
#[derive(Debug, Clone, PartialEq)]
pub enum ListAction {
    Set(Vec<(String, Value)>),
    Unset(String),
    /// Ask for a whole number for this key (the `also_integer` entry).
    Integer(String),
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub label: String,
    pub summary: String,
    /// Option index in the focused field, so the help pane can show its description and caution.
    pub option: Option<usize>,
    pub action: ListAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTarget {
    Field,
    SavePath,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Popup {
    UseCase {
        sel: usize,
    },
    List {
        title: String,
        items: Vec<ListItem>,
        sel: usize,
    },
    Input {
        title: String,
        hint: String,
        buf: String,
        error: Option<String>,
        target: InputTarget,
        /// For [`InputTarget::Field`]: the key being edited; the number replaces an option.
        path: String,
        integer_only: bool,
    },
    ConfirmQuit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Bench {
    Idle,
    Running { stage: Stage, fraction: f64 },
    Done(Box<BenchmarkResult>),
    Failed(String),
    Cancelled,
}

/// Settings of the save page.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveState {
    pub path: String,
    pub options: WriteOptions,
    /// Row focus: 0 path, 1 only changed, 2 comment level, 3 save.
    pub focus: usize,
    /// Whether `path` exists (the file is then updated in place, after a backup).
    pub exists: bool,
    pub outcome: Option<Result<SaveReport, String>>,
}

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

pub struct App {
    pub catalog: Catalog,
    pub draft: Draft,
    pub path: PathBuf,
    pub is_new: bool,
    pub screen: Screen,
    pub popup: Option<Popup>,
    pub use_case: Option<UseCase>,
    pub mode: Mode,
    pub show_all: bool,
    pub folded: BTreeSet<String>,
    pub cursor: usize,
    /// Lines the help pane is scrolled down.
    pub help_scroll: u16,
    pub start_sel: usize,
    pub notice: Option<String>,
    pub probe: Option<ProbeReport>,
    pub probe_running: bool,
    /// An assist (id, key path) waiting for the probe it asked for.
    pending_assist: Option<(String, String)>,
    pub bench: Bench,
    /// The draft the last benchmark ran on, to mark a stale result.
    pub bench_draft: Option<Draft>,
    pub save: SaveState,
    pub quit: bool,
    /// Resolved values (defaults overlaid with the draft's) for conditions.
    resolved: BTreeMap<String, Value>,
    /// Values the draft sets explicitly.
    set: BTreeMap<String, Value>,
}

fn flatten(prefix: &str, t: &toml::Table, out: &mut BTreeMap<String, Value>) {
    for (k, v) in t {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            toml::Value::Table(sub) => flatten(&path, sub, out),
            other => {
                if let Ok(j) = serde_json::to_value(other) {
                    out.insert(path, j);
                }
            }
        }
    }
}

fn is_bool_field(f: &Field) -> bool {
    f.default.as_ref().is_some_and(Value::is_boolean)
        || f.recommended.values().any(|r| r.value.is_boolean())
        || f.default_text
            .as_deref()
            .is_some_and(|t| t.starts_with("true") || t.starts_with("false"))
}

impl App {
    /// Opens the form on `existing` (the target's current text) or a new file when `None`.
    pub fn new(path: &Path, existing: Option<&str>) -> Result<Self, LoadError> {
        let catalog = Catalog::load().expect("the metadata catalog is valid");
        let draft = match existing {
            Some(text) => Draft::from_toml_str(text).map_err(|e| LoadError::Invalid {
                path: path.to_path_buf(),
                message: match &e {
                    crate::draft::DraftError::Settings(SettingsError::TomlParse(t)) => {
                        t.to_string().trim_end().to_string()
                    }
                    other => other.to_string(),
                },
            })?,
            None => Draft::empty(),
        };
        let folded = catalog
            .sections()
            .into_iter()
            .filter(|s| catalog.section(s).is_some_and(|m| m.advanced))
            .map(str::to_string)
            .collect();
        let mut app = Self {
            catalog,
            draft,
            path: path.to_path_buf(),
            is_new: existing.is_none(),
            screen: Screen::Start,
            popup: None,
            use_case: None,
            mode: Mode::All,
            show_all: false,
            folded,
            cursor: 0,
            help_scroll: 0,
            start_sel: 1,
            notice: None,
            probe: None,
            probe_running: false,
            pending_assist: None,
            bench: Bench::Idle,
            bench_draft: None,
            save: SaveState {
                path: path.display().to_string(),
                options: WriteOptions::default(),
                focus: 3,
                exists: path.exists(),
                outcome: None,
            },
            quit: false,
            resolved: BTreeMap::new(),
            set: BTreeMap::new(),
        };
        app.refresh();
        Ok(app)
    }

    /// Reads `path` (a missing file is a new one) and opens the form on it.
    pub fn open(path: &Path) -> Result<Self, LoadError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::new(path, Some(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::new(path, None),
            Err(source) => Err(LoadError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Rebuilds the value caches after the draft changed.
    fn refresh(&mut self) {
        let mut set = BTreeMap::new();
        if let Ok(t) = self.draft.values() {
            flatten("", &t, &mut set);
        }
        let mut resolved = BTreeMap::new();
        for f in self.catalog.fields() {
            if f.kind == FieldKind::Value {
                if let Some(d) = &f.default {
                    resolved.insert(f.path.clone(), d.clone());
                }
            }
        }
        resolved.extend(set.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.set = set;
        self.resolved = resolved;
    }

    // ---- queries -----------------------------------------------------------------------

    /// The value the draft sets for `path`, if any.
    pub fn set_value(&self, path: &str) -> Option<&Value> {
        self.set.get(path)
    }

    /// The value in effect for `path`: the draft's, else the documented default.
    pub fn resolved_value(&self, path: &str) -> Option<&Value> {
        self.resolved.get(path)
    }

    pub fn dirty(&self) -> bool {
        !self.draft.touched().is_empty()
    }

    pub fn issues(&self) -> Vec<Issue> {
        self.draft.issues()
    }

    /// Whether `path` applies to the current mode (`applies_to` empty means every command).
    fn applies(&self, applies_to: &[Command]) -> bool {
        applies_to.is_empty() || applies_to.iter().any(|c| self.mode.commands().contains(c))
    }

    /// Why `path` is hidden, if it is: `None` means shown.
    pub fn hidden_reason(&self, f: &Field) -> Option<&'static str> {
        let section = f.path.split('.').next().unwrap_or("");
        let section_ok = self
            .catalog
            .section(section)
            .is_none_or(|s| self.applies(&s.applies_to));
        if !section_ok || !self.applies(&f.applies_to) {
            return Some("other mode");
        }
        if !self.catalog.is_visible(&f.path, &self.resolved) {
            return Some("not applicable to the current choices");
        }
        None
    }

    pub fn is_required(&self, f: &Field) -> bool {
        self.mode
            .commands()
            .iter()
            .any(|c| self.catalog.is_required(&f.path, *c, &self.resolved))
    }

    /// The list rows in display order.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for section in self.catalog.sections() {
            let meta = self.catalog.section(section);
            let fields: Vec<&Field> = self
                .catalog
                .fields()
                .iter()
                .filter(|f| f.path.split('.').next() == Some(section))
                .filter(|f| self.show_all || self.hidden_reason(f).is_none())
                .collect();
            let section_hidden = meta.is_some_and(|m| !self.applies(&m.applies_to));
            if fields.is_empty() || (section_hidden && !self.show_all) {
                continue;
            }
            rows.push(Row::Section(section.to_string()));
            if self.folded.contains(section) {
                continue;
            }
            rows.extend(fields.into_iter().map(|f| Row::Field(f.path.clone())));
        }
        rows
    }

    pub fn focused(&self) -> Option<Row> {
        self.rows().get(self.cursor).cloned()
    }

    pub fn focused_field(&self) -> Option<&Field> {
        match self.focused()? {
            Row::Field(p) => self.catalog.field(&p),
            Row::Section(_) => None,
        }
    }

    /// Findings of the capability pre-check, when an ALSA probe is available.
    pub fn capability_findings(&self) -> Vec<CapabilityFinding> {
        let Some(Ok(alsa)) = self.probe.as_ref().map(|p| &p.alsa) else {
            return Vec::new();
        };
        if self.draft.has_errors() {
            return Vec::new();
        }
        capability::check(self.draft.settings(), alsa)
    }

    /// The preset bundle whose values all match the draft's resolved values.
    pub fn matching_preset(&self, assist_id: &str) -> Option<String> {
        let a = self.catalog.assist(assist_id)?;
        a.presets
            .iter()
            .find(|p| {
                p.values.iter().all(|(k, v)| {
                    let v = normalized(k, v);
                    self.resolved.get(k).is_some_and(|r| same(r, &v))
                })
            })
            .map(|p| p.label.clone())
    }

    pub fn bench_is_stale(&self) -> bool {
        self.bench_draft.as_ref().is_some_and(|d| d != &self.draft)
    }

    // ---- navigation helpers -------------------------------------------------------------

    fn clamp(&mut self) {
        let n = self.rows().len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
    }

    /// Moves the cursor to `path`, unfolding its section and showing hidden keys if needed.
    pub fn goto_field(&mut self, path: &str) {
        self.screen = Screen::Form;
        let section = path.split('.').next().unwrap_or("").to_string();
        self.folded.remove(&section);
        // An issue about a whole section (`target`) lands on the section row.
        let want = if self.catalog.field(path).is_some() {
            Row::Field(path.to_string())
        } else {
            Row::Section(section)
        };
        if !self.rows().contains(&want) {
            self.show_all = true;
        }
        if let Some(i) = self.rows().iter().position(|r| *r == want) {
            self.cursor = i;
        }
    }

    fn set_use_case(&mut self, u: Option<UseCase>) {
        self.use_case = u;
        self.mode = Mode::for_use_case(u);
        self.clamp();
    }

    // ---- editing ------------------------------------------------------------------------

    fn apply(&mut self, edits: &[(String, Value)]) {
        for (path, v) in edits {
            if let Err(e) = self.draft.set(path, v) {
                self.notice = Some(format!("{path}: {e}"));
                break;
            }
        }
        self.refresh();
        self.clamp();
    }

    fn unset(&mut self, path: &str) {
        if let Err(e) = self.draft.unset(path) {
            self.notice = Some(format!("{path}: {e}"));
        }
        self.refresh();
        self.clamp();
    }

    /// Opens the editor for the focused field.
    fn edit_focused(&mut self) {
        let Some(f) = self.focused_field().cloned() else {
            return;
        };
        if f.kind == FieldKind::Group {
            return;
        }
        if !f.options.is_empty() {
            let recommended = self
                .use_case
                .and_then(|u| f.recommended.get(&u).map(|r| (u, option_text(&r.value))));
            let visible: Vec<(usize, &audigma_settings::meta::OptionMeta)> = f
                .options
                .iter()
                .enumerate()
                .filter(|(_, o)| o.show_if.as_ref().is_none_or(|c| c.eval(&self.resolved)))
                .collect();
            let mut items: Vec<ListItem> = visible
                .iter()
                .map(|(i, o)| {
                    let mut label = option_label(&o.key, &o.label);
                    if let Some((u, Some(r))) = &recommended {
                        if *r == o.key {
                            label.push_str(&format!(" (Recommended for {})", use_case_label(*u)));
                        }
                    }
                    ListItem {
                        label,
                        summary: o.summary.clone(),
                        option: Some(*i),
                        action: ListAction::Set(vec![(f.path.clone(), option_value(&o.key))]),
                    }
                })
                .collect();
            if let Some(n) = &f.also_integer {
                items.push(ListItem {
                    label: n.label.clone(),
                    summary: n.summary.clone(),
                    option: None,
                    action: ListAction::Integer(f.path.clone()),
                });
            }
            // Cursor: the value the draft sets, else the use case's recommendation, else the
            // schema default, else the first item.
            let target = self
                .set
                .get(&f.path)
                .and_then(option_text)
                .or_else(|| recommended.and_then(|(_, r)| r))
                .or_else(|| f.default.as_ref().and_then(option_text));
            let sel = target
                .and_then(|t| visible.iter().position(|(_, o)| o.key == t))
                .unwrap_or(0);
            self.popup = Some(Popup::List {
                title: f.key.clone(),
                items,
                sel,
            });
        } else if is_bool_field(&f) {
            let now = self
                .set
                .get(&f.path)
                .and_then(Value::as_bool)
                .or_else(|| {
                    let u = self.use_case?;
                    f.recommended.get(&u)?.value.as_bool()
                })
                .or_else(|| f.default.as_ref().and_then(Value::as_bool));
            let rec = self
                .use_case
                .and_then(|u| Some((u, f.recommended.get(&u)?.value.as_bool()?)));
            let items = [true, false]
                .into_iter()
                .map(|b| ListItem {
                    label: match rec {
                        Some((u, r)) if r == b => {
                            format!("{b} (Recommended for {})", use_case_label(u))
                        }
                        _ => b.to_string(),
                    },
                    summary: String::new(),
                    option: None,
                    action: ListAction::Set(vec![(f.path.clone(), Value::Bool(b))]),
                })
                .collect();
            self.popup = Some(Popup::List {
                title: f.key.clone(),
                items,
                sel: usize::from(now == Some(false)),
            });
        } else {
            let buf = self.set.get(&f.path).map(input_text).unwrap_or_default();
            self.popup = Some(Popup::Input {
                title: f.key.clone(),
                hint: input_hint(&f),
                buf,
                error: None,
                target: InputTarget::Field,
                path: f.path.clone(),
                integer_only: false,
            });
        }
    }

    fn open_assist(&mut self) -> Vec<Effect> {
        let Some(f) = self.focused_field().cloned() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        for id in &f.assist {
            let Some(a) = self.catalog.assist(id).cloned() else {
                continue;
            };
            match (id.as_str(), a.kind) {
                ("alsa-detect", _) | ("pipewire-sinks", _) => {
                    // Device lists are never reused: every press probes again, and the
                    // modal opens from `on_probe` once the fresh report arrives.
                    self.pending_assist = Some((id.clone(), f.path.clone()));
                    if !self.probe_running {
                        self.probe_running = true;
                        effects.push(Effect::Probe);
                    }
                    self.notice = Some("Probing devices...".into());
                    return effects;
                }
                (_, AssistKind::Preset) => {
                    let items = a
                        .presets
                        .iter()
                        .map(|p| ListItem {
                            label: p.label.clone(),
                            summary: p.summary.clone(),
                            option: None,
                            action: ListAction::Set(
                                p.values
                                    .iter()
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect(),
                            ),
                        })
                        .collect();
                    self.popup = Some(Popup::List {
                        title: a.label.clone(),
                        items,
                        sel: 0,
                    });
                    return effects;
                }
                _ => {}
            }
        }
        self.notice = Some("No assist for this key here.".into());
        effects
    }

    fn assist_list(&self, id: &str, f: &Field) -> Option<Popup> {
        let probe = self.probe.as_ref()?;
        let mut items = Vec::new();
        let is_alsa_device = id == "alsa-detect" && f.path == "output.device";
        if is_alsa_device {
            match &probe.alsa {
                Ok(r) => {
                    for c in &r.cards {
                        for d in &c.devices {
                            let mut s = format!("{} / {}", c.longname, d.name);
                            if d.is_busy {
                                s.push_str(" (busy)");
                            }
                            items.push(ListItem {
                                label: d.device_path.clone(),
                                summary: s,
                                option: None,
                                action: ListAction::Set(vec![(
                                    "output.device".into(),
                                    Value::String(d.device_path.clone()),
                                )]),
                            });
                        }
                    }
                }
                Err(e) => return Some(message_list("ALSA probe failed", e)),
            }
        } else if id == "pipewire-sinks" {
            match &probe.pipewire {
                Ok(r) => {
                    for s in &r.sinks {
                        items.push(ListItem {
                            label: s.node_name.clone(),
                            summary: s.description.clone().unwrap_or_default(),
                            option: None,
                            action: ListAction::Set(vec![(
                                "output.target_node".into(),
                                Value::String(s.node_name.clone()),
                            )]),
                        });
                    }
                }
                Err(e) => return Some(message_list("PipeWire probe failed", e)),
            }
        } else {
            return None;
        }
        if items.is_empty() {
            return Some(message_list("Nothing found", "The probe found no devices."));
        }
        Some(Popup::List {
            title: if is_alsa_device {
                "ALSA devices".into()
            } else {
                "PipeWire sinks".into()
            },
            items,
            sel: 0,
        })
    }

    // ---- runner callbacks ---------------------------------------------------------------

    pub fn on_probe(&mut self, report: ProbeReport) {
        self.probe_running = false;
        self.probe = Some(report);
        self.notice = Some("Device probe finished.".into());
        // Open the assist that asked for this probe, if the user is still on that key.
        if let Some((id, path)) = self.pending_assist.take() {
            let still_there =
                self.popup.is_none() && self.focused_field().is_some_and(|f| f.path == path);
            if still_there {
                if let Some(f) = self.focused_field().cloned() {
                    if let Some(p) = self.assist_list(&id, &f) {
                        self.notice = None;
                        self.popup = Some(p);
                    }
                }
            }
        }
    }

    pub fn on_bench(&mut self, ev: BenchmarkEvent) {
        self.bench = match ev {
            BenchmarkEvent::Progress { stage, fraction } => Bench::Running { stage, fraction },
            BenchmarkEvent::Finished(r) => Bench::Done(r),
            BenchmarkEvent::Failed(m) => Bench::Failed(m),
            BenchmarkEvent::Cancelled => Bench::Cancelled,
        };
    }

    // ---- keys ---------------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.kind == KeyEventKind::Release {
            return Vec::new();
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return self.request_quit();
        }
        if self.popup.is_some() {
            return self.popup_key(key);
        }
        match self.screen {
            Screen::Start => self.start_key(key),
            Screen::Form => self.form_key(key),
            Screen::Review => self.review_key(key),
            Screen::Save => self.save_key(key),
        }
    }

    fn request_quit(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if matches!(self.bench, Bench::Running { .. }) {
            effects.push(Effect::CancelBenchmark);
        }
        if self.dirty() && !self.saved() {
            self.popup = Some(Popup::ConfirmQuit);
        } else {
            self.quit = true;
        }
        effects
    }

    fn saved(&self) -> bool {
        matches!(self.save.outcome, Some(Ok(_)))
    }

    fn start_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.start_sel = 0,
            KeyCode::Down | KeyCode::Char('j') => self.start_sel = 1,
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Enter => {
                if self.start_sel == 0 {
                    self.notice = Some("Guided setup is not available yet.".into());
                } else {
                    self.screen = Screen::Form;
                    if self.is_new {
                        self.popup = Some(Popup::UseCase { sel: 0 });
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn form_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let before = (self.cursor, self.popup.is_some());
        let effects = self.form_key_inner(key);
        if before.0 != self.cursor {
            self.help_scroll = 0;
        }
        effects
    }

    fn form_key_inner(&mut self, key: KeyEvent) -> Vec<Effect> {
        let n = self.rows().len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(n.saturating_sub(1));
            }
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(10),
            KeyCode::PageDown => self.cursor = (self.cursor + 10).min(n.saturating_sub(1)),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = n.saturating_sub(1),
            KeyCode::Left | KeyCode::Right => {
                if let Some(Row::Section(s)) = self.focused() {
                    self.toggle_fold(&s, key.code == KeyCode::Left);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.focused() {
                Some(Row::Section(s)) => {
                    let fold = !self.folded.contains(&s);
                    self.toggle_fold(&s, fold);
                }
                Some(Row::Field(_)) => self.edit_focused(),
                None => {}
            },
            KeyCode::Backspace | KeyCode::Delete => {
                if let Some(Row::Field(p)) = self.focused() {
                    if self.set.contains_key(&p) {
                        self.unset(&p);
                    }
                }
            }
            KeyCode::Char(']') => self.help_scroll = self.help_scroll.saturating_add(3),
            KeyCode::Char('[') => self.help_scroll = self.help_scroll.saturating_sub(3),
            KeyCode::Char('a') => return self.open_assist(),
            KeyCode::Char('u') => {
                let sel = self
                    .use_case
                    .and_then(|u| USE_CASES.iter().position(|(c, _)| *c == u))
                    .unwrap_or(0);
                self.popup = Some(Popup::UseCase { sel });
            }
            KeyCode::Char('m') => {
                self.mode = self.mode.next();
                self.clamp();
            }
            KeyCode::Char('*') => {
                self.show_all = !self.show_all;
                self.clamp();
            }
            KeyCode::Char('r') | KeyCode::F(2) => self.screen = Screen::Review,
            KeyCode::Char('q') | KeyCode::Esc => return self.request_quit(),
            _ => {}
        }
        Vec::new()
    }

    fn toggle_fold(&mut self, section: &str, fold: bool) {
        if fold {
            self.folded.insert(section.to_string());
        } else {
            self.folded.remove(section);
        }
        self.clamp();
    }

    fn review_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('f') => {
                if matches!(self.bench, Bench::Running { .. }) {
                    return vec![Effect::CancelBenchmark];
                }
                self.screen = Screen::Form;
            }
            KeyCode::Char('g') => {
                if let Some(path) = self.issues().into_iter().find_map(|i| i.path) {
                    self.goto_field(&path);
                }
            }
            KeyCode::Char('p') => {
                if !self.probe_running {
                    self.probe_running = true;
                    return vec![Effect::Probe];
                }
            }
            KeyCode::Char('b') => {
                if self.draft.has_errors() {
                    self.notice = Some("Fix the errors before validating.".into());
                } else if !matches!(self.bench, Bench::Running { .. }) {
                    self.bench = Bench::Running {
                        stage: Stage::EndToEnd,
                        fraction: 0.0,
                    };
                    self.bench_draft = Some(self.draft.clone());
                    return vec![Effect::StartBenchmark(Box::new(
                        self.draft.settings().clone(),
                    ))];
                }
            }
            KeyCode::Char('c') => {
                if matches!(self.bench, Bench::Running { .. }) {
                    return vec![Effect::CancelBenchmark];
                }
            }
            KeyCode::Char('s') | KeyCode::Enter => {
                if self.draft.has_errors() {
                    self.notice = Some("Fix the errors before saving.".into());
                } else {
                    self.screen = Screen::Save;
                    self.save.exists = Path::new(&self.save.path).exists();
                }
            }
            KeyCode::Char('q') => return self.request_quit(),
            _ => {}
        }
        Vec::new()
    }

    fn save_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.saved() {
            if matches!(key.code, KeyCode::Char('q') | KeyCode::Enter | KeyCode::Esc) {
                self.quit = true;
            }
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.screen = Screen::Review,
            KeyCode::Up | KeyCode::Char('k') => self.save.focus = self.save.focus.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.save.focus = (self.save.focus + 1).min(3);
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ') | KeyCode::Enter => {
                match self.save.focus {
                    0 if key.code == KeyCode::Enter => {
                        self.popup = Some(Popup::Input {
                            title: "Write to".into(),
                            hint: "Path of the configuration file.".into(),
                            buf: self.save.path.clone(),
                            error: None,
                            target: InputTarget::SavePath,
                            path: String::new(),
                            integer_only: false,
                        });
                    }
                    1 => self.save.options.only_changed = !self.save.options.only_changed,
                    2 => {
                        self.save.options.comment_level =
                            match (self.save.options.comment_level, key.code == KeyCode::Left) {
                                (CommentLevel::None, false) | (CommentLevel::Full, true) => {
                                    CommentLevel::Brief
                                }
                                (CommentLevel::Brief, false) | (CommentLevel::None, true) => {
                                    CommentLevel::Full
                                }
                                (CommentLevel::Full, false) | (CommentLevel::Brief, true) => {
                                    CommentLevel::None
                                }
                            };
                    }
                    3 if key.code == KeyCode::Enter => self.do_save(),
                    _ => {}
                }
            }
            KeyCode::Char('q') => return self.request_quit(),
            _ => {}
        }
        Vec::new()
    }

    fn do_save(&mut self) {
        let path = PathBuf::from(&self.save.path);
        self.save.outcome =
            Some(save(&self.draft, &path, &self.save.options).map_err(|e| e.to_string()));
    }

    fn popup_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(popup) = self.popup.take() else {
            return Vec::new();
        };
        match popup {
            Popup::ConfirmQuit => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => self.quit = true,
                    _ => {}
                }
                Vec::new()
            }
            Popup::UseCase { mut sel } => {
                let last = USE_CASES.len() + 1; // "no use case" and "skip" share the last row
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => sel = (sel + 1).min(last - 1),
                    KeyCode::Esc => {
                        return Vec::new();
                    }
                    KeyCode::Enter => {
                        self.set_use_case(USE_CASES.get(sel).map(|(u, _)| *u));
                        return Vec::new();
                    }
                    _ => {}
                }
                self.popup = Some(Popup::UseCase { sel });
                Vec::new()
            }
            Popup::List {
                title,
                items,
                mut sel,
            } => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => {
                        sel = (sel + 1).min(items.len().saturating_sub(1));
                    }
                    KeyCode::Esc => return Vec::new(),
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let action = items[sel].action.clone();
                        self.run_action(action);
                        return Vec::new();
                    }
                    _ => {}
                }
                self.popup = Some(Popup::List { title, items, sel });
                Vec::new()
            }
            Popup::Input {
                title,
                hint,
                mut buf,
                mut error,
                target,
                path,
                integer_only,
            } => {
                match key.code {
                    KeyCode::Esc => return Vec::new(),
                    KeyCode::Backspace => {
                        buf.pop();
                        error = None;
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        error = None;
                    }
                    KeyCode::Enter => match target {
                        InputTarget::SavePath => {
                            if !buf.trim().is_empty() {
                                self.save.path = buf.trim().to_string();
                                self.save.exists = Path::new(&self.save.path).exists();
                            }
                            return Vec::new();
                        }
                        InputTarget::Field => match self.commit_input(&path, &buf, integer_only) {
                            Ok(()) => return Vec::new(),
                            Err(e) => error = Some(e),
                        },
                    },
                    _ => {}
                }
                self.popup = Some(Popup::Input {
                    title,
                    hint,
                    buf,
                    error,
                    target,
                    path,
                    integer_only,
                });
                Vec::new()
            }
        }
    }

    fn run_action(&mut self, action: ListAction) {
        match action {
            ListAction::Set(edits) => self.apply(&edits),
            ListAction::Unset(p) => self.unset(&p),
            ListAction::Close => {}
            ListAction::Integer(path) => {
                let range = self
                    .catalog
                    .field(&path)
                    .and_then(|f| f.also_integer.as_ref())
                    .map(|n| format!("A whole number from {} to {}.", n.range[0], n.range[1]))
                    .unwrap_or_default();
                let buf = self
                    .set
                    .get(&path)
                    .filter(|v| v.is_i64() || v.is_u64())
                    .map(input_text)
                    .unwrap_or_default();
                self.popup = Some(Popup::Input {
                    title: path.rsplit('.').next().unwrap_or("").to_string(),
                    hint: range,
                    buf,
                    error: None,
                    target: InputTarget::Field,
                    path,
                    integer_only: true,
                });
            }
        }
    }

    /// Parses the typed text and sets the key; the error is shown in the popup.
    fn commit_input(&mut self, path: &str, text: &str, integer_only: bool) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            self.unset(path);
            return Ok(());
        }
        let value = if integer_only {
            text.parse::<i64>()
                .map(Value::from)
                .map_err(|_| "Enter a whole number.".to_string())?
        } else {
            parse_input(text)
        };
        if let (Some([lo, hi]), Some(x)) = (
            self.catalog.field(path).and_then(|f| f.range),
            value.as_f64(),
        ) {
            if x < lo || x > hi {
                return Err(format!(
                    "Must be between {} and {}.",
                    show_num(lo),
                    show_num(hi)
                ));
            }
        }
        self.draft.set(path, &value).map_err(|e| e.to_string())?;
        self.refresh();
        self.clamp();
        Ok(())
    }
}

/// `key - Label`, as the picker shows an option (just the label when they read the same).
fn option_label(key: &str, label: &str) -> String {
    if key.eq_ignore_ascii_case(label) {
        label.to_string()
    } else {
        format!("{key} - {label}")
    }
}

fn message_list(title: &str, body: &str) -> Popup {
    Popup::List {
        title: title.to_string(),
        items: vec![ListItem {
            label: "Close".into(),
            summary: body.to_string(),
            option: None,
            action: ListAction::Close,
        }],
        sel: 0,
    }
}

/// The JSON value of option `key`, typed like the key's current value (numbers and booleans
/// match by decimal text in the metadata, §Decisions 20).
fn option_value(key: &str) -> Value {
    if let Ok(i) = key.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(b) = key.parse::<bool>() {
        return Value::Bool(b);
    }
    Value::String(key.to_string())
}

/// `v` as the settings store it (`apodization = "gentle"` is kept as its 19 kHz edge).
fn normalized(path: &str, v: &Value) -> Value {
    let mut d = Draft::empty();
    if d.set(path, v).is_err() {
        return v.clone();
    }
    let mut flat = BTreeMap::new();
    if let Ok(t) = d.values() {
        flatten("", &t, &mut flat);
    }
    flat.remove(path).unwrap_or_else(|| v.clone())
}

fn same(a: &Value, b: &Value) -> bool {
    match (option_text(a), option_text(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Text shown when editing an existing value.
fn input_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Interprets typed text: a TOML value (number, boolean, array, quoted string) when it parses
/// as one, else a bare string.
pub fn parse_input(text: &str) -> Value {
    let probe = format!("v = {text}");
    match toml::from_str::<toml::Table>(&probe) {
        Ok(t) => t
            .get("v")
            .and_then(|v| serde_json::to_value(v).ok())
            .unwrap_or_else(|| Value::String(text.to_string())),
        Err(_) => Value::String(text.to_string()),
    }
}

fn input_hint(f: &Field) -> String {
    let mut parts = Vec::new();
    if let Some([lo, hi]) = f.range {
        parts.push(format!("Range: {} to {}.", show_num(lo), show_num(hi)));
    }
    match (&f.default, &f.default_text) {
        (Some(d), _) => parts.push(format!("Default: {}.", show_value(d))),
        (None, Some(t)) => parts.push(format!("Default: {t}.")),
        _ => {}
    }
    parts.push("Empty resets to the default.".into());
    parts.join(" ")
}

fn show_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        x.to_string()
    }
}
