//! Authentication, request checks and the JSON API.
//!
//! [`handle`] is a plain function from [`Request`] to [`Response`] over [`Shared`], so the
//! tests call it without sockets; `mod.rs` only moves bytes.
//!
//! `POST /api/load` takes `{text, name?}` (the text of an uploaded TOML file) and replaces the
//! draft with it (200 with the new `DraftView`; 409 while a benchmark or guided setup runs; 422
//! with the parse error otherwise). Guided setup is refused after a load.
//!
//! `POST /api/preview` takes the body of `POST /api/save` and answers with the file text a save
//! would write (`path`, `exists`, `existing`, `text`, `unchanged`, `filename`), going through the
//! same prepare step as the save, but writing nothing, not stopping the server and not marking
//! the session saved. It is allowed while a benchmark runs (the draft cannot change then and
//! nothing is ended); errors come back exactly as from save (422 with `error`/`path`).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use audigma_io::{AlsaProbeReport, PipeWireProbeReport};
use audigma_settings::Settings;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::http::{Request, Response};
use super::jobs::{self, Job, JobKind};
use super::session::{
    ApiError, DraftEdit, DraftView, GuidedNext, GuidedState, LoadRequest, PreviewResponse,
    SaveRequest, SaveResponse, Session,
};
use super::{EndReason, Shared};
use crate::assist::capability::CapabilityFinding;
use crate::assist::probe::ProbeReport;
use crate::draft::Issue;
use crate::guided::Outcome;

const PLACEHOLDER: &str = include_str!("placeholder.html");

/// Compares in time that does not depend on where the strings differ.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

fn token_matches(sh: &Shared, req: &Request) -> bool {
    let bearer = req
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "));
    let cookie = req
        .header("cookie")
        .and_then(|c| cookie_value(c, &sh.cookie_name));
    [bearer, cookie, req.query("token")]
        .into_iter()
        .flatten()
        .any(|t| ct_eq(t, &sh.token))
}

/// The `host[:port]` part of an `Origin` value.
fn origin_authority(origin: &str) -> &str {
    origin.split_once("://").map_or(origin, |(_, rest)| rest)
}

fn json_err(status: u16, e: &ApiError) -> Response {
    Response::json(status, e)
}

fn bad(msg: impl Into<String>) -> Response {
    json_err(400, &ApiError::new(msg))
}

fn conflict(e: ApiError) -> Response {
    json_err(409, &e)
}

fn parse<T: DeserializeOwned + Default>(req: &Request) -> Result<T, Response> {
    if req.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(&req.body).map_err(|e| bad(format!("invalid JSON body: {e}")))
}

fn ok<T: Serialize>(v: &T) -> Response {
    Response::json(200, v)
}

/// What `GET /api/probe/alsa` returns: the report, or why the probe failed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AlsaProbeResponse {
    pub report: Option<AlsaProbeReport>,
    pub error: Option<String>,
}

/// What `GET /api/probe/pipewire` returns: the report, or why the probe failed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PipeWireProbeResponse {
    pub report: Option<PipeWireProbeReport>,
    pub error: Option<String>,
}

/// What `POST /api/validate` returns.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ValidateResponse {
    /// No error blocks saving.
    pub ok: bool,
    pub issues: Vec<Issue>,
    pub findings: Vec<CapabilityFinding>,
}

/// Body of `POST /api/guided/answer`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuidedAnswer {
    pub key: String,
}

/// What the guided endpoints return.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GuidedResponse {
    pub next: GuidedNext,
    /// Set with `next = "benchmark"`: follow `/api/benchmark/{id}/events`.
    pub benchmark_id: Option<u64>,
    pub state: GuidedState,
    /// Set when the flow is over.
    pub draft: Option<DraftView>,
}

/// What `POST /api/benchmark` and the discovery start return.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct JobStarted {
    pub id: u64,
}

/// Every request body the API takes: the source of `api.requests.schema.json`.
#[derive(Debug, JsonSchema)]
pub struct ApiRequests {
    pub draft_edit: DraftEdit,
    pub guided_answer: GuidedAnswer,
    pub save: SaveRequest,
    pub load: LoadRequest,
}

/// Every response body the API gives, plus the benchmark event stream: the source of
/// `api.responses.schema.json`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiResponses {
    pub draft: DraftView,
    pub validate: ValidateResponse,
    pub probe_alsa: AlsaProbeResponse,
    pub probe_pipewire: PipeWireProbeResponse,
    pub job_started: JobStarted,
    pub job_status: jobs::JobStatus,
    pub job_event: jobs::JobEvent,
    pub guided_state: GuidedState,
    pub guided: GuidedResponse,
    pub save: SaveResponse,
    pub preview: PreviewResponse,
    pub error: ApiError,
}

/// Handles one request. Never panics on bad input: every failure is a JSON error body.
pub fn handle(sh: &Arc<Shared>, req: &Request) -> Response {
    sh.touch();
    if !token_matches(sh, req) {
        return json_err(
            401,
            &ApiError::new("missing or wrong token; open the URL audigma printed"),
        );
    }
    let safe = matches!(req.method.as_str(), "GET" | "HEAD");
    if !safe {
        if let Some(origin) = req.header("origin") {
            if req.header("host") != Some(origin_authority(origin)) {
                return json_err(403, &ApiError::new("cross-origin request refused"));
            }
        }
    }
    // First visit: trade the URL token for a cookie and drop it from the address bar.
    if req.method == "GET" && !req.path.starts_with("/api/") && req.query("token").is_some() {
        let rest: Vec<String> = req
            .query
            .iter()
            .filter(|(k, _)| k != "token")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let mut location = req.path.clone();
        if !rest.is_empty() {
            location = format!("{location}?{}", rest.join("&"));
        }
        return Response::redirect(&location).with_header(
            "Set-Cookie",
            format!(
                "{}={}; HttpOnly; SameSite=Strict; Path=/",
                sh.cookie_name, sh.token
            ),
        );
    }
    route(sh, req)
}

fn route(sh: &Arc<Shared>, req: &Request) -> Response {
    let segs: Vec<&str> = req.path.trim_matches('/').split('/').collect();
    let m = req.method.as_str();
    match (m, segs.as_slice()) {
        ("GET", [""]) => Response::html(PLACEHOLDER),
        ("GET", ["api", "meta"]) => ok(&sh.meta),
        ("GET", ["api", "draft"]) => ok(&sh.session().draft_view()),
        ("PUT", ["api", "draft"]) => put_draft(sh, req),
        ("POST", ["api", "validate"]) => {
            let s = sh.session();
            ok(&ValidateResponse {
                ok: !s.draft.has_errors(),
                issues: s.draft.issues(),
                findings: s.findings(),
            })
        }
        ("GET", ["api", "probe", which]) => probe(sh, which),
        ("POST", ["api", "benchmark"]) => start_validation(sh),
        ("GET", ["api", "benchmark", id]) => with_job(sh, id, |j| ok(&j.status())),
        ("GET", ["api", "benchmark", id, "events"]) => {
            with_job(sh, id, |j| Response::events(Arc::clone(j)))
        }
        ("DELETE", ["api", "benchmark", id]) => with_job(sh, id, |j| {
            j.cancel();
            Response::empty(202)
        }),
        ("GET", ["api", "guided", "state"]) => ok(&sh.session().guided_state()),
        ("POST", ["api", "guided", "start"]) => guided_start(sh),
        ("POST", ["api", "guided", "answer"]) => guided_answer(sh, req),
        ("POST", ["api", "guided", "back"]) => guided_move(sh, Session::guided_back),
        ("POST", ["api", "guided", "skip"]) => guided_move(sh, Session::guided_skip),
        ("POST", ["api", "load"]) => load(sh, req),
        ("POST", ["api", "save"]) => save(sh, req),
        ("POST", ["api", "preview"]) => preview(sh, req),
        ("POST", ["api", "quit"]) => {
            sh.finish(EndReason::Quit);
            let mut r = Response::empty(202);
            r.then_shutdown = true;
            r
        }
        (_, ["" | "api", ..]) if is_known(&req.path) => json_err(
            405,
            &ApiError::new(format!("{m} is not allowed on {}", req.path)),
        ),
        _ => json_err(404, &ApiError::new("not found")),
    }
}

fn is_known(path: &str) -> bool {
    let p = path.trim_end_matches('/');
    matches!(
        p,
        "" | "/api/meta"
            | "/api/draft"
            | "/api/validate"
            | "/api/benchmark"
            | "/api/guided/state"
            | "/api/guided/start"
            | "/api/guided/answer"
            | "/api/guided/back"
            | "/api/guided/skip"
            | "/api/load"
            | "/api/save"
            | "/api/preview"
            | "/api/quit"
    ) || p.starts_with("/api/probe/")
        || p.starts_with("/api/benchmark/")
}

fn with_job(sh: &Shared, id: &str, f: impl FnOnce(&Arc<Job>) -> Response) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return bad("the job id is a number");
    };
    match sh.job(id) {
        Some(j) => f(&j),
        None => json_err(404, &ApiError::new("no such benchmark")),
    }
}

fn put_draft(sh: &Shared, req: &Request) -> Response {
    let edit: DraftEdit = match parse(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let mut s = sh.session();
    match s.edit(&edit) {
        Ok(()) => ok(&s.draft_view()),
        Err(e) if e.path.is_some() => json_err(422, &e),
        Err(e) => conflict(e),
    }
}

fn probe(sh: &Shared, which: &str) -> Response {
    match which {
        "alsa" => {
            let found = sh.backend.probe_alsa();
            sh.session().probe_part(Some(found.clone()), None);
            let (report, error) = split(found);
            ok(&AlsaProbeResponse { report, error })
        }
        "pipewire" => {
            let found = sh.backend.probe_pipewire();
            sh.session().probe_part(None, Some(found.clone()));
            let (report, error) = split(found);
            ok(&PipeWireProbeResponse { report, error })
        }
        _ => json_err(404, &ApiError::new("probes are `alsa` and `pipewire`")),
    }
}

fn split<T>(r: Result<T, String>) -> (Option<T>, Option<String>) {
    match r {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    }
}

/// Starts a job for `settings`: the session turns busy first, so two jobs cannot overlap.
fn start_job(sh: &Arc<Shared>, kind: JobKind, settings: Settings) -> u64 {
    let id = sh.next_job.fetch_add(1, Ordering::Relaxed);
    let job = Job::new(id, kind);
    sh.add_job(Arc::clone(&job));
    sh.session().busy = true;
    jobs::spawn(
        job,
        Arc::clone(&sh.session),
        Arc::clone(&sh.backend),
        settings,
    );
    id
}

fn start_validation(sh: &Arc<Shared>) -> Response {
    let settings = {
        let s = sh.session();
        if s.busy {
            return conflict(ApiError::new("a benchmark is already running"));
        }
        if s.draft.has_errors() {
            return json_err(
                422,
                &ApiError::new("fix the errors before validating on this machine"),
            );
        }
        s.draft.settings().clone()
    };
    Response::json(
        200,
        &JobStarted {
            id: start_job(sh, JobKind::Validate, settings),
        },
    )
}

fn guided_response(
    sh: &Shared,
    next: GuidedNext,
    benchmark_id: Option<u64>,
    finished: bool,
) -> Response {
    let s = sh.session();
    ok(&GuidedResponse {
        next,
        benchmark_id,
        state: s.guided_state(),
        draft: finished.then(|| s.draft_view()),
    })
}

/// Runs a fresh probe for a step that needs one and keeps the report.
fn probe_for_guided(sh: &Shared) {
    let report = ProbeReport {
        alsa: sh.backend.probe_alsa(),
        pipewire: sh.backend.probe_pipewire(),
    };
    sh.session().probe = Some(report);
}

fn guided_start(sh: &Arc<Shared>) -> Response {
    if let Err(e) = sh.session().guided_start() {
        return conflict(e);
    }
    let mut next = GuidedNext::Step;
    if sh.session().guided_waits_for_probe() {
        probe_for_guided(sh);
        next = GuidedNext::Probed;
    }
    guided_response(sh, next, None, false)
}

fn guided_move(sh: &Arc<Shared>, f: fn(&mut Session) -> Result<(), ApiError>) -> Response {
    if let Err(e) = f(&mut sh.session()) {
        return conflict(e);
    }
    let mut next = GuidedNext::Step;
    if sh.session().guided_waits_for_probe() {
        probe_for_guided(sh);
        next = GuidedNext::Probed;
    }
    guided_response(sh, next, None, false)
}

fn guided_answer(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: GuidedAnswer = match parse(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let outcome = sh.session().guided_answer(&body.key);
    match outcome {
        Err(e) => conflict(e),
        Ok(Outcome::Next) => guided_response(sh, GuidedNext::Step, None, false),
        Ok(Outcome::NeedsProbe) => {
            probe_for_guided(sh);
            guided_response(sh, GuidedNext::Probed, None, false)
        }
        Ok(Outcome::StartBenchmark(settings)) => {
            let id = start_job(sh, JobKind::Discover, *settings);
            guided_response(sh, GuidedNext::Benchmark, Some(id), false)
        }
        Ok(Outcome::Done) => {
            let next = sh.session().after_done();
            guided_response(sh, next, None, true)
        }
    }
}

/// Replaces the draft with uploaded TOML text. 409 while a job or guided setup runs, 422 when
/// the text is not a valid config (the message carries the line and column or the key).
fn load(sh: &Shared, req: &Request) -> Response {
    let body: LoadRequest = match parse(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut s = sh.session();
    let busy = s.busy || s.guided.is_some();
    match s.load_text(&body) {
        Ok(()) => ok(&s.draft_view()),
        Err(e) if busy => conflict(e),
        Err(e) => json_err(422, &e),
    }
}

/// Read-only: no `busy` check (a running benchmark cannot change the draft, and nothing is
/// written or ended), no write, no shutdown, the session is not marked saved.
fn preview(sh: &Shared, req: &Request) -> Response {
    let body: SaveRequest = match parse(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let previewed: Result<PreviewResponse, ApiError> = sh.session().preview(&body);
    match previewed {
        Ok(p) => ok(&p),
        Err(e) => json_err(422, &e),
    }
}

fn save(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: SaveRequest = match parse(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if sh.session().busy {
        return conflict(ApiError::new("a benchmark is running"));
    }
    let saved: Result<SaveResponse, ApiError> = sh.session().save(&body);
    match saved {
        Ok(done) => {
            sh.finish(EndReason::Saved(done.clone()));
            let mut r = ok(&done);
            r.then_shutdown = true;
            r
        }
        Err(e) => json_err(422, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_is_exact() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert!(!ct_eq("", "a"));
        assert!(ct_eq("", ""));
    }

    #[test]
    fn cookies_are_picked_by_name() {
        assert_eq!(cookie_value("a=1; b=2", "b"), Some("2"));
        assert_eq!(cookie_value("a=1", "b"), None);
        assert_eq!(cookie_value("ab=1; b=2", "b"), Some("2"));
    }

    #[test]
    fn origin_authority_drops_the_scheme() {
        assert_eq!(origin_authority("http://127.0.0.1:80"), "127.0.0.1:80");
        assert_eq!(origin_authority("https://a.lan"), "a.lan");
        assert_eq!(origin_authority("null"), "null");
    }
}
