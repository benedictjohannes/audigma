//! Web backend: the JSON API over a real socket, with fake probes and a tiny benchmark.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use audigma_configurator::assist::benchmark::{
    self, BenchmarkHandle, BenchmarkParams, BenchmarkRequest,
};
use audigma_configurator::web::{Backend, EndReason, ServeOptions, Server, Session, is_exposed};
use audigma_io::{AlsaProbeReport, PipeWireProbeReport, PipeWireSinkInfo};
use audigma_settings::Settings;
use serde_json::{Value, json};

/// Fake probes, and benchmarks shrunk to about a second.
struct Fake;

impl Backend for Fake {
    fn probe_alsa(&self) -> Result<AlsaProbeReport, String> {
        Ok(AlsaProbeReport { cards: vec![] })
    }
    fn probe_pipewire(&self) -> Result<PipeWireProbeReport, String> {
        Ok(PipeWireProbeReport {
            sinks: vec![PipeWireSinkInfo {
                id: 1,
                node_name: "alsa_output.speaker".into(),
                description: Some("Speaker".into()),
                device_api: None,
                alsa_card: None,
                alsa_card_name: None,
                alsa_path: None,
            }],
        })
    }
    fn start_benchmark(&self, settings: Settings) -> BenchmarkHandle {
        let mut req = BenchmarkRequest::new(settings);
        req.params = BenchmarkParams {
            seconds: 1,
            blocks: 250,
        };
        benchmark::start(req)
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn raw(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\n");
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        req.push_str(&format!("Host: {addr}\r\n"));
    }
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").expect("a complete response");
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_string(),
    }
}

struct T {
    server: Server,
    addr: SocketAddr,
    auth: String,
}

const NEW_PATH: &str = "/nonexistent/audigma/config.toml";

fn new_session() -> Session {
    Session::open(Path::new(NEW_PATH)).unwrap()
}

impl T {
    fn new(session: Session) -> Self {
        Self::with(session, &ServeOptions::default())
    }

    fn with(session: Session, opts: &ServeOptions) -> Self {
        let server = Server::start(session, Arc::new(Fake), opts).unwrap();
        let auth = format!("Bearer {}", server.token);
        Self {
            addr: server.addr,
            server,
            auth,
        }
    }

    fn call(&self, method: &str, path: &str, body: &str) -> Reply {
        raw(
            self.addr,
            method,
            path,
            &[("Authorization", &self.auth)],
            body,
        )
    }

    fn get(&self, path: &str) -> Reply {
        self.call("GET", path, "")
    }

    fn put_draft(&self, body: Value) -> Reply {
        self.call("PUT", "/api/draft", &body.to_string())
    }

    fn post(&self, path: &str, body: Value) -> Reply {
        self.call("POST", path, &body.to_string())
    }

    fn answer(&self, key: &str) -> Value {
        let r = self.post("/api/guided/answer", json!({ "key": key }));
        assert_eq!(r.status, 200, "{key}: {}", r.body);
        r.json()
    }

    /// Runs the PipeWire guided flow up to the discover step.
    fn guided_to_discover(&self) {
        assert_eq!(self.post("/api/guided/start", json!({})).status, 200);
        self.answer("interactive-desktop");
        assert_eq!(self.answer("pipewire")["next"], "probed");
        self.answer("alsa_output.speaker");
        self.answer("neutral");
        let s = self.answer("source");
        assert_eq!(s["state"]["step"]["id"], "discover");
    }
}

/// The text of an event stream up to its end.
fn events(t: &T, id: u64) -> String {
    let r = t.get(&format!("/api/benchmark/{id}/events"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/event-stream"));
    r.body
}

fn last_event(stream: &str) -> &str {
    stream
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("event: "))
        .expect("at least one event")
}

const PCM_FILE: &str = "schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 88200\n";

fn existing_session(dir: &Path) -> Session {
    let path = dir.join("config.toml");
    std::fs::write(&path, PCM_FILE).unwrap();
    Session::open(&path).unwrap()
}

#[test]
fn every_request_needs_the_token() {
    let t = T::new(new_session());
    let none = raw(t.addr, "GET", "/api/meta", &[], "");
    assert_eq!(none.status, 401);
    let wrong = raw(
        t.addr,
        "GET",
        "/api/meta",
        &[("Authorization", "Bearer nope")],
        "",
    );
    assert_eq!(wrong.status, 401);
    let q = raw(t.addr, "GET", "/api/meta?token=nope", &[], "");
    assert_eq!(q.status, 401);
    let page = raw(t.addr, "GET", "/", &[], "");
    assert_eq!(page.status, 401);
    let ok = t.get("/api/meta");
    assert_eq!(ok.status, 200);
    assert!(ok.json()["sections"].is_array() || ok.json().is_object());
    // A token in the query works for the API too (curl).
    let via_query = raw(
        t.addr,
        "GET",
        &format!("/api/draft?token={}", t.server.token),
        &[],
        "",
    );
    assert_eq!(via_query.status, 200);
}

#[test]
fn the_url_token_becomes_a_cookie_and_leaves_the_address() {
    let t = T::new(new_session());
    let first = raw(
        t.addr,
        "GET",
        &format!("/?token={}", t.server.token),
        &[],
        "",
    );
    assert_eq!(first.status, 302);
    assert_eq!(first.header("location"), Some("/"));
    let cookie = first.header("set-cookie").unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
    let pair = cookie.split(';').next().unwrap();
    let page = raw(t.addr, "GET", "/", &[("Cookie", pair)], "");
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Audigma configurator"));
    // The cookie also opens the API.
    assert_eq!(
        raw(t.addr, "GET", "/api/draft", &[("Cookie", pair)], "").status,
        200
    );
}

#[test]
fn there_is_no_host_allowlist() {
    let t = T::new(new_session());
    for host in ["example.com", "audigma.lan:8080", "192.168.1.5"] {
        let r = raw(
            t.addr,
            "GET",
            "/api/draft",
            &[("Authorization", &t.auth), ("Host", host)],
            "",
        );
        assert_eq!(r.status, 200, "Host: {host}");
    }
}

#[test]
fn an_origin_must_match_the_host_on_changes_only() {
    let t = T::new(new_session());
    let body = json!({ "use_case": "headless-appliance" }).to_string();
    let put = |origin: &str, host: &str| {
        raw(
            t.addr,
            "PUT",
            "/api/draft",
            &[
                ("Authorization", &t.auth),
                ("Origin", origin),
                ("Host", host),
            ],
            &body,
        )
    };
    assert_eq!(put("https://evil.example", "127.0.0.1:1").status, 403);
    assert_eq!(put("null", "127.0.0.1:1").status, 403);
    // Behind a proxy the origin and the forwarded host agree, whatever they are.
    assert_eq!(put("https://audigma.lan", "audigma.lan").status, 200);
    assert_eq!(put("http://127.0.0.1:1", "127.0.0.1:1").status, 200);
    let get = raw(
        t.addr,
        "GET",
        "/api/draft",
        &[
            ("Authorization", &t.auth),
            ("Origin", "https://evil.example"),
        ],
        "",
    );
    assert_eq!(get.status, 200);
}

#[test]
fn unknown_paths_and_methods_are_refused() {
    let t = T::new(new_session());
    assert_eq!(t.get("/api/nothing").status, 404);
    assert_eq!(t.call("DELETE", "/api/draft", "").status, 405);
    assert_eq!(t.get("/api/probe/usb").status, 404);
    assert_eq!(t.get("/api/benchmark/abc").status, 400);
    assert_eq!(t.get("/api/benchmark/99").status, 404);
    let bad = t.call("PUT", "/api/draft", "{not json");
    assert_eq!(bad.status, 400);
    let unknown = t.put_draft(json!({ "nope": 1 }));
    assert_eq!(unknown.status, 400);
}

#[test]
fn meta_is_the_catalog_bundle() {
    let t = T::new(new_session());
    let meta = t.get("/api/meta").json();
    assert_eq!(meta, audigma_configurator::schema::meta_json());
}

#[test]
fn draft_edits_are_attributed_and_all_or_nothing() {
    let t = T::new(new_session());
    let d = t.get("/api/draft").json();
    assert_eq!(d["is_new"], true);
    assert_eq!(d["exists"], false);
    assert!(
        d["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["error"] == true)
    );

    let ok = t.put_draft(json!({
        "set": { "target.format": "pcm", "target.pcm_rate_hz": 88200 },
        "use_case": "interactive-desktop",
    }));
    assert_eq!(ok.status, 200, "{}", ok.body);
    let d = ok.json();
    assert_eq!(d["use_case"], "interactive-desktop");
    assert_eq!(d["values"]["target"]["pcm_rate_hz"], 88200);
    assert!(
        d["touched"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "target.pcm_rate_hz")
    );

    // The second key is wrong: nothing of the edit stays, and the error names the key.
    let bad = t.put_draft(json!({
        "set": { "output.backend": "pipewire", "target.pcm_rate_hz": "fast" },
    }));
    assert_eq!(bad.status, 422);
    assert_eq!(bad.json()["path"], "target.pcm_rate_hz");
    let d = t.get("/api/draft").json();
    assert_eq!(d["values"]["target"]["pcm_rate_hz"], 88200);
    assert!(d["values"].get("output").is_none(), "{d}");
    assert!(
        !d["touched"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "output.backend")
    );

    let unset = t.put_draft(json!({ "unset": ["target.pcm_rate_hz"], "use_case": null }));
    assert_eq!(unset.status, 200);
    let d = unset.json();
    assert!(d["values"]["target"].get("pcm_rate_hz").is_none());
    assert_eq!(d["use_case"], Value::Null);
}

/// `values` is typed as a `ValueTree` in the schema: objects whose leaves are scalars or lists
/// of scalars.
fn is_value_tree(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.values().all(is_value_tree),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => true,
        Value::Array(a) => a.iter().all(|x| !x.is_object() && !x.is_array()),
        _ => false,
    }
}

#[test]
fn draft_values_have_the_declared_shape() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let d = t.get("/api/draft").json();
    assert!(
        d["values"].is_object() && is_value_tree(&d["values"]),
        "{}",
        d["values"]
    );
    let g = T::new(new_session());
    g.guided_to_discover();
    g.answer("skip");
    let done = g.answer("176400");
    assert!(is_value_tree(&done["draft"]["values"]));
}

#[test]
fn validate_reports_issues_with_their_keys() {
    let t = T::new(new_session());
    let v = t.post("/api/validate", json!({})).json();
    assert_eq!(v["ok"], false);
    t.put_draft(json!({ "set": { "target.format": "pcm", "target.pcm_rate_hz": 88200 } }));
    let v = t.post("/api/validate", json!({})).json();
    assert_eq!(v["ok"], true, "{v}");
}

#[test]
fn probes_use_the_backend_and_feed_the_session() {
    let t = T::new(new_session());
    let alsa = t.get("/api/probe/alsa").json();
    assert_eq!(alsa["error"], Value::Null);
    assert_eq!(alsa["report"]["cards"], json!([]));
    let pw = t.get("/api/probe/pipewire").json();
    assert_eq!(pw["report"]["sinks"][0]["node_name"], "alsa_output.speaker");
}

#[test]
fn guided_setup_runs_the_pipewire_flow_to_the_review() {
    let t = T::new(new_session());
    let state = t.get("/api/guided/state").json();
    assert_eq!(state["active"], false);

    t.guided_to_discover();
    let step = t.answer("skip");
    assert_eq!(step["state"]["step"]["id"], "rate");
    let done = t.answer("176400");
    assert_eq!(done["next"], "review");
    assert_eq!(done["state"]["active"], false);
    let d = &done["draft"];
    assert_eq!(d["use_case"], "interactive-desktop");
    assert_eq!(d["values"]["output"]["backend"], "pipewire");
    assert_eq!(d["values"]["output"]["target_node"], "alsa_output.speaker");
    assert_eq!(d["values"]["target"]["pcm_rate_hz"], 176400);
    assert_eq!(d["guided_active"], false);
}

#[test]
fn guided_back_and_skip_leave_a_usable_draft() {
    let t = T::new(new_session());
    t.post("/api/guided/start", json!({}));
    t.answer("headless-appliance");
    let back = t.post("/api/guided/back", json!({}));
    assert_eq!(back.status, 200);
    assert_eq!(back.json()["state"]["step"]["id"], "use-case");
    assert_eq!(t.post("/api/guided/back", json!({})).status, 409);
    t.answer("headless-appliance");
    let skip = t.post("/api/guided/skip", json!({}));
    assert_eq!(skip.status, 200);
    assert_eq!(skip.json()["state"]["active"], false);
    assert_eq!(t.post("/api/guided/skip", json!({})).status, 409);
    let bad = t.post("/api/guided/answer", json!({ "key": "x" }));
    assert_eq!(bad.status, 409);
}

#[test]
fn guided_setup_is_refused_for_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let d = t.get("/api/draft").json();
    assert_eq!(d["is_new"], false);
    assert_eq!(d["values"]["target"]["pcm_rate_hz"], 88200);
    assert_eq!(t.post("/api/guided/start", json!({})).status, 409);
}

#[test]
fn offline_guided_setup_ends_in_the_form() {
    let t = T::new(new_session());
    t.post("/api/guided/start", json!({}));
    t.answer("offline-conversion");
    t.answer("dsd");
    let done = t.answer("dsd256");
    assert_eq!(done["next"], "form_offline");
}

#[test]
fn validating_streams_progress_and_marks_a_later_edit_stale() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let started = t.post("/api/benchmark", json!({}));
    assert_eq!(started.status, 200, "{}", started.body);
    let id = started.json()["id"].as_u64().unwrap();

    // While it runs, edits and a second run are refused.
    let busy = t.put_draft(json!({ "set": { "target.rate_family": "44100" } }));
    assert_eq!(busy.status, 409);
    assert_eq!(t.post("/api/benchmark", json!({})).status, 409);

    let stream = events(&t, id);
    assert!(stream.contains("event: progress"), "{stream}");
    assert_eq!(last_event(&stream), "finished", "{stream}");
    // The log is kept: reading again replays it.
    assert_eq!(events(&t, id), stream);
    let status = t.get(&format!("/api/benchmark/{id}")).json();
    assert_eq!(status["done"], true);

    let d = t.get("/api/draft").json();
    assert_eq!(d["validation"]["stale"], false);
    assert!(
        d["validation"]["result"]["worker_throughput_x"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    t.put_draft(json!({ "set": { "target.rate_family": "44100" } }));
    let d = t.get("/api/draft").json();
    assert_eq!(d["validation"]["stale"], true);
}

#[test]
fn validating_needs_a_valid_draft() {
    let t = T::new(new_session());
    assert_eq!(t.post("/api/benchmark", json!({})).status, 422);
}

#[test]
fn a_discovery_sweep_can_be_cancelled() {
    let t = T::new(new_session());
    t.guided_to_discover();
    let started = t.answer("run");
    assert_eq!(started["next"], "benchmark");
    let id = started["benchmark_id"].as_u64().unwrap();
    assert_eq!(
        t.call("DELETE", &format!("/api/benchmark/{id}"), "").status,
        202
    );
    let stream = events(&t, id);
    assert_eq!(last_event(&stream), "cancelled", "{stream}");
    let state = t.get("/api/guided/state").json();
    assert_eq!(state["discovery"]["active"], false);
    let tiers = state["discovery"]["tiers"].as_array().unwrap();
    assert!(tiers.iter().any(|x| x["state"] == "cancelled"), "{state}");
    // The flow is usable again after the sweep.
    assert_eq!(state["step"]["id"], "discover");
}

#[test]
fn saving_writes_the_file_reports_next_steps_and_stops_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("out.toml");
    let t = T::new(new_session());
    t.put_draft(json!({ "set": { "target.format": "pcm", "target.pcm_rate_hz": 88200 } }));
    let r = t.post(
        "/api/save",
        json!({ "path": path.display().to_string(), "comment_level": "none", "only_changed": false }),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let body = r.json();
    assert_eq!(body["created"], true);
    assert!(body["next"][1].as_str().unwrap().contains("audigma run -c"));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("pcm_rate_hz = 88200"), "{text}");
    assert!(!text.contains("##"));
    assert!(matches!(t.server.wait(), EndReason::Saved(_)));
}

#[test]
fn saving_is_refused_while_the_draft_has_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let t = T::new(new_session());
    let r = t.post("/api/save", json!({ "path": path.display().to_string() }));
    assert_eq!(r.status, 422);
    assert!(!path.exists());
    // The server is still up.
    assert_eq!(t.get("/api/draft").status, 200);
    t.post("/api/quit", json!({}));
    assert!(matches!(t.server.wait(), EndReason::Quit));
}

#[test]
fn an_idle_server_stops() {
    let t = T::with(
        new_session(),
        &ServeOptions {
            idle_timeout: Some(Duration::from_millis(300)),
            ..ServeOptions::default()
        },
    );
    assert!(matches!(t.server.wait(), EndReason::Idle));
}

#[test]
fn the_server_binds_loopback_by_default() {
    let t = T::new(new_session());
    assert!(!is_exposed(&t.addr));
    assert!(is_exposed(&SocketAddr::from(([0, 0, 0, 0], 1))));
    assert!(t.server.url().contains(&t.server.token));
    t.server.stop();
    assert!(matches!(t.server.wait(), EndReason::Quit));
}

fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn valid_draft(t: &T) {
    let r = t.put_draft(json!({ "set": { "target.format": "pcm", "target.pcm_rate_hz": 96000 } }));
    assert_eq!(r.status, 200, "{}", r.body);
}

#[test]
fn previewing_needs_the_token_and_the_origin_rule() {
    let t = T::new(new_session());
    let none = raw(t.addr, "POST", "/api/preview", &[], "{}");
    assert_eq!(none.status, 401);
    let cross = raw(
        t.addr,
        "POST",
        "/api/preview",
        &[
            ("Authorization", &t.auth),
            ("Origin", "https://evil.example"),
        ],
        "{}",
    );
    assert_eq!(cross.status, 403);
    assert_eq!(t.get("/api/preview").status, 405);
}

#[test]
fn previewing_is_refused_with_the_same_error_as_saving() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.toml");
    let t = T::new(new_session());
    let body = json!({ "path": path.display().to_string() });
    let p = t.post("/api/preview", body.clone());
    let s = t.post("/api/save", body);
    assert_eq!(p.status, 422);
    assert_eq!((p.status, p.json()), (s.status, s.json()));
    assert!(dir_names(dir.path()).is_empty());
}

#[test]
fn previewing_writes_nothing_and_keeps_the_server_running() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let before = dir_names(dir.path());
    let file = std::fs::read(dir.path().join("config.toml")).unwrap();
    t.put_draft(json!({ "set": { "target.pcm_rate_hz": 96000 } }));
    for _ in 0..2 {
        let r = t.post("/api/preview", json!({}));
        assert_eq!(r.status, 200, "{}", r.body);
    }
    // A new-file target in a directory that does not exist is not created either.
    let missing = dir.path().join("nope").join("x.toml");
    let r = t.post(
        "/api/preview",
        json!({ "path": missing.display().to_string() }),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(!dir.path().join("nope").exists());
    assert_eq!(dir_names(dir.path()), before);
    assert_eq!(std::fs::read(dir.path().join("config.toml")).unwrap(), file);
    // Still serving, not saved, still savable.
    assert_eq!(t.get("/api/draft").status, 200);
    t.post("/api/quit", json!({}));
    assert!(matches!(t.server.wait(), EndReason::Quit));
}

#[test]
fn a_preview_reports_existing_text_and_whether_it_changes_anything() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let path = dir.path().join("config.toml");
    let p = t.post("/api/preview", json!({})).json();
    assert_eq!(p["exists"], true);
    assert_eq!(p["existing"], PCM_FILE);
    assert_eq!(p["filename"], "config.toml");
    assert_eq!(p["path"], path.display().to_string());
    // The default level adds the generated comments, so the file would change.
    assert_eq!(p["unchanged"], false);
    let p = t
        .post("/api/preview", json!({ "comment_level": "none" }))
        .json();
    assert_eq!(p["unchanged"], true, "{p}");
    assert_eq!(p["text"], p["existing"]);

    t.put_draft(json!({ "set": { "target.pcm_rate_hz": 96000 } }));
    let p = t.post("/api/preview", json!({})).json();
    assert_eq!(p["unchanged"], false);
    assert_eq!(p["existing"], PCM_FILE);
    assert!(p["text"].as_str().unwrap().contains("96000"), "{p}");

    // A target that does not exist: null, and the default path is the session's.
    let new = T::new(new_session());
    valid_draft(&new);
    let p = new.post("/api/preview", json!({})).json();
    assert_eq!(p["exists"], false);
    assert_eq!(p["existing"], Value::Null);
    assert_eq!(p["path"], NEW_PATH);
    assert_eq!(p["filename"], "config.toml");
    assert_eq!(p["unchanged"], false);
}

#[test]
fn an_unreadable_target_is_an_error_not_a_null() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(new_session());
    valid_draft(&t);
    // A directory cannot be read as text.
    let r = t.post(
        "/api/preview",
        json!({ "path": dir.path().display().to_string() }),
    );
    assert_eq!(r.status, 422, "{}", r.body);
    assert!(r.json()["error"].is_string());
    // So is a file that is not UTF-8.
    let bin = dir.path().join("bin.toml");
    std::fs::write(&bin, [0xff, 0xfe, 0x00]).unwrap();
    let r = t.post("/api/preview", json!({ "path": bin.display().to_string() }));
    assert_eq!(r.status, 422, "{}", r.body);
}

#[test]
fn a_preview_is_allowed_while_a_benchmark_runs() {
    let dir = tempfile::tempdir().unwrap();
    let t = T::new(existing_session(dir.path()));
    let id = t.post("/api/benchmark", json!({})).json()["id"]
        .as_u64()
        .unwrap();
    assert_eq!(t.post("/api/preview", json!({})).status, 200);
    // Saving would end the server mid-run, so it stays refused.
    assert_eq!(t.post("/api/save", json!({})).status, 409);
    events(&t, id);
}

#[test]
fn saving_writes_exactly_the_previewed_text() {
    let dir = tempfile::tempdir().unwrap();
    for (level, only_changed) in [("none", true), ("brief", true), ("full", false)] {
        // Update in place, with user comments the update must keep.
        let user = "# my notes\nschema_version = 1\n\n# keep this\n[target]\nformat = \"pcm\"\n\
                    pcm_rate_hz = 88200 # inline\n";
        let existing = dir.path().join(format!("in-place-{level}.toml"));
        std::fs::write(&existing, user).unwrap();
        let new_file = dir.path().join(format!("new-{level}.toml"));
        for (path, edit) in [
            (&existing, json!({ "set": { "target.pcm_rate_hz": 96000 } })),
            (
                &new_file,
                json!({ "set": { "target.format": "pcm", "target.pcm_rate_hz": 96000 } }),
            ),
        ] {
            let t = T::new(Session::open(&existing).unwrap());
            assert_eq!(t.put_draft(edit).status, 200);
            let opts = json!({
                "path": path.display().to_string(),
                "comment_level": level,
                "only_changed": only_changed,
            });
            let before = std::fs::read(path).ok();
            let p = t.post("/api/preview", opts.clone());
            assert_eq!(p.status, 200, "{}", p.body);
            let p = p.json();
            assert_eq!(std::fs::read(path).ok(), before, "preview wrote");
            let s = t.post("/api/save", opts);
            assert_eq!(s.status, 200, "{}", s.body);
            assert!(matches!(t.server.wait(), EndReason::Saved(_)));
            let saved = std::fs::read_to_string(path).unwrap();
            assert_eq!(saved, p["text"].as_str().unwrap(), "{level}");
            if path == &existing {
                assert_eq!(p["existing"], user);
                assert!(
                    saved.contains("# my notes") && saved.contains("# keep this"),
                    "{saved}"
                );
            } else {
                assert_eq!(p["exists"], false);
            }
        }
    }
}
