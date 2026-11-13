//! TUI tests: the state machine through key events, and golden snapshots of the rendered
//! screens on a `TestBackend`. Regenerate goldens with `UPDATE_GOLDEN=1 cargo test`.
#![cfg(feature = "tui")]

use std::path::Path;

use audigma_configurator::assist::benchmark::BenchmarkEvent;
use audigma_configurator::assist::probe::ProbeReport;
use audigma_configurator::tui::{App, Effect, ListItem, Mode, Popup, Screen, draw};
use audigma_settings::meta::UseCase;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const PATH: &str = "/nonexistent/audigma/config.toml";

fn new_app() -> App {
    App::new(Path::new(PATH), None).unwrap()
}

/// A new file, already in guided setup.
fn new_guided() -> App {
    let mut app = new_app();
    assert_eq!(app.screen, Screen::Start);
    enter(&mut app);
    assert_eq!(app.screen, Screen::Guided);
    app
}

fn existing(text: &str) -> App {
    App::new(Path::new(PATH), Some(text)).unwrap()
}

fn key(app: &mut App, code: KeyCode) -> Vec<Effect> {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn press(app: &mut App, keys: &str) {
    for c in keys.chars() {
        key(app, KeyCode::Char(c));
    }
}

fn enter(app: &mut App) {
    key(app, KeyCode::Enter);
}

fn down(app: &mut App, n: usize) {
    for _ in 0..n {
        key(app, KeyCode::Down);
    }
}

/// Moves the cursor onto the key `path`.
fn focus(app: &mut App, path: &str) {
    app.goto_field(path);
    assert_eq!(
        app.focused_field().map(|f| f.path.as_str()),
        Some(path),
        "{path} is not focusable"
    );
}

fn screen(app: &App, w: u16, h: u16) -> String {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, app)).unwrap();
    let buf = t.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("tui_{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; run with UPDATE_GOLDEN=1",
            path.display()
        )
    });
    assert_eq!(
        actual, want,
        "golden {name} differs (UPDATE_GOLDEN=1 to accept)"
    );
}

/// Leaves guided setup (a new file starts in it) for the form; an existing file is there already.
fn into_form(app: &mut App) {
    match app.screen {
        Screen::Start => {
            key(app, KeyCode::Down); // "Edit configuration"
            enter(app);
            key(app, KeyCode::Esc); // skip the use-case question
        }
        Screen::Guided => {
            key(app, KeyCode::Esc);
            press(app, "y");
        }
        _ => {}
    }
    assert_eq!(app.screen, Screen::Form);
}

// ---- state machine ---------------------------------------------------------------------

#[test]
fn a_new_file_offers_the_choice_and_an_existing_one_opens_the_form() {
    let mut app = new_app();
    assert_eq!(app.screen, Screen::Start);
    assert!(app.guided.is_none());
    enter(&mut app); // the first item is guided setup
    assert_eq!(app.screen, Screen::Guided);
    assert!(app.guided.is_some());
    let mut app = new_app();
    key(&mut app, KeyCode::Down);
    enter(&mut app);
    assert_eq!(app.screen, Screen::Form, "direct editing");
    assert!(
        matches!(app.popup, Some(Popup::UseCase { .. })),
        "asks the use case"
    );
    assert!(app.guided.is_none());
    let app = existing("schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n");
    assert_eq!(app.screen, Screen::Form);
    assert!(app.guided.is_none());
}

#[test]
fn skipping_guided_setup_keeps_the_answers_and_applies_no_recommendations() {
    let mut app = new_guided();
    enter(&mut app); // interactive desktop (first choice)
    assert_eq!(app.use_case, Some(UseCase::InteractiveDesktop));
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.popup, Some(Popup::ConfirmSkip));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.screen, Screen::Guided, "any other key stays");
    key(&mut app, KeyCode::Esc);
    press(&mut app, "y");
    assert_eq!(app.screen, Screen::Form);
    assert!(app.guided.is_none());
    assert_eq!(app.mode, Mode::Live);
    assert!(!app.draft.is_touched("output.keep_audio_sink_ms"));
}

#[test]
fn skipping_at_the_first_step_leaves_the_use_case_unset() {
    let mut app = new_app();
    into_form(&mut app);
    assert_eq!(app.use_case, None);
    assert_eq!(app.mode, Mode::All);
}

#[test]
fn back_returns_to_the_previous_step() {
    let mut app = new_guided();
    key(&mut app, KeyCode::Backspace);
    assert_eq!(
        app.guided_view().unwrap().id,
        "use-case",
        "no step before the first"
    );
    enter(&mut app);
    assert_eq!(app.guided_view().unwrap().id, "output-path");
    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.guided_view().unwrap().id, "use-case");
    assert_eq!(app.use_case, None);
}

#[test]
fn guided_offline_ends_in_the_offline_section_of_the_form() {
    let mut app = new_guided();
    down(&mut app, 2);
    enter(&mut app); // offline conversion
    assert_eq!(app.guided_view().unwrap().id, "offline-format");
    down(&mut app, 1);
    enter(&mut app); // dsd
    assert_eq!(app.guided_view().unwrap().id, "rate");
    down(&mut app, 2);
    enter(&mut app); // dsd256
    assert!(app.guided.is_none());
    assert_eq!(app.screen, Screen::Form);
    assert_eq!(app.mode, Mode::Offline);
    assert_eq!(
        app.focused(),
        Some(audigma_configurator::tui::app::Row::Section(
            "offline".into()
        ))
    );
    assert_eq!(
        app.set_value("target.dsd_rate"),
        Some(&serde_json::json!("dsd256"))
    );
}

#[test]
fn guided_live_flow_probes_at_the_device_step_and_ends_on_review() {
    let mut app = new_guided();
    enter(&mut app); // interactive desktop
    key(&mut app, KeyCode::Down); // PipeWire sink
    let fx = key(&mut app, KeyCode::Enter);
    assert_eq!(
        fx,
        vec![Effect::Probe],
        "the device step asks for a fresh probe"
    );
    assert!(app.guided_view().unwrap().waiting_for_probe);
    app.on_probe(ProbeReport {
        alsa: Err("none".into()),
        pipewire: Ok(audigma_io::PipeWireProbeReport { sinks: vec![] }),
    });
    let v = app.guided_view().unwrap();
    assert_eq!(v.id, "pipewire-sink");
    assert_eq!(v.choices.len(), 1, "only 'the default sink'");
    assert!(v.problem.is_some(), "the empty probe is reported");
    enter(&mut app); // default sink; the format answers itself (PCM)
    assert_eq!(app.guided_view().unwrap().id, "quality");
    enter(&mut app); // neutral
    assert_eq!(app.guided_view().unwrap().id, "rate-policy");
    enter(&mut app); // follow the source
    assert_eq!(app.guided_view().unwrap().id, "discover");
    let fx = key(&mut app, KeyCode::Enter); // run the benchmark
    assert!(
        matches!(fx.as_slice(), [Effect::StartBenchmark(_)]),
        "{fx:?}"
    );
    assert!(app.discovering());
    // Esc while measuring cancels the benchmark instead of asking to skip.
    assert_eq!(key(&mut app, KeyCode::Esc), vec![Effect::CancelBenchmark]);
    assert!(app.popup.is_none());
    app.on_bench(BenchmarkEvent::Cancelled);
    assert!(!app.discovering());
    enter(&mut app); // continue
    assert_eq!(app.guided_view().unwrap().id, "rate");
    enter(&mut app); // first tier; volume is skipped on PipeWire
    assert!(app.guided.is_none());
    assert_eq!(app.screen, Screen::Review);
    assert!(
        app.review_banner,
        "the review page says where the values came from"
    );
    golden("guided_review", &screen(&app, 80, 24));
    press(&mut app, "f");
    assert_eq!(app.screen, Screen::Form);
    assert!(!app.review_banner);
    assert!(!app.draft.has_errors(), "{:?}", app.issues());
}

#[test]
fn existing_file_does_not_ask() {
    let mut app =
        existing("schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n");
    into_form(&mut app);
    assert!(app.popup.is_none());
    assert_eq!(app.use_case, None);
}

#[test]
fn use_case_sets_the_mode() {
    let mut app = new_app();
    into_form(&mut app);
    press(&mut app, "u");
    down(&mut app, 2);
    enter(&mut app); // offline conversion
    assert_eq!(app.use_case, Some(UseCase::OfflineConversion));
    assert_eq!(app.mode, Mode::Offline);
    press(&mut app, "u");
    key(&mut app, KeyCode::Up);
    enter(&mut app); // headless appliance
    assert_eq!(app.mode, Mode::Live);
    press(&mut app, "m");
    assert_eq!(app.mode, Mode::Offline);
}

#[test]
fn mode_filters_by_applies_to() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    let rows = |a: &App| a.rows().len();
    app.folded.clear();
    let all = rows(&app);
    press(&mut app, "m"); // live
    let live = rows(&app);
    press(&mut app, "m"); // offline
    let offline = rows(&app);
    assert!(live < all && offline < all);
    // `output.backend` is live-only; `offline.*` is convert-only.
    let has = |a: &App, p: &str| {
        a.rows()
            .contains(&audigma_configurator::tui::app::Row::Field(p.into()))
    };
    assert!(!has(&app, "output.backend"));
    press(&mut app, "m"); // all
    press(&mut app, "m"); // live
    assert!(has(&app, "output.backend"));
    assert!(!has(&app, "offline.tail_policy"));
    // Show all brings hidden keys back.
    press(&mut app, "*");
    assert!(has(&app, "offline.tail_policy"));
}

#[test]
fn backend_before_device_passes_through_an_invalid_state() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    app.draft
        .set("target.format", &serde_json::json!("pcm"))
        .unwrap();
    app.draft
        .set("target.pcm_rate_hz", &serde_json::json!(192000))
        .unwrap();
    focus(&mut app, "output.backend");
    enter(&mut app);
    // alsa, pipewire: the cursor starts on alsa
    enter(&mut app);
    assert_eq!(
        app.set_value("output.backend").and_then(|v| v.as_str()),
        Some("alsa")
    );
    let issues = app.issues();
    assert!(
        issues
            .iter()
            .any(|i| i.error && i.path.as_deref() == Some("output.device"))
    );
    focus(&mut app, "output.device");
    enter(&mut app);
    press(&mut app, "hw:CARD=K17,DEV=0");
    enter(&mut app);
    assert_eq!(
        app.set_value("output.device").and_then(|v| v.as_str()),
        Some("hw:CARD=K17,DEV=0")
    );
    assert!(
        !app.issues()
            .iter()
            .any(|i| i.path.as_deref() == Some("output.device"))
    );
}

#[test]
fn input_errors_stay_in_the_popup_and_leave_the_draft_alone() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.buffer_ms");
    enter(&mut app);
    press(&mut app, "nonsense");
    enter(&mut app);
    assert!(app.popup.is_some());
    assert!(app.set_value("output.buffer_ms").is_none());
    for _ in 0..8 {
        key(&mut app, KeyCode::Backspace);
    }
    press(&mut app, "60");
    enter(&mut app);
    assert!(app.popup.is_none());
    assert_eq!(
        app.set_value("output.buffer_ms").and_then(|v| v.as_i64()),
        Some(60)
    );
    // Backspace on the row resets it.
    key(&mut app, KeyCode::Backspace);
    assert!(app.set_value("output.buffer_ms").is_none());
}

#[test]
fn dsp_threads_takes_a_number_as_the_extra_choice() {
    let mut app =
        existing("schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n");
    into_form(&mut app);
    app.folded.clear();
    focus(&mut app, "runtime.dsp_threads");
    enter(&mut app);
    down(&mut app, 2); // auto, inline, "a number"
    enter(&mut app);
    press(&mut app, "4");
    enter(&mut app);
    assert_eq!(
        app.set_value("runtime.dsp_threads")
            .and_then(|v| v.as_i64()),
        Some(4)
    );
}

#[test]
fn preset_assist_fills_its_keys_and_reports_the_match() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "resampler.phase");
    assert_eq!(
        app.matching_preset("quality-starting-point").as_deref(),
        Some("Neutral")
    );
    press(&mut app, "a");
    down(&mut app, 1); // Apodizing
    enter(&mut app);
    assert_eq!(
        app.matching_preset("quality-starting-point").as_deref(),
        Some("Apodizing")
    );
    // The settings keep `gentle` as its 19 kHz edge.
    assert_eq!(
        app.set_value("resampler.apodization")
            .and_then(|v| v.as_i64()),
        Some(19000)
    );
    // A manual change makes it Custom.
    focus(&mut app, "resampler.phase");
    enter(&mut app);
    key(&mut app, KeyCode::Up);
    enter(&mut app);
    assert_eq!(app.matching_preset("quality-starting-point"), None);
}

#[test]
fn probe_assist_requests_a_probe_and_offers_the_devices() {
    use audigma_configurator::assist::probe::ProbeReport;
    use audigma_io::{AlsaCardReport, AlsaDeviceReport, AlsaProbeReport, PipeWireProbeReport};
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.device");
    assert_eq!(press_effects(&mut app, 'a'), vec![Effect::Probe]);
    app.on_probe(ProbeReport {
        alsa: Ok(AlsaProbeReport {
            cards: vec![AlsaCardReport {
                index: 2,
                id: "K17".into(),
                name: "K17".into(),
                longname: "FiiO K17".into(),
                usb_id: None,
                mixer_controls: vec![],
                devices: vec![AlsaDeviceReport {
                    card_index: 2,
                    device_index: 0,
                    id: "USB Audio".into(),
                    name: "USB Audio".into(),
                    device_path: "hw:CARD=K17,DEV=0".into(),
                    is_busy: false,
                    active_status: None,
                    capabilities: None,
                    error: None,
                }],
            }],
        }),
        pipewire: Ok(PipeWireProbeReport { sinks: vec![] }),
    });
    // The modal opened by itself when the probe finished.
    assert!(matches!(app.popup, Some(Popup::List { .. })));
    enter(&mut app);
    assert_eq!(
        app.set_value("output.device").and_then(|v| v.as_str()),
        Some("hw:CARD=K17,DEV=0")
    );
    // Pressing a again probes again instead of reusing the earlier report.
    focus(&mut app, "output.device");
    assert_eq!(press_effects(&mut app, 'a'), vec![Effect::Probe]);
}

#[test]
fn assist_does_not_open_if_the_cursor_moved_during_the_probe() {
    use audigma_configurator::assist::probe::ProbeReport;
    use audigma_io::{AlsaProbeReport, PipeWireProbeReport};
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.device");
    assert_eq!(press_effects(&mut app, 'a'), vec![Effect::Probe]);
    focus(&mut app, "output.backend");
    app.on_probe(ProbeReport {
        alsa: Ok(AlsaProbeReport { cards: vec![] }),
        pipewire: Ok(PipeWireProbeReport { sinks: vec![] }),
    });
    assert!(app.popup.is_none());
}

fn press_effects(app: &mut App, c: char) -> Vec<Effect> {
    key(app, KeyCode::Char(c))
}

#[test]
fn review_blocks_save_while_there_are_errors_and_jumps_to_them() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    press(&mut app, "r");
    assert_eq!(app.screen, Screen::Review);
    press(&mut app, "s");
    assert_eq!(app.screen, Screen::Review);
    assert!(app.notice.is_some());
    press(&mut app, "g");
    assert_eq!(app.screen, Screen::Form);
    assert_eq!(
        app.focused(),
        Some(audigma_configurator::tui::app::Row::Section(
            "target".into()
        ))
    );
}

#[test]
fn review_scrolls_with_arrows_and_clamps_at_both_ends() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    let keys = [
        ("target.format", serde_json::json!("pcm")),
        ("target.pcm_rate_hz", serde_json::json!(192000)),
        ("output.backend", serde_json::json!("alsa")),
        ("output.device", serde_json::json!("hw:CARD=K17,DEV=0")),
        ("input.channels", serde_json::json!(2)),
        ("input.rate_mode", serde_json::json!("dynamic")),
        ("input.allow_dsd", serde_json::json!(true)),
        ("output.period_ms", serde_json::json!(5)),
        ("output.device_wait_ms", serde_json::json!(-1)),
        ("output.keep_audio_sink_ms", serde_json::json!(5000)),
        ("output.device_wait_expiry", serde_json::json!("exit")),
        ("output.volume_control", serde_json::json!("software")),
        ("logging.level", serde_json::json!("info")),
        ("resampler.phase", serde_json::json!("linear")),
        ("output.buffer_ms", serde_json::json!(40)),
    ];
    for (k, v) in &keys {
        app.draft.set(k, v).unwrap();
    }
    press(&mut app, "r");
    let short = screen(&app, 80, 24);
    assert!(!short.contains("... and"), "no truncation line: {short}");
    let top = short.clone();
    key(&mut app, KeyCode::Up);
    assert_eq!(screen(&app, 80, 24), top, "cannot scroll above the top");
    down(&mut app, 100);
    let bottom = screen(&app, 80, 24);
    assert_ne!(bottom, top, "scrolled");
    assert!(bottom.contains("output.buffer_ms"), "last key is reachable");
    key(&mut app, KeyCode::Up);
    assert_ne!(
        screen(&app, 80, 24),
        bottom,
        "one Up undoes overshoot immediately"
    );
    key(&mut app, KeyCode::Home);
    assert_eq!(screen(&app, 80, 24), top);
}

#[test]
fn benchmark_is_requested_for_the_exact_settings_and_goes_stale() {
    use audigma_configurator::assist::benchmark::{BenchmarkEvent, Stage};
    let mut app =
        existing("schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n");
    into_form(&mut app);
    press(&mut app, "r");
    let effects = press_effects(&mut app, 'b');
    assert!(matches!(effects.as_slice(), [Effect::StartBenchmark(_)]));
    app.on_bench(BenchmarkEvent::Progress {
        stage: Stage::Workers,
        fraction: 0.5,
    });
    assert_eq!(press_effects(&mut app, 'c'), vec![Effect::CancelBenchmark]);
    app.on_bench(BenchmarkEvent::Cancelled);
    assert!(!app.bench_is_stale());
    app.draft
        .set("gain.preamp_db", &serde_json::json!(-1.0))
        .unwrap();
    assert!(app.bench_is_stale());
}

#[test]
fn save_writes_the_file_and_reports_the_commands() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = App::new(&path, None).unwrap();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    app.draft
        .set("target.format", &serde_json::json!("pcm"))
        .unwrap();
    app.draft
        .set("target.pcm_rate_hz", &serde_json::json!(192000))
        .unwrap();
    press(&mut app, "r");
    press(&mut app, "s");
    assert_eq!(app.screen, Screen::Save);
    enter(&mut app); // focus starts on Save
    assert!(path.exists());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("pcm_rate_hz = 192000"));
    audigma_settings::Settings::from_toml_str(&text).unwrap();
    // Leaving after a save does not ask to discard.
    press(&mut app, "q");
    assert!(app.quit);
}

#[test]
fn quitting_a_dirty_draft_asks_first() {
    let mut app = new_app();
    into_form(&mut app);
    app.draft
        .set("target.format", &serde_json::json!("pcm"))
        .unwrap();
    press(&mut app, "q");
    assert!(!app.quit);
    key(&mut app, KeyCode::Char('n'));
    assert!(!app.quit);
    press(&mut app, "q");
    press(&mut app, "y");
    assert!(app.quit);
}

#[test]
fn an_unloadable_file_is_refused_with_the_field_error() {
    let err = App::new(
        Path::new(PATH),
        Some("schema_version = 1\n[target]\nformat = \"pcm\"\n[output]\nbackend = \"alsa\"\n"),
    )
    .err()
    .unwrap();
    let msg = err.to_string();
    assert!(msg.contains(PATH), "{msg}");
    assert!(msg.contains("output.device"), "{msg}");
    let err = App::new(Path::new(PATH), Some("[target\nformat = "))
        .err()
        .unwrap();
    assert!(err.to_string().contains("line 1"), "{err}");
}

// ---- snapshots -------------------------------------------------------------------------

#[test]
fn golden_start_new_file() {
    golden("start_new", &screen(&new_app(), 80, 24));
}

#[test]
fn golden_guided_first_step() {
    golden("guided_use_case", &screen(&new_guided(), 80, 24));
}

#[test]
fn golden_use_case_modal() {
    let mut app = new_app();
    into_form(&mut app);
    press(&mut app, "u");
    golden("use_case", &screen(&app, 80, 24));
}

#[test]
fn golden_form_output_section() {
    let mut app = new_app();
    into_form(&mut app);
    press(&mut app, "u");
    down(&mut app, 1);
    enter(&mut app); // headless appliance
    app.draft
        .set("output.backend", &serde_json::json!("alsa"))
        .unwrap();
    app.draft
        .set("output.device", &serde_json::json!("hw:CARD=K17,DEV=0"))
        .unwrap();
    app.draft
        .set("target.format", &serde_json::json!("dsd"))
        .unwrap();
    focus(&mut app, "output.backend");
    golden("form_output", &screen(&app, 80, 24));
}

#[test]
fn golden_form_with_picker_open() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "resampler.phase");
    enter(&mut app);
    key(&mut app, KeyCode::Down);
    golden("picker", &screen(&app, 80, 24));
}

#[test]
fn golden_input_popup_with_error() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.buffer_ms");
    enter(&mut app);
    press(&mut app, "999999");
    enter(&mut app);
    golden("input_error", &screen(&app, 80, 24));
}

#[test]
fn golden_review_with_errors() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    app.draft
        .set("output.backend", &serde_json::json!("alsa"))
        .unwrap();
    press(&mut app, "r");
    golden("review_errors", &screen(&app, 80, 24));
}

#[test]
fn golden_save_page() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    app.draft
        .set("target.format", &serde_json::json!("pcm"))
        .unwrap();
    app.draft
        .set("target.pcm_rate_hz", &serde_json::json!(192000))
        .unwrap();
    press(&mut app, "r");
    press(&mut app, "s");
    golden("save", &screen(&app, 80, 24));
}

#[test]
fn golden_too_small() {
    golden("too_small", &screen(&new_app(), 60, 12));
}

#[test]
fn golden_picker_wraps_long_option_summaries() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.volume_control");
    enter(&mut app);
    golden("picker_long", &screen(&app, 80, 24));
}

fn popup_list(app: &App) -> (&[ListItem], usize) {
    match &app.popup {
        Some(Popup::List { items, sel, .. }) => (items, *sel),
        other => panic!("expected a list popup, got {other:?}"),
    }
}

#[test]
fn picker_starts_on_the_first_option_without_a_use_case_or_default() {
    let mut app = new_app();
    into_form(&mut app);
    key(&mut app, KeyCode::Esc);
    focus(&mut app, "output.backend");
    enter(&mut app);
    let (items, sel) = popup_list(&app);
    assert_eq!(sel, 0);
    assert!(items[0].label.starts_with("alsa"));
    assert!(!items.iter().any(|i| i.label.contains("Recommended")));
}

#[test]
fn picker_preselects_and_tags_the_use_case_recommendation() {
    let mut app = new_app();
    app.use_case = Some(UseCase::InteractiveDesktop);
    focus(&mut app, "output.device_wait_expiry");
    enter(&mut app);
    let (items, sel) = popup_list(&app);
    // default is `exit`, listed first; the desktop recommendation is `pause`.
    assert_eq!(sel, 1);
    assert!(items[0].label.starts_with("Exit"));
    assert!(
        items[1]
            .label
            .ends_with("(Recommended for interactive desktop)")
    );
}

#[test]
fn picker_prefers_the_value_already_set() {
    let mut app = new_app();
    app.use_case = Some(UseCase::InteractiveDesktop);
    focus(&mut app, "output.backend");
    enter(&mut app);
    key(&mut app, KeyCode::Down);
    enter(&mut app); // pipewire
    focus(&mut app, "output.backend");
    enter(&mut app);
    // alsa (recommended) is first; the cursor stays on the value the draft already sets.
    assert_eq!(popup_list(&app).1, 1);
}

// ---- guided screens --------------------------------------------------------------------

fn k17_probe() -> ProbeReport {
    use audigma_io::alsa::probe::{DeviceCapabilities, DopSupport, NativeDsdSupport};
    use audigma_io::{AlsaCardReport, AlsaDeviceReport, AlsaProbeReport};
    ProbeReport {
        alsa: Ok(AlsaProbeReport {
            cards: vec![AlsaCardReport {
                index: 3,
                id: "K17".into(),
                name: "USB-Audio".into(),
                longname: "FiiO K17".into(),
                usb_id: None,
                mixer_controls: vec![],
                devices: vec![AlsaDeviceReport {
                    card_index: 3,
                    device_index: 0,
                    id: "USB Audio".into(),
                    name: "USB Audio".into(),
                    device_path: "hw:CARD=K17,DEV=0".into(),
                    is_busy: false,
                    active_status: None,
                    capabilities: Some(DeviceCapabilities {
                        formats: vec!["S32_LE".into()],
                        rates_hz: vec![44100, 48000, 88200, 96000, 176400, 192000],
                        min_channels: 2,
                        max_channels: 2,
                        native_dsd: NativeDsdSupport {
                            supported: true,
                            formats: vec![],
                            max_frame_rate_hz: None,
                            max_dsd_tier: Some("DSD512".into()),
                            dsd_raw: None,
                        },
                        dop: DopSupport {
                            supported: false,
                            max_dop_tier: None,
                            supported_containers: vec![],
                        },
                        usb_altsets: vec![],
                    }),
                    error: None,
                }],
            }],
        }),
        pipewire: Err("none".into()),
    }
}

#[test]
fn golden_guided_device_step() {
    let mut app = new_guided();
    enter(&mut app); // interactive desktop
    enter(&mut app); // ALSA
    golden("guided_probing", &screen(&app, 80, 24));
    app.on_probe(k17_probe());
    golden("guided_device", &screen(&app, 80, 24));
}

#[test]
fn golden_guided_rates_with_results_and_unsupported_tiers() {
    let mut app = new_guided();
    enter(&mut app); // interactive desktop
    enter(&mut app); // ALSA
    app.on_probe(k17_probe());
    enter(&mut app); // the K17
    golden("guided_format", &screen(&app, 80, 24)); // no DoP offered
    enter(&mut app); // DSD, native (the recommended one)
    enter(&mut app); // neutral
    enter(&mut app); // follow the source
    let fx = key(&mut app, KeyCode::Enter); // run the benchmark
    assert!(matches!(fx.as_slice(), [Effect::StartBenchmark(_)]));
    app.on_bench(BenchmarkEvent::Failed("no benchmark in tests".into()));
    golden("guided_discovery", &screen(&app, 80, 24));
    enter(&mut app); // continue
    golden("guided_rate", &screen(&app, 80, 24));
}
