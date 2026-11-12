//! Guided setup: the step engine behind the short front-end that fills a new draft.
//!
//! The flow is data (`guided.toml`): steps, their prose, static choices, skip conditions and the
//! keys each choice sets. This module adds what data cannot say: choices that come from the
//! probe (ALSA devices, PipeWire sinks), rate tiers annotated with the capability check and the
//! discovery results, and the benchmark sweep over the tiers.
//!
//! The engine has no UI dependency and no threads. A frontend renders [`Guided::view`], sends
//! the user's pick to [`Guided::answer`] and acts on the returned [`Outcome`] (probe, start a
//! benchmark). A new draft is the only case: the engine is never started on an existing file.
//!
//! - A `choice` step with exactly one visible choice is answered by it without asking (for
//!   example the format on a PipeWire output, which is always PCM).
//! - [`Guided::back`] restores the draft and the use case from before the step it returns to.
//! - On the last step the use case's recommended values fill every visible key that no step
//!   touched ([`Outcome::Done`]); they are ordinary touched keys, so the review page lists
//!   them as changes.
//! - Discovery sweeps the supported tiers from the lowest upward and stops at the first one that
//!   fails the gates. Results are estimates and go out of date when quality settings change.

use std::collections::BTreeMap;

use audigma_settings::Settings;
use audigma_settings::meta::{Catalog, Condition, UseCase};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assist::benchmark::{BenchmarkEvent, BenchmarkResult};
use crate::assist::capability::{self, Severity};
use crate::assist::probe::ProbeReport;
use crate::draft::Draft;

const SOURCE: &str = include_str!("../guided.toml");

/// Keys the engine supplies to conditions besides the draft's own values.
pub const PSEUDO_KEYS: [&str; 3] = ["use_case", "device.native_dsd", "device.dop"];

/// What kind of input a step takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    /// Static choices from the file.
    #[default]
    Choice,
    /// ALSA devices from the probe, then the static choices.
    AlsaDevice,
    /// PipeWire sinks from the probe, then the static choices.
    PipewireSink,
    /// The bundles of a `preset` assist.
    Presets,
    /// Opt-in benchmark sweep over the supported tiers.
    Discover,
    /// Static choices annotated with the capability check and the discovery results.
    Tiers,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceDef {
    pub key: String,
    pub label: String,
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub caution: Option<String>,
    #[serde(default)]
    pub show_if: Option<Condition>,
    #[serde(default)]
    pub sets: BTreeMap<String, Value>,
    #[serde(default)]
    pub unsets: Vec<String>,
    /// Picking this choice selects the use case.
    #[serde(default)]
    pub use_case: Option<UseCase>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepDef {
    pub id: String,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub kind: StepKind,
    #[serde(default)]
    pub skip_if: Option<Condition>,
    /// Id of the `preset` assist whose bundles are the choices (kind `presets`).
    #[serde(default)]
    pub presets: Option<String>,
    /// Shown when a probe step lists nothing.
    #[serde(default)]
    pub empty: Option<String>,
    #[serde(default, rename = "choices")]
    pub choices: Vec<ChoiceDef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuidedDef {
    #[serde(rename = "step")]
    pub steps: Vec<StepDef>,
}

impl GuidedDef {
    /// The embedded flow.
    pub fn load() -> Result<Self, String> {
        toml::from_str(SOURCE).map_err(|e| format!("guided.toml: {e}"))
    }
}

/// One choice as a frontend shows it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct ChoiceView {
    pub key: String,
    pub label: String,
    pub summary: String,
    pub description: Option<String>,
    pub caution: Option<String>,
    /// Why the use case recommends this choice, when it does.
    pub recommendation: Option<String>,
    /// Extra line: capability warning or discovery result.
    pub note: Option<String>,
    /// Why the choice cannot be picked, when it cannot.
    pub disabled: Option<String>,
}

/// A step as a frontend shows it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct StepView {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub kind: StepKind,
    pub choices: Vec<ChoiceView>,
    /// Index of the choice the cursor starts on.
    pub selected: usize,
    /// 1-based position among the steps this run will ask.
    pub number: usize,
    pub total: usize,
    /// A probe is needed and has not reported yet.
    pub waiting_for_probe: bool,
    /// The probe failed, or listed nothing.
    pub problem: Option<String>,
}

/// What the frontend does after [`Guided::answer`].
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Show [`Guided::view`] again (a new step, or the same one).
    Next,
    /// The new step needs a fresh probe.
    NeedsProbe,
    /// Run this benchmark and feed its events to [`Guided::on_discovery`].
    StartBenchmark(Box<Settings>),
    /// The flow is over; the draft carries the answers and the recommendations.
    Done,
}

/// What a discovery event asks the frontend to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryNext {
    Nothing,
    StartBenchmark(Box<Settings>),
    Finished,
}

/// Result of one tier in the sweep.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TierState {
    Untested,
    Running,
    Passed {
        throughput_x: f64,
        p999_percent: f64,
    },
    Failed {
        throughput_x: f64,
        p999_percent: f64,
    },
    Error {
        message: String,
    },
    Cancelled,
}

/// The sweep over the tiers: what ran, and the settings it ran with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Discovery {
    /// Tier choice key and its state, lowest first.
    pub tiers: Vec<(String, TierState)>,
    /// Progress of the running tier, `0.0..=1.0`.
    pub progress: f64,
    pub active: bool,
    /// The quality-relevant settings the sweep ran with.
    basis: BTreeMap<String, Value>,
}

impl Discovery {
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }

    fn state(&self, key: &str) -> Option<&TierState> {
        self.tiers.iter().find(|(k, _)| k == key).map(|(_, s)| s)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    pos: usize,
    draft: Draft,
    use_case: Option<UseCase>,
}

/// The step engine.
#[derive(Debug, Clone)]
pub struct Guided {
    def: GuidedDef,
    pos: usize,
    trail: Vec<Snapshot>,
    pub use_case: Option<UseCase>,
    pub discovery: Discovery,
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

/// The values the draft sets, by absolute path.
fn set_values(draft: &Draft) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    if let Ok(t) = draft.values() {
        flatten("", &t, &mut m);
    }
    m
}

/// Settings that decide what a benchmark measures, other than the tier itself.
fn quality_basis(draft: &Draft) -> BTreeMap<String, Value> {
    const KEYS: [&str; 4] = [
        "target.format",
        "target.sample_format",
        "target.dither",
        "output.transport",
    ];
    let mut m = set_values(draft);
    m.retain(|k, _| {
        KEYS.contains(&k.as_str())
            || ["resampler.", "gain.", "runtime.", "input."]
                .iter()
                .any(|p| k.starts_with(p))
    });
    m
}

impl Guided {
    /// Starts at the first step.
    pub fn new() -> Self {
        Self {
            def: GuidedDef::load().expect("guided.toml is valid"),
            pos: 0,
            trail: Vec::new(),
            use_case: None,
            discovery: Discovery::default(),
        }
    }

    pub fn def(&self) -> &GuidedDef {
        &self.def
    }

    pub fn step(&self) -> &StepDef {
        &self.def.steps[self.pos]
    }

    pub fn can_go_back(&self) -> bool {
        !self.trail.is_empty()
    }

    /// Values for conditions: the draft's own, plus the engine's pseudo keys.
    fn resolver(&self, draft: &Draft, probe: Option<&ProbeReport>) -> BTreeMap<String, Value> {
        let mut m = set_values(draft);
        if let Some(u) = self.use_case {
            m.insert(
                "use_case".into(),
                serde_json::to_value(u).expect("a use case is a string"),
            );
        }
        if let (Some(Value::String(dev)), Some(Ok(report))) = (
            m.get("output.device").cloned(),
            probe.map(|p| p.alsa.as_ref()),
        ) {
            if let Some((_, d)) = capability::find_device(report, &dev) {
                if let Some(c) = &d.capabilities {
                    m.insert(
                        "device.native_dsd".into(),
                        Value::Bool(c.native_dsd.supported),
                    );
                    m.insert("device.dop".into(), Value::Bool(c.dop.supported));
                }
            }
        }
        m
    }

    fn skipped(&self, i: usize, r: &BTreeMap<String, Value>) -> bool {
        self.def.steps[i]
            .skip_if
            .as_ref()
            .is_some_and(|c| c.eval(r))
    }

    /// The step's static choices that are visible now (presets become choices).
    fn static_choices(
        &self,
        step: &StepDef,
        catalog: &Catalog,
        r: &BTreeMap<String, Value>,
    ) -> Vec<ChoiceDef> {
        let mut out: Vec<ChoiceDef> = step
            .choices
            .iter()
            .filter(|c| c.show_if.as_ref().is_none_or(|s| s.eval(r)))
            .cloned()
            .collect();
        if let Some(id) = &step.presets {
            if let Some(a) = catalog.assist(id) {
                for p in &a.presets {
                    // A bundle never sets a key that is hidden for this target (the modulator
                    // order on a PCM target).
                    let sets = p
                        .values
                        .iter()
                        .filter(|(k, _)| catalog.is_visible(k, r))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    out.push(ChoiceDef {
                        key: p.key.clone(),
                        label: p.label.clone(),
                        summary: p.summary.clone(),
                        description: None,
                        caution: None,
                        show_if: None,
                        sets,
                        unsets: Vec::new(),
                        use_case: None,
                    });
                }
            }
        }
        out
    }

    /// The recommendation the use case gives for a choice: every key it sets that has one must
    /// match, and at least one must.
    fn recommendation(&self, catalog: &Catalog, c: &ChoiceDef) -> Option<String> {
        let uc = self.use_case?;
        let mut why = None;
        for (path, v) in &c.sets {
            if let Some(r) = catalog.field(path).and_then(|f| f.recommended.get(&uc)) {
                if &r.value != v {
                    return None;
                }
                why.get_or_insert_with(|| r.why.clone());
            }
        }
        why
    }

    /// Applies a choice to the draft; the draft is unchanged on error.
    fn apply(draft: &mut Draft, c: &ChoiceDef) -> Result<(), String> {
        let backup = draft.clone();
        let current = set_values(draft);
        for p in &c.unsets {
            if current.contains_key(p) {
                if let Err(e) = draft.unset(p) {
                    *draft = backup;
                    return Err(format!("{p}: {e}"));
                }
            }
        }
        for (p, v) in &c.sets {
            if let Err(e) = draft.set(p, v) {
                *draft = backup;
                return Err(format!("{p}: {e}"));
            }
        }
        Ok(())
    }

    /// Per-tier capability finding for the tier choice `c` (the key it sets), if any.
    fn tier_finding(
        draft: &Draft,
        probe: Option<&ProbeReport>,
        c: &ChoiceDef,
    ) -> Option<(Severity, String)> {
        let Some(Ok(report)) = probe.map(|p| p.alsa.as_ref()) else {
            return None;
        };
        let mut trial = draft.clone();
        Self::apply(&mut trial, c).ok()?;
        let paths: Vec<&str> = c.sets.keys().map(String::as_str).collect();
        capability::check(trial.settings(), report)
            .into_iter()
            .filter(|f| paths.contains(&f.path))
            .filter(|f| f.severity != Severity::Info)
            .max_by_key(|f| f.severity == Severity::Error)
            .map(|f| (f.severity, f.message))
    }

    fn tier_note(&self, key: &str, stale: bool) -> Option<String> {
        let note = self.tier_state_note(key)?;
        Some(if stale {
            format!("out of date, quality settings changed since: {note}")
        } else {
            note
        })
    }

    /// The label of the rate-step choice `key` (the key itself when there is none).
    pub fn tier_label(&self, key: &str) -> String {
        self.def
            .steps
            .iter()
            .filter(|s| s.kind == StepKind::Tiers)
            .flat_map(|s| &s.choices)
            .find(|c| c.key == key)
            .map_or_else(|| key.to_string(), |c| c.label.clone())
    }

    fn tier_state_note(&self, key: &str) -> Option<String> {
        Some(match self.discovery.state(key)? {
            TierState::Untested => return None,
            TierState::Running => "measuring...".to_string(),
            TierState::Passed {
                throughput_x,
                p999_percent,
            } => format!(
                "passed the gates: {throughput_x:.1}x real time, p99.9 at {p999_percent:.0}% of the block budget"
            ),
            TierState::Failed {
                throughput_x,
                p999_percent,
            } => format!(
                "fails the gates: {throughput_x:.1}x real time, p99.9 at {p999_percent:.0}% of the block budget"
            ),
            TierState::Error { message: m } => format!("not measured: {m}"),
            TierState::Cancelled => "not measured (cancelled)".to_string(),
        })
    }

    /// Whether the discovery results no longer match the draft's quality settings.
    pub fn discovery_stale(&self, draft: &Draft) -> bool {
        !self.discovery.is_empty() && self.discovery.basis != quality_basis(draft)
    }

    /// The current step as a frontend shows it.
    pub fn view(&self, catalog: &Catalog, draft: &Draft, probe: Option<&ProbeReport>) -> StepView {
        let step = self.step();
        let r = self.resolver(draft, probe);
        let mut choices: Vec<ChoiceView> = Vec::new();
        let mut waiting = false;
        let mut problem = None;
        let probe_choices = |step: &StepDef| -> (Vec<ChoiceView>, bool, Option<String>) {
            let mut v = Vec::new();
            let Some(p) = probe else {
                return (v, true, None);
            };
            match step.kind {
                StepKind::AlsaDevice => match &p.alsa {
                    Ok(rep) => {
                        for c in &rep.cards {
                            for d in c.devices.iter().filter(|d| d.error.is_none()) {
                                let mut s = format!("{} / {}", c.longname, d.name);
                                match &d.capabilities {
                                    Some(cap) => {
                                        if cap.native_dsd.supported {
                                            s.push_str(&format!(
                                                "; native DSD up to {}",
                                                cap.native_dsd
                                                    .max_dsd_tier
                                                    .as_deref()
                                                    .unwrap_or("?")
                                            ));
                                        }
                                        if cap.dop.supported {
                                            s.push_str(&format!(
                                                "; DoP up to {}",
                                                cap.dop.max_dop_tier.as_deref().unwrap_or("?")
                                            ));
                                        }
                                    }
                                    None if d.is_busy => {
                                        s.push_str(" (busy: capabilities unknown)")
                                    }
                                    None => {}
                                }
                                v.push(ChoiceView {
                                    key: d.device_path.clone(),
                                    label: d.device_path.clone(),
                                    summary: s,
                                    description: None,
                                    caution: None,
                                    recommendation: None,
                                    note: None,
                                    disabled: None,
                                });
                            }
                        }
                        (v, false, None)
                    }
                    Err(e) => (v, false, Some(format!("ALSA probe failed: {e}"))),
                },
                StepKind::PipewireSink => match &p.pipewire {
                    Ok(rep) => {
                        for s in &rep.sinks {
                            v.push(ChoiceView {
                                key: s.node_name.clone(),
                                label: s.node_name.clone(),
                                summary: s.description.clone().unwrap_or_default(),
                                description: None,
                                caution: None,
                                recommendation: None,
                                note: None,
                                disabled: None,
                            });
                        }
                        (v, false, None)
                    }
                    Err(e) => (v, false, Some(format!("PipeWire probe failed: {e}"))),
                },
                _ => (v, false, None),
            }
        };
        if matches!(step.kind, StepKind::AlsaDevice | StepKind::PipewireSink) {
            let (c, w, p) = probe_choices(step);
            choices = c;
            waiting = w;
            problem = p;
            if !waiting && problem.is_none() && choices.is_empty() {
                problem = step.empty.clone();
            }
        }
        let stale = self.discovery_stale(draft);
        let statics = self.static_choices(step, catalog, &r);
        for c in &statics {
            let mut v = ChoiceView {
                key: c.key.clone(),
                label: c.label.clone(),
                summary: c.summary.clone(),
                description: c.description.clone(),
                caution: c.caution.clone(),
                recommendation: self.recommendation(catalog, c),
                note: None,
                disabled: None,
            };
            if step.kind == StepKind::Tiers {
                match Self::tier_finding(draft, probe, c) {
                    Some((Severity::Error, m)) => v.disabled = Some(m),
                    Some((_, m)) => v.note = Some(m),
                    None => {}
                }
                if let Some(n) = self.tier_note(&c.key, stale) {
                    v.note = Some(match v.note.take() {
                        Some(old) => format!("{old}; {n}"),
                        None => n,
                    });
                }
            }
            choices.push(v);
        }
        let selected = choices
            .iter()
            .position(|c| c.recommendation.is_some() && c.disabled.is_none())
            .or_else(|| choices.iter().position(|c| c.disabled.is_none()))
            .unwrap_or(0);
        // Steps this run will still ask: not skipped, and not a choice step that has only one
        // visible choice (those answer themselves).
        // Until the output is chosen, count the longer path (ALSA); the total shrinks if the
        // user takes the shorter one.
        let mut counting = r.clone();
        counting
            .entry("output.backend".into())
            .or_insert_with(|| Value::String("alsa".into()));
        let remaining = (self.pos + 1..self.def.steps.len())
            .filter(|&i| !self.skipped(i, &counting))
            .filter(|&i| {
                let s = &self.def.steps[i];
                s.kind != StepKind::Choice || self.static_choices(s, catalog, &counting).len() > 1
            })
            .count();
        let number = self.trail.len() + 1;
        StepView {
            id: step.id.clone(),
            title: step.title.clone(),
            prompt: step.prompt.clone(),
            kind: step.kind,
            choices,
            selected,
            number,
            total: number + remaining,
            waiting_for_probe: waiting,
            problem,
        }
    }

    // ---- moving -----------------------------------------------------------------------

    /// Answers the current step with the choice `key` (a probed device or sink included).
    pub fn answer(
        &mut self,
        catalog: &Catalog,
        draft: &mut Draft,
        probe: Option<&ProbeReport>,
        key: &str,
    ) -> Result<Outcome, String> {
        let step = self.step().clone();
        let r = self.resolver(draft, probe);
        let snapshot = Snapshot {
            pos: self.pos,
            draft: draft.clone(),
            use_case: self.use_case,
        };
        let statics = self.static_choices(&step, catalog, &r);
        let chosen = statics.iter().find(|c| c.key == key);
        match step.kind {
            StepKind::Discover => {
                if key == "run" {
                    return self.start_discovery(catalog, draft, probe);
                }
            }
            StepKind::AlsaDevice | StepKind::PipewireSink if chosen.is_none() => {
                let path = if step.kind == StepKind::AlsaDevice {
                    "output.device"
                } else {
                    "output.target_node"
                };
                draft
                    .set(path, &Value::String(key.to_string()))
                    .map_err(|e| format!("{path}: {e}"))?;
            }
            _ => {
                let c = chosen.ok_or_else(|| format!("`{key}` is not a choice of this step"))?;
                if step.kind == StepKind::Tiers {
                    let view = self.view(catalog, draft, probe);
                    if let Some(why) = view
                        .choices
                        .iter()
                        .find(|v| v.key == key)
                        .and_then(|v| v.disabled.clone())
                    {
                        return Err(why);
                    }
                }
                Self::apply(draft, c)?;
                if let Some(u) = c.use_case {
                    self.use_case = Some(u);
                }
            }
        }
        self.trail.push(snapshot);
        self.discovery.active = false;
        Ok(self.seek(catalog, draft, probe, self.pos + 1))
    }

    /// Goes back to the previous step the user answered, undoing everything since.
    pub fn back(&mut self, draft: &mut Draft) -> bool {
        let Some(s) = self.trail.pop() else {
            return false;
        };
        self.pos = s.pos;
        *draft = s.draft;
        self.use_case = s.use_case;
        self.discovery.active = false;
        true
    }

    /// Finds the next step to ask, answering the single-choice ones on the way.
    fn seek(
        &mut self,
        catalog: &Catalog,
        draft: &mut Draft,
        probe: Option<&ProbeReport>,
        from: usize,
    ) -> Outcome {
        for i in from..self.def.steps.len() {
            let r = self.resolver(draft, probe);
            if self.skipped(i, &r) {
                continue;
            }
            let step = self.def.steps[i].clone();
            if step.kind == StepKind::Choice {
                let visible = self.static_choices(&step, catalog, &r);
                if visible.is_empty() {
                    continue;
                }
                // A key the draft cannot take leaves the step unanswered.
                if visible.len() == 1 && Self::apply(draft, &visible[0]).is_ok() {
                    continue;
                }
            }
            self.pos = i;
            return if matches!(step.kind, StepKind::AlsaDevice | StepKind::PipewireSink) {
                Outcome::NeedsProbe
            } else {
                Outcome::Next
            };
        }
        self.apply_recommendations(catalog, draft, probe);
        Outcome::Done
    }

    /// Fills every visible key no step touched with the use case's recommended value.
    fn apply_recommendations(
        &self,
        catalog: &Catalog,
        draft: &mut Draft,
        probe: Option<&ProbeReport>,
    ) {
        let Some(uc) = self.use_case else {
            return;
        };
        for f in catalog.fields() {
            let Some(rec) = f.recommended.get(&uc) else {
                continue;
            };
            if draft.is_touched(&f.path) {
                continue;
            }
            let r = self.resolver(draft, probe);
            if catalog.is_visible(&f.path, &r) {
                let _ = draft.set(&f.path, &rec.value);
            }
        }
    }

    // ---- discovery --------------------------------------------------------------------

    /// The draft with the tier choice `key` of the `rate` step applied.
    fn tier_settings(
        &self,
        catalog: &Catalog,
        draft: &Draft,
        probe: Option<&ProbeReport>,
        key: &str,
    ) -> Option<Settings> {
        let step = self.def.steps.iter().find(|s| s.kind == StepKind::Tiers)?;
        let r = self.resolver(draft, probe);
        let c = self
            .static_choices(step, catalog, &r)
            .into_iter()
            .find(|c| c.key == key)?;
        let mut trial = draft.clone();
        Self::apply(&mut trial, &c).ok()?;
        Some(trial.settings().clone())
    }

    fn start_discovery(
        &mut self,
        catalog: &Catalog,
        draft: &Draft,
        probe: Option<&ProbeReport>,
    ) -> Result<Outcome, String> {
        let tiers_step = self
            .def
            .steps
            .iter()
            .position(|s| s.kind == StepKind::Tiers)
            .ok_or("the flow has no rate step")?;
        let saved = self.pos;
        self.pos = tiers_step;
        let view = self.view(catalog, draft, probe);
        self.pos = saved;
        let keys: Vec<String> = view
            .choices
            .into_iter()
            .filter(|c| c.disabled.is_none())
            .map(|c| c.key)
            .collect();
        if keys.is_empty() {
            return Err("the device supports none of the tiers".into());
        }
        let first = self
            .tier_settings(catalog, draft, probe, &keys[0])
            .ok_or("the first tier cannot be applied to the draft")?;
        self.discovery = Discovery {
            tiers: keys.into_iter().map(|k| (k, TierState::Untested)).collect(),
            progress: 0.0,
            active: true,
            basis: quality_basis(draft),
        };
        self.discovery.tiers[0].1 = TierState::Running;
        Ok(Outcome::StartBenchmark(Box::new(first)))
    }

    /// Feeds one benchmark event of the sweep.
    pub fn on_discovery(
        &mut self,
        catalog: &Catalog,
        draft: &Draft,
        probe: Option<&ProbeReport>,
        ev: &BenchmarkEvent,
    ) -> DiscoveryNext {
        if !self.discovery.active {
            return DiscoveryNext::Nothing;
        }
        let Some(i) = self
            .discovery
            .tiers
            .iter()
            .position(|(_, s)| *s == TierState::Running)
        else {
            return DiscoveryNext::Nothing;
        };
        match ev {
            BenchmarkEvent::Progress { fraction, .. } => {
                self.discovery.progress = *fraction;
                DiscoveryNext::Nothing
            }
            BenchmarkEvent::Finished(res) => {
                let passed = Self::record(res);
                self.discovery.tiers[i].1 = passed;
                self.discovery.progress = 0.0;
                let ok = matches!(self.discovery.tiers[i].1, TierState::Passed { .. });
                if ok && i + 1 < self.discovery.tiers.len() {
                    let key = self.discovery.tiers[i + 1].0.clone();
                    match self.tier_settings(catalog, draft, probe, &key) {
                        Some(s) => {
                            self.discovery.tiers[i + 1].1 = TierState::Running;
                            return DiscoveryNext::StartBenchmark(Box::new(s));
                        }
                        None => {
                            self.discovery.tiers[i + 1].1 = TierState::Error {
                                message: "cannot be applied to the draft".into(),
                            };
                        }
                    }
                }
                self.discovery.active = false;
                DiscoveryNext::Finished
            }
            BenchmarkEvent::Failed(m) => {
                self.discovery.tiers[i].1 = TierState::Error { message: m.clone() };
                self.discovery.active = false;
                DiscoveryNext::Finished
            }
            BenchmarkEvent::Cancelled => {
                self.discovery.tiers[i].1 = TierState::Cancelled;
                self.discovery.active = false;
                DiscoveryNext::Finished
            }
        }
    }

    fn record(r: &BenchmarkResult) -> TierState {
        let (throughput_x, p999_percent) = (r.worker_throughput_x, r.p999_budget_percent);
        if r.passed() {
            TierState::Passed {
                throughput_x,
                p999_percent,
            }
        } else {
            TierState::Failed {
                throughput_x,
                p999_percent,
            }
        }
    }
}

impl Default for Guided {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::benchmark::{Gates, Stage, Tier};
    use audigma_io::alsa::probe::{DeviceCapabilities, DopSupport, NativeDsdSupport};
    use audigma_io::{
        AlsaCardReport, AlsaDeviceReport, AlsaProbeReport, PipeWireProbeReport, PipeWireSinkInfo,
    };
    use audigma_settings::meta::FieldKind;
    use serde_json::json;

    fn catalog() -> Catalog {
        Catalog::load().unwrap()
    }

    fn caps(rates: &[u32], native: bool, dop: bool) -> DeviceCapabilities {
        DeviceCapabilities {
            formats: vec!["S32_LE".into()],
            rates_hz: rates.to_vec(),
            min_channels: 2,
            max_channels: 2,
            native_dsd: NativeDsdSupport {
                supported: native,
                formats: vec![],
                max_frame_rate_hz: None,
                max_dsd_tier: None,
                dsd_raw: None,
            },
            dop: DopSupport {
                supported: dop,
                max_dop_tier: None,
                supported_containers: vec![],
            },
            usb_altsets: vec![],
        }
    }

    fn alsa_probe(c: DeviceCapabilities) -> ProbeReport {
        ProbeReport {
            alsa: Ok(AlsaProbeReport {
                cards: vec![AlsaCardReport {
                    index: 2,
                    id: "K17".into(),
                    name: "USB-Audio".into(),
                    longname: "FIIO K17".into(),
                    usb_id: None,
                    devices: vec![AlsaDeviceReport {
                        card_index: 2,
                        device_index: 0,
                        id: "USB Audio".into(),
                        name: "USB Audio".into(),
                        device_path: "hw:CARD=K17,DEV=0".into(),
                        is_busy: false,
                        active_status: None,
                        capabilities: Some(c),
                        error: None,
                    }],
                }],
            }),
            pipewire: Err("none".into()),
        }
    }

    fn pw_probe() -> ProbeReport {
        ProbeReport {
            alsa: Err("none".into()),
            pipewire: Ok(PipeWireProbeReport {
                sinks: vec![PipeWireSinkInfo {
                    id: 1,
                    node_name: "alsa_output.speaker".into(),
                    description: Some("Speaker".into()),
                    device_api: None,
                    alsa_card: None,
                    alsa_card_name: None,
                    alsa_path: None,
                }],
            }),
        }
    }

    struct Run {
        g: Guided,
        cat: Catalog,
        draft: Draft,
        probe: Option<ProbeReport>,
    }

    impl Run {
        fn new(probe: Option<ProbeReport>) -> Self {
            Self {
                g: Guided::new(),
                cat: catalog(),
                draft: Draft::empty(),
                probe,
            }
        }
        fn answer(&mut self, key: &str) -> Outcome {
            self.g
                .answer(&self.cat, &mut self.draft, self.probe.as_ref(), key)
                .unwrap_or_else(|e| panic!("answer {key}: {e}"))
        }
        fn view(&self) -> StepView {
            self.g.view(&self.cat, &self.draft, self.probe.as_ref())
        }
        fn step(&self) -> String {
            self.g.step().id.clone()
        }
        fn get(&self, path: &str) -> Option<Value> {
            set_values(&self.draft).get(path).cloned()
        }
    }

    fn pcm_rate(s: &Settings) -> i64 {
        let t = toml::Table::try_from(s).unwrap();
        t["target"]["pcm_rate_hz"].as_integer().unwrap()
    }

    fn result(pass: bool) -> Box<BenchmarkResult> {
        Box::new(BenchmarkResult {
            filter: "f".into(),
            input_rate_hz: 44100,
            target_format: "Pcm".into(),
            worker_throughput_x: if pass { 5.0 } else { 1.2 },
            sequential_throughput_x: 1.0,
            speedup_factor: 1.0,
            parallel_efficiency_percent: 1.0,
            tier: Tier::Optimal,
            end_to_end_throughput_x: 1.0,
            audio_duration_sec: 1.0,
            block_frames: 64,
            block_budget_micros: 1.0,
            avg_block_micros: 1.0,
            p50_block_micros: 1.0,
            p99_block_micros: 1.0,
            p999_block_micros: 1.0,
            p999_budget_percent: 10.0,
            gates: Gates::evaluate(if pass { 5.0 } else { 1.2 }, 10.0),
        })
    }

    // ---- the data ------------------------------------------------------------------

    #[test]
    fn the_flow_matches_the_catalog() {
        let def = GuidedDef::load().unwrap();
        let cat = catalog();
        let mut ids = std::collections::BTreeSet::new();
        for s in &def.steps {
            assert!(ids.insert(s.id.clone()), "duplicate step {}", s.id);
            assert!(s.prompt.trim().len() > 10, "{}: prompt", s.id);
            let mut keys = std::collections::BTreeSet::new();
            let mut conds: Vec<&Condition> = s.skip_if.iter().collect();
            for c in &s.choices {
                assert!(keys.insert(&c.key), "{}: duplicate choice {}", s.id, c.key);
                assert!(!c.summary.trim().is_empty() && !c.label.trim().is_empty());
                conds.extend(c.show_if.iter());
                for p in c.sets.keys().chain(&c.unsets) {
                    let f = cat
                        .field(p)
                        .unwrap_or_else(|| panic!("{}: unknown key {p}", s.id));
                    assert_eq!(f.kind, FieldKind::Value, "{p}");
                }
                // Every set value is accepted by the draft on its own.
                let mut d = Draft::empty();
                d.set("target.format", &json!("pcm")).unwrap();
                for (p, v) in &c.sets {
                    d.set(p, v)
                        .unwrap_or_else(|e| panic!("{}/{}: {p}: {e}", s.id, c.key));
                }
            }
            for cond in conds {
                for (key, _) in cond.comparisons() {
                    assert!(
                        PSEUDO_KEYS.contains(&key) || cat.field(key).is_some(),
                        "{}: condition on unknown key {key}",
                        s.id
                    );
                }
            }
            if s.kind == StepKind::Presets {
                let a = cat.assist(s.presets.as_deref().unwrap()).unwrap();
                assert!(!a.presets.is_empty());
            }
        }
        assert_eq!(def.steps[0].id, "use-case");
        assert!(
            def.steps[0].choices.iter().all(|c| c.use_case.is_some()),
            "every first-step choice picks a use case"
        );
    }

    // ---- flows ---------------------------------------------------------------------

    #[test]
    fn offline_asks_the_format_and_the_rate_only() {
        let mut r = Run::new(None);
        assert_eq!(r.view().number, 1);
        r.answer("offline-conversion");
        assert_eq!(r.step(), "offline-format");
        assert_eq!(r.view().total, 3);
        r.answer("dsd");
        assert_eq!(r.step(), "rate");
        let v = r.view();
        assert!(v.choices.iter().all(|c| c.key.starts_with("dsd")));
        assert_eq!(r.answer("dsd256"), Outcome::Done);
        assert_eq!(r.get("target.format"), Some(json!("dsd")));
        assert_eq!(r.get("target.dsd_rate"), Some(json!("dsd256")));
        // The offline recommendations fill the keys no step asked about.
        assert_eq!(r.get("gain.peak_analysis"), Some(json!("two-pass")));
        assert_eq!(r.get("output.backend"), None);
    }

    #[test]
    fn pipewire_output_is_pcm_without_asking_the_format() {
        let mut r = Run::new(Some(pw_probe()));
        r.answer("interactive-desktop");
        assert_eq!(r.step(), "output-path");
        assert_eq!(r.answer("pipewire"), Outcome::NeedsProbe);
        assert_eq!(r.step(), "pipewire-sink");
        let v = r.view();
        assert_eq!(v.choices[0].key, "alsa_output.speaker");
        assert_eq!(v.choices.last().unwrap().key, "default");
        // The format step is answered by its only visible choice.
        assert_eq!(r.answer("alsa_output.speaker"), Outcome::Next);
        assert_eq!(r.step(), "quality");
        assert_eq!(r.get("target.format"), Some(json!("pcm")));
        r.answer("apodizing");
        assert_eq!(r.get("resampler.phase"), Some(json!("minimum")));
        // PCM target: the preset leaves the DSD-only modulator order alone.
        assert!(!r.draft.is_touched("resampler.modulator_order"));
        assert_eq!(r.step(), "rate-policy");
        r.answer("source");
        assert_eq!(r.step(), "discover");
        r.answer("skip");
        assert_eq!(r.step(), "rate");
        // Volume is skipped on PipeWire.
        assert_eq!(r.answer("176400"), Outcome::Done);
        assert_eq!(
            r.get("output.target_node"),
            Some(json!("alsa_output.speaker"))
        );
        assert_eq!(r.get("target.pcm_rate_hz"), Some(json!(176400)));
        assert_eq!(r.get("output.volume_control"), None);
        // Recommendations fill what no step asked about, as touched keys.
        assert_eq!(r.get("output.keep_audio_sink_ms"), Some(json!(2000)));
        assert!(r.draft.is_touched("output.keep_audio_sink_ms"));
        assert!(!r.draft.has_errors(), "{:?}", r.draft.issues());
    }

    #[test]
    fn alsa_flow_offers_only_what_the_device_supports() {
        let rates = [44100, 48000, 88200, 96000, 176400, 192000];
        let mut r = Run::new(Some(alsa_probe(caps(&rates, true, false))));
        r.answer("headless-appliance");
        assert_eq!(r.answer("alsa"), Outcome::NeedsProbe);
        assert_eq!(r.step(), "alsa-device");
        assert_eq!(r.view().choices[0].key, "hw:CARD=K17,DEV=0");
        r.answer("hw:CARD=K17,DEV=0");
        assert_eq!(r.step(), "format");
        let keys: Vec<_> = r.view().choices.into_iter().map(|c| c.key).collect();
        assert_eq!(keys, ["pcm", "dsd-native"], "no DoP: the device lacks it");
        r.answer("pcm");
        assert_eq!(r.get("output.transport"), None);
        r.answer("neutral");
        r.answer("source");
        r.answer("skip");
        let v = r.view();
        let c = |k: &str| v.choices.iter().find(|c| c.key == k).unwrap();
        assert!(c("176400").disabled.is_none());
        assert!(c("352800").disabled.is_some(), "above the device's rates");
        let err =
            r.g.answer(&r.cat, &mut r.draft, r.probe.as_ref(), "352800")
                .unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(r.step(), "rate", "a refused answer stays on the step");
        r.answer("176400");
        assert_eq!(r.step(), "volume");
        assert_eq!(r.answer("software"), Outcome::Done);
        assert_eq!(r.get("output.volume_control"), Some(json!("software")));
        assert_eq!(r.get("output.device"), Some(json!("hw:CARD=K17,DEV=0")));
        assert_eq!(r.get("input.allow_dsd"), Some(json!(true)));
        assert!(!r.draft.has_errors(), "{:?}", r.draft.issues());
    }

    #[test]
    fn dsd_choices_and_the_modulator_order_follow_the_format() {
        let mut r = Run::new(Some(alsa_probe(caps(&[44100], true, true))));
        r.answer("interactive-desktop");
        r.answer("alsa");
        r.answer("later");
        let keys: Vec<_> = r.view().choices.into_iter().map(|c| c.key).collect();
        assert_eq!(keys, ["pcm", "dsd-native", "dsd-dop"]);
        r.answer("dsd-dop");
        assert_eq!(r.get("output.transport"), Some(json!("dop")));
        r.answer("low-latency");
        assert_eq!(r.get("resampler.modulator_order"), Some(json!(5)));
    }

    #[test]
    fn the_use_case_recommendation_is_tagged_and_preselected() {
        let mut r = Run::new(None);
        r.answer("interactive-desktop");
        let v = r.view();
        let alsa = v.choices.iter().position(|c| c.key == "alsa").unwrap();
        assert!(v.choices[alsa].recommendation.is_some());
        assert_eq!(v.selected, alsa);
    }

    #[test]
    fn back_restores_the_draft_and_the_use_case() {
        let mut r = Run::new(None);
        assert!(!r.g.can_go_back());
        r.answer("interactive-desktop");
        r.answer("alsa");
        assert_eq!(r.get("output.backend"), Some(json!("alsa")));
        assert!(r.g.back(&mut r.draft));
        assert_eq!(r.step(), "output-path");
        assert_eq!(r.get("output.backend"), None);
        assert!(r.g.back(&mut r.draft));
        assert_eq!(r.step(), "use-case");
        assert_eq!(r.g.use_case, None);
        assert!(!r.g.back(&mut r.draft));
        // A different answer on the way forward takes the other branch.
        r.answer("offline-conversion");
        assert_eq!(r.step(), "offline-format");
    }

    #[test]
    fn progress_counts_the_steps_this_run_asks() {
        let mut r = Run::new(Some(pw_probe()));
        r.answer("headless-appliance");
        r.answer("pipewire");
        let v = r.view();
        assert_eq!((v.number, v.total), (3, 7));
        // Before the output is chosen the longer path is counted.
        let r = Run::new(None);
        assert_eq!(r.view().total, 9);
    }

    // ---- discovery -----------------------------------------------------------------

    fn at_discover(rates: &[u32]) -> Run {
        let mut r = Run::new(Some(alsa_probe(caps(rates, false, false))));
        r.answer("headless-appliance");
        r.answer("alsa");
        r.answer("hw:CARD=K17,DEV=0");
        r.answer("neutral");
        r.answer("source");
        assert_eq!(r.step(), "discover");
        r
    }

    fn feed(r: &mut Run, ev: BenchmarkEvent) -> DiscoveryNext {
        r.g.on_discovery(&r.cat, &r.draft, r.probe.as_ref(), &ev)
    }

    #[test]
    fn discovery_sweeps_up_and_stops_at_the_first_failure() {
        let mut r = at_discover(&[44100, 48000, 88200, 96000, 176400, 192000]);
        let Outcome::StartBenchmark(first) = r.answer("run") else {
            panic!("expected a benchmark");
        };
        assert_eq!(pcm_rate(&first), 44100);
        let keys: Vec<_> = r.g.discovery.tiers.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, ["44100", "88200", "176400"], "supported tiers only");
        assert!(r.g.discovery.active);
        let ev = BenchmarkEvent::Progress {
            stage: Stage::Workers,
            fraction: 0.5,
        };
        assert_eq!(feed(&mut r, ev), DiscoveryNext::Nothing);
        assert_eq!(r.g.discovery.progress, 0.5);
        let next = feed(&mut r, BenchmarkEvent::Finished(result(true)));
        let DiscoveryNext::StartBenchmark(s) = next else {
            panic!("expected the next tier");
        };
        assert_eq!(pcm_rate(&s), 88200);
        let next = feed(&mut r, BenchmarkEvent::Finished(result(false)));
        assert_eq!(next, DiscoveryNext::Finished);
        assert!(!r.g.discovery.active);
        let states: Vec<_> = r.g.discovery.tiers.iter().map(|(_, s)| s.clone()).collect();
        assert!(matches!(states[0], TierState::Passed { .. }));
        assert!(matches!(states[1], TierState::Failed { .. }));
        assert_eq!(states[2], TierState::Untested, "never run after a failure");
        r.answer("continue");
        assert_eq!(r.step(), "rate");
        let v = r.view();
        let note = |k: &str| v.choices.iter().find(|c| c.key == k).unwrap().note.clone();
        assert!(note("44100").unwrap().contains("passed"));
        assert!(note("88200").unwrap().contains("fails"));
        assert!(note("176400").is_none());
    }

    #[test]
    fn a_cancelled_sweep_keeps_what_it_measured() {
        let mut r = at_discover(&[44100, 48000, 88200, 96000]);
        r.answer("run");
        feed(&mut r, BenchmarkEvent::Finished(result(true)));
        feed(&mut r, BenchmarkEvent::Cancelled);
        assert!(!r.g.discovery.active);
        assert!(matches!(r.g.discovery.tiers[0].1, TierState::Passed { .. }));
        assert_eq!(r.g.discovery.tiers[1].1, TierState::Cancelled);
        // Events after the sweep are ignored.
        assert_eq!(
            feed(&mut r, BenchmarkEvent::Finished(result(true))),
            DiscoveryNext::Nothing
        );
    }

    #[test]
    fn results_go_stale_when_the_quality_settings_change() {
        let mut r = at_discover(&[44100, 48000]);
        r.answer("run");
        feed(&mut r, BenchmarkEvent::Finished(result(true)));
        assert!(!r.g.discovery_stale(&r.draft));
        r.draft.set("resampler.phase", &json!("minimum")).unwrap();
        assert!(r.g.discovery_stale(&r.draft));
        // The rate itself is not a quality setting.
        let mut r = at_discover(&[44100, 48000]);
        r.answer("run");
        feed(&mut r, BenchmarkEvent::Finished(result(true)));
        r.draft.set("target.pcm_rate_hz", &json!(44100)).unwrap();
        assert!(!r.g.discovery_stale(&r.draft));
    }
}
