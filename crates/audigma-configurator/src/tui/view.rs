//! Drawing. Pure functions of [`App`]; no state of their own.

use audigma_settings::meta::{AssistKind, Field, FieldKind};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::app::{App, Bench, ListItem, Popup, Row, Screen, USE_CASES, use_case_label};
use super::markdown::{inline_spans, render};
use crate::assist::benchmark::Stage;
use crate::assist::capability::Severity;
use crate::writer::CommentLevel;
use crate::writer::text::{option_text, show_value};

pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

const DIM: Style = Style::new().fg(Color::DarkGray);
const KEY: Style = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ERR: Style = Style::new().fg(Color::Red);
const WARN: Style = Style::new().fg(Color::Yellow);
const OK: Style = Style::new().fg(Color::Green);

fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!(
            "Terminal too small: {}x{} (need at least {MIN_WIDTH}x{MIN_HEIGHT}).",
            area.width, area.height
        );
        f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), area);
        return;
    }
    match app.screen {
        Screen::Start => draw_start(f, app, area),
        Screen::Guided => draw_guided(f, app, area),
        Screen::Form => draw_form(f, app, area),
        Screen::Review => draw_review(f, app, area),
        Screen::Save => draw_save(f, app, area),
    }
    if let Some(p) = &app.popup {
        draw_popup(f, app, p, area);
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(width.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn pad(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let n = t.chars().count();
    format!("{t}{}", " ".repeat(width.saturating_sub(n)))
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn header(app: &App) -> Line<'static> {
    let file = if app.is_new {
        "new file"
    } else {
        "existing file"
    };
    let uc = app.use_case.map_or("not set", use_case_label);
    Line::from(vec![
        Span::styled(" audigma configure ", BOLD.add_modifier(Modifier::REVERSED)),
        Span::raw(format!(
            " {file} · use case: {uc} · mode: {}{}",
            app.mode.label(),
            if app.show_all {
                " · hidden keys shown"
            } else {
                ""
            }
        )),
    ])
}

/// The key bar: each entry is `key description`, entries are separated by two spaces or ` · `.
/// The key is drawn bright so it stands out from its dim description.
fn hints(text: &str) -> Paragraph<'static> {
    let sep = if text.contains(" · ") { " · " } else { "  " };
    let mut spans = vec![Span::styled(" ", DIM)];
    for (i, entry) in text.split(sep).enumerate() {
        if i > 0 {
            spans.push(Span::styled(sep, DIM));
        }
        match entry.split_once(' ') {
            Some((key, rest)) => {
                spans.push(Span::styled(key.to_string(), KEY));
                spans.push(Span::styled(format!(" {rest}"), DIM));
            }
            None => spans.push(Span::styled(entry.to_string(), KEY)),
        }
    }
    Paragraph::new(Line::from(spans))
}

// ---- start ---------------------------------------------------------------------------------

fn draw_start(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(Paragraph::new(header(app)), rows[0]);
    let boxed = centered(rows[1], 64, 12);
    let block = Block::default().borders(Borders::ALL).title(" New file ");
    let inner = block.inner(boxed);
    f.render_widget(block, boxed);
    let what = format!(
        "{} does not exist yet; a new file will be created.",
        app.path.display()
    );
    let item = |i: usize, label: &str, sub: &str| {
        let st = if i == app.start_sel {
            selected()
        } else {
            Style::default()
        };
        vec![
            Line::from(vec![
                Span::raw(if i == app.start_sel { "> " } else { "  " }),
                Span::styled(label.to_string(), st),
            ]),
            Line::styled(format!("    {sub}"), DIM),
        ]
    };
    let mut lines = render(&what, inner.width, Style::default());
    lines.push(Line::default());
    lines.extend(item(
        0,
        "Guided setup",
        "A few questions; the rest takes recommended values.",
    ));
    lines.push(Line::default());
    lines.extend(item(
        1,
        "Edit configuration",
        "Every key, with its documentation.",
    ));
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(n) = &app.notice {
        f.render_widget(Paragraph::new(Line::styled(format!(" {n}"), WARN)), rows[2]);
    }
    f.render_widget(hints("↑↓ choose · Enter select · q quit"), rows[3]);
}

// ---- guided --------------------------------------------------------------------------------

fn progress_bar(fraction: f64, width: usize) -> String {
    let done = ((fraction.clamp(0.0, 1.0)) * width as f64).round() as usize;
    format!(
        "[{}{}] {:>3.0}%",
        "#".repeat(done),
        "-".repeat(width - done),
        fraction * 100.0
    )
}

fn indent(lines: Vec<Line<'static>>, by: &str) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|l| {
            let mut spans = vec![Span::raw(by.to_string())];
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect()
}

fn draw_guided(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(Paragraph::new(header(app)), rows[0]);
    let Some(view) = app.guided_view() else {
        return;
    };
    let block = Block::default().borders(Borders::ALL).title(format!(
        " Guided setup · step {} of {} · {} ",
        view.number, view.total, view.title
    ));
    let inner = block.inner(rows[1]);
    f.render_widget(block, rows[1]);
    let w = inner.width.saturating_sub(1);
    let discovering = app.discovering();

    let mut lines = render(&view.prompt, w, Style::default());
    lines.push(Line::default());
    if view.waiting_for_probe {
        lines.push(Line::styled("Probing the machine...", WARN));
    }
    if let Some(p) = &view.problem {
        lines.push(Line::styled(p.clone(), WARN));
        lines.push(Line::default());
    }
    // The line range of the selected choice, to keep it on screen.
    let mut sel_range = (0, 0);
    // Once the sweep has run, its results replace the run/skip choices.
    let hide_choices = view.kind == crate::guided::StepKind::Discover
        && app.guided.as_ref().is_some_and(|g| !g.discovery.is_empty());
    for (i, c) in view.choices.iter().enumerate().filter(|_| !hide_choices) {
        let on = i == app.guided_sel && !discovering;
        let start = lines.len();
        let style = if c.disabled.is_some() {
            DIM
        } else if on {
            selected()
        } else {
            Style::default()
        };
        let mut head = vec![
            Span::raw(if on { "> " } else { "  " }),
            Span::styled(c.label.clone(), style),
        ];
        if let (Some(_), Some(u)) = (&c.recommendation, app.use_case) {
            head.push(Span::styled(
                format!(" (recommended for {})", super::app::use_case_label(u)),
                OK,
            ));
        }
        lines.push(Line::from(head));
        lines.extend(indent(render(&c.summary, w.saturating_sub(4), DIM), "    "));
        if let Some(n) = &c.note {
            lines.extend(indent(render(n, w.saturating_sub(4), WARN), "    "));
        }
        if let Some(d) = &c.disabled {
            lines.extend(indent(render(d, w.saturating_sub(4), ERR), "    "));
        }
        if on {
            if let Some(why) = &c.recommendation {
                lines.extend(indent(
                    render(&format!("Recommended: {why}"), w.saturating_sub(4), OK),
                    "    ",
                ));
            }
            if let Some(d) = &c.description {
                lines.extend(indent(
                    render(d, w.saturating_sub(4), Style::default()),
                    "    ",
                ));
            }
            if let Some(ca) = &c.caution {
                lines.extend(indent(
                    render(&format!("Caution: {ca}"), w.saturating_sub(4), WARN),
                    "    ",
                ));
            }
        }
        sel_range = if on { (start, lines.len()) } else { sel_range };
    }
    if let Some(g) = app
        .guided
        .as_ref()
        .filter(|_| matches!(view.id.as_str(), "discover" | "rate"))
    {
        if !g.discovery.is_empty() {
            lines.push(Line::default());
            lines.push(Line::styled("Results (estimates)", BOLD));
            for (key, state) in &g.discovery.tiers {
                use crate::guided::TierState as T;
                let (mark, text, st) = match state {
                    T::Untested => ("  ", "not run".to_string(), DIM),
                    T::Running => (
                        "> ",
                        format!("measuring {}", progress_bar(g.discovery.progress, 20)),
                        WARN,
                    ),
                    T::Passed {
                        throughput_x,
                        p999_percent,
                    } => (
                        "ok",
                        format!(
                            "{throughput_x:.1}x real time, p99.9 at {p999_percent:.0}% of the block budget"
                        ),
                        OK,
                    ),
                    T::Failed {
                        throughput_x,
                        p999_percent,
                    } => (
                        "x ",
                        format!(
                            "fails the gates: {throughput_x:.1}x real time, p99.9 at {p999_percent:.0}%"
                        ),
                        ERR,
                    ),
                    T::Error { message: m } => ("! ", format!("not measured: {m}"), ERR),
                    T::Cancelled => ("  ", "cancelled".to_string(), DIM),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!(" {mark} "), st),
                    Span::raw(format!("{:<16}", g.tier_label(key))),
                    Span::styled(text, st),
                ]));
            }
            if app
                .guided
                .as_ref()
                .is_some_and(|g| g.discovery_stale(&app.draft))
            {
                lines.push(Line::styled(
                    "The quality settings changed since: these results are out of date.",
                    WARN,
                ));
            }
        }
    }
    let height = inner.height as usize;
    let scroll = if sel_range.1 > height {
        (sel_range.1 - height).min(sel_range.0)
    } else {
        0
    };
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), inner);
    if let Some(n) = &app.notice {
        f.render_widget(Paragraph::new(Line::styled(format!(" {n}"), WARN)), rows[2]);
    }
    let has_results = app
        .guided
        .as_ref()
        .is_some_and(|g| !g.discovery.is_empty() && view.id == "discover");
    let text = if discovering {
        "Esc cancel the benchmark".to_string()
    } else if has_results {
        "Enter continue · r run again · Backspace back · Esc to form".to_string()
    } else {
        let mut t = String::from("↑↓ choose · Enter select");
        if app.guided.as_ref().is_some_and(|g| g.can_go_back()) {
            t.push_str(" · Backspace back");
        }
        if matches!(
            view.kind,
            crate::guided::StepKind::AlsaDevice | crate::guided::StepKind::PipewireSink
        ) {
            t.push_str(" · r rescan");
        }
        t.push_str(" · Esc to form · q quit");
        t
    };
    f.render_widget(hints(&text), rows[3]);
}

fn status_line(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(n) = &app.notice {
        Line::styled(format!(" {n}"), WARN)
    } else if let Some(i) = app.issues().into_iter().find(|i| i.error) {
        Line::styled(
            format!(
                " Error{}: {}",
                i.path.map(|p| format!(" in {p}")).unwrap_or_default(),
                i.message
            ),
            ERR,
        )
    } else {
        Line::default()
    };
    f.render_widget(Paragraph::new(line), area);
}

// ---- form ----------------------------------------------------------------------------------

fn draw_form(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(Paragraph::new(header(app)), rows[0]);
    let cols = Layout::horizontal([Constraint::Length(38), Constraint::Min(0)]).split(rows[1]);
    draw_list(f, app, cols[0]);
    draw_help(f, app, cols[1]);
    status_line(f, app, rows[2]);
    f.render_widget(
        hints("Enter edit  Del reset  a assist  u use case  m mode  * hidden  r review  q quit"),
        rows[3],
    );
}

fn value_cell(app: &App, field: &Field) -> (String, Style) {
    if let Some(v) = app.set_value(&field.path) {
        return (show_value(v), BOLD);
    }
    match (&field.default, &field.default_text) {
        (Some(d), _) => (show_value(d), DIM),
        (None, Some(_)) => ("(auto)".into(), DIM),
        _ => ("-".into(), DIM),
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Settings ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = app.rows();
    let h = usize::from(inner.height);
    let top = (app.cursor + 1).saturating_sub(h);
    let issues = app.issues();
    let findings = app.capability_findings();
    let w = usize::from(inner.width);
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(top).take(h) {
        let focused = i == app.cursor;
        let line = match row {
            Row::Section(name) => {
                let meta = app.catalog.section(name);
                let arrow = if app.folded.contains(name) {
                    '▸'
                } else {
                    '▾'
                };
                let title = meta.map_or(name.as_str(), |m| m.title.as_str());
                let adv = if meta.is_some_and(|m| m.advanced) {
                    " (advanced)"
                } else {
                    ""
                };
                Line::styled(
                    pad(&format!("{arrow} {title}{adv}"), w),
                    if focused { selected() } else { BOLD },
                )
            }
            Row::Field(path) => {
                let Some(fd) = app.catalog.field(path) else {
                    continue;
                };
                let depth = path.matches('.').count().saturating_sub(1);
                let hidden = app.hidden_reason(fd).is_some();
                let error = issues
                    .iter()
                    .any(|x| x.error && x.path.as_deref() == Some(path));
                let warn = issues
                    .iter()
                    .any(|x| !x.error && x.path.as_deref() == Some(path))
                    || findings
                        .iter()
                        .any(|x| x.path == path && x.severity != Severity::Info);
                let (marker, mstyle) = if error {
                    ('!', ERR)
                } else if warn {
                    ('~', WARN)
                } else if app.is_required(fd) && app.set_value(path).is_none() {
                    ('?', Style::new().fg(Color::Magenta))
                } else if app.set_value(path).is_some() {
                    ('*', OK)
                } else {
                    (' ', Style::default())
                };
                let key = format!("{}{}", "  ".repeat(depth), fd.key);
                let key_w = 19usize;
                let (val, vstyle) = if fd.kind == FieldKind::Group {
                    (String::new(), DIM)
                } else if hidden {
                    ("(hidden)".to_string(), DIM)
                } else {
                    value_cell(app, fd)
                };
                let base = if hidden { DIM } else { Style::default() };
                let mut spans = vec![
                    Span::styled(marker.to_string(), mstyle),
                    Span::styled(
                        pad(&key, key_w),
                        if fd.kind == FieldKind::Group {
                            BOLD
                        } else {
                            base
                        },
                    ),
                    Span::raw(" "),
                    Span::styled(pad(&val, w.saturating_sub(key_w + 2)), vstyle),
                ];
                if focused {
                    for s in &mut spans {
                        s.style = s.style.patch(selected());
                    }
                }
                Line::from(spans)
            }
        };
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The field's own text: summary, then description.
fn field_text(fd: &Field, width: u16) -> Vec<Line<'static>> {
    let mut out = render(&fd.summary, width, BOLD);
    if !fd.description.trim().is_empty() {
        out.push(Line::default());
        out.extend(render(&fd.description, width, Style::default()));
    }
    out
}

/// What an option adds beyond its summary: the full description and the caution.
fn option_detail(o: &audigma_settings::meta::OptionMeta, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if !o.description.trim().is_empty() {
        out.extend(render(&o.description, width, Style::default()));
    }
    if let Some(c) = &o.caution {
        if !out.is_empty() {
            out.push(Line::default());
        }
        out.extend(render(&format!("Caution: {c}"), width, WARN));
    }
    out
}

/// Help text for the focused row; `option` focuses one option (a picker is open on it).
pub fn help_lines(app: &App, width: u16, option: Option<usize>) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    match app.focused() {
        None => return out,
        Some(Row::Section(name)) => {
            if let Some(m) = app.catalog.section(&name) {
                out.push(Line::from(inline_spans(&m.title, BOLD)));
                out.push(Line::default());
                out.extend(render(&m.summary, width, Style::default()));
                if !m.description.trim().is_empty() {
                    out.push(Line::default());
                    out.extend(render(&m.description, width, Style::default()));
                }
            }
            return out;
        }
        Some(Row::Field(_)) => {}
    }
    let Some(fd) = app.focused_field() else {
        return out;
    };
    out.extend(field_text(fd, width));
    // Options: the one the picker is on, else the one in effect.
    let current = app.resolved_value(&fd.path).and_then(option_text);
    let idx = option.or_else(|| current.and_then(|c| fd.options.iter().position(|o| o.key == c)));
    if let Some(o) = idx.and_then(|i| fd.options.get(i)) {
        out.push(Line::default());
        out.push(Line::from(vec![
            Span::styled(format!("{}: ", o.label), BOLD),
            Span::raw(o.summary.clone()),
        ]));
        let detail = option_detail(o, width);
        if !detail.is_empty() {
            out.push(Line::default());
            out.extend(detail);
        }
    } else if let (Some(n), Some(v)) = (&fd.also_integer, app.resolved_value(&fd.path)) {
        if v.is_i64() || v.is_u64() {
            out.push(Line::default());
            out.push(Line::styled(format!("{}: {}", n.label, n.summary), BOLD));
            if !n.description.trim().is_empty() {
                out.extend(render(&n.description, width, Style::default()));
            }
        }
    }
    out.push(Line::default());
    let default = match (&fd.default, &fd.default_text) {
        (Some(d), _) => show_value(d),
        (None, Some(t)) => t.clone(),
        _ => "none".to_string(),
    };
    let mut facts = format!("Default: {default}.");
    if let Some([lo, hi]) = fd.range {
        facts.push_str(&format!(" Range: {lo} to {hi}."));
    }
    out.extend(render(&facts, width, DIM));
    if let Some(uc) = app.use_case {
        if let Some(r) = fd.recommended.get(&uc) {
            out.push(Line::default());
            out.extend(render(
                &format!(
                    "Recommended for {}: `{}`. {}",
                    use_case_label(uc),
                    show_value(&r.value),
                    r.why
                ),
                width,
                Style::default().fg(Color::Cyan),
            ));
        }
    }
    if !fd.applies_to.is_empty() {
        let cmds: Vec<String> = fd
            .applies_to
            .iter()
            .map(|c| format!("{c:?}").to_lowercase())
            .collect();
        out.extend(render(
            &format!("Used by: {}.", cmds.join(", ")),
            width,
            DIM,
        ));
    }
    if let Some(reason) = app.hidden_reason(fd) {
        out.extend(render(&format!("Hidden: {reason}."), width, DIM));
    }
    // Assists, matches and findings.
    for id in &fd.assist {
        if let Some(a) = app.catalog.assist(id) {
            if a.kind == AssistKind::Preset {
                let m = app.matching_preset(id).unwrap_or_else(|| "Custom".into());
                out.push(Line::default());
                out.push(Line::styled(format!("Press a: {} (now: {m})", a.label), OK));
            } else if matches!(a.kind, AssistKind::Probe) {
                out.push(Line::default());
                out.push(Line::styled(format!("Press a: {}", a.label), OK));
            }
        }
    }
    for fi in app
        .capability_findings()
        .iter()
        .filter(|x| x.path == fd.path)
    {
        let st = match fi.severity {
            Severity::Error => ERR,
            Severity::Warning => WARN,
            Severity::Info => DIM,
        };
        out.push(Line::default());
        out.extend(render(&fi.message, width, st));
    }
    for i in app
        .issues()
        .iter()
        .filter(|x| x.path.as_deref() == Some(&fd.path))
    {
        out.push(Line::default());
        out.extend(render(&i.message, width, if i.error { ERR } else { WARN }));
    }
    out
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Help ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = help_lines(app, inner.width.saturating_sub(1), None);
    let max = lines.len().saturating_sub(usize::from(inner.height)) as u16;
    let scroll = app.help_scroll.min(max);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    if scroll < max {
        let r = Rect::new(inner.right().saturating_sub(12), area.bottom() - 1, 11, 1);
        f.render_widget(Paragraph::new(Span::styled(" ] more ", DIM)), r);
    }
}

// ---- popups --------------------------------------------------------------------------------

fn draw_popup(f: &mut Frame, app: &App, p: &Popup, area: Rect) {
    match p {
        Popup::ConfirmSkip => {
            let r = centered(area, 56, 6);
            f.render_widget(Clear, r);
            let b = Block::default()
                .borders(Borders::ALL)
                .title(" Skip to the form ");
            let inner = b.inner(r);
            f.render_widget(b, r);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from("Leave guided setup and keep the answers so far?"),
                    Line::from("The use-case recommendations are not applied."),
                    Line::styled("y / Enter skip · any other key stays", DIM),
                ]),
                inner,
            );
        }
        Popup::ConfirmQuit => {
            let r = centered(area, 46, 5);
            f.render_widget(Clear, r);
            let b = Block::default().borders(Borders::ALL).title(" Quit ");
            let inner = b.inner(r);
            f.render_widget(b, r);
            f.render_widget(
                Paragraph::new(vec![
                    Line::from("Discard the unsaved changes?"),
                    Line::styled("y / Enter discard · any other key stays", DIM),
                ]),
                inner,
            );
        }
        Popup::UseCase { sel } => {
            let r = centered(area, 70, 18);
            f.render_widget(Clear, r);
            let b = Block::default()
                .borders(Borders::ALL)
                .title(" What will you use audigma for? ");
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut lines = vec![
                Line::styled(
                    "This only highlights recommendations in the help pane and picks a",
                    DIM,
                ),
                Line::styled(
                    "mode. It never changes a value and is not written to the file.",
                    DIM,
                ),
                Line::default(),
            ];
            for (i, (u, desc)) in USE_CASES.iter().enumerate() {
                let st = if i == *sel {
                    selected()
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::raw(if i == *sel { "> " } else { "  " }),
                    Span::styled(use_case_label(*u).to_string(), st),
                ]));
                for l in render(desc, inner.width.saturating_sub(5), DIM) {
                    let mut spans = vec![Span::raw("    ")];
                    spans.extend(l.spans);
                    lines.push(Line::from(spans));
                }
                lines.push(Line::default());
            }
            let i = USE_CASES.len();
            let st = if i == *sel {
                selected()
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::raw(if i == *sel { "> " } else { "  " }),
                Span::styled("Skip", st),
            ]));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Popup::List { title, items, sel } => draw_list_popup(f, app, title, items, *sel, area),
        Popup::Input {
            title,
            hint,
            buf,
            error,
            ..
        } => {
            let r = centered(area, 64, 8);
            f.render_widget(Clear, r);
            let b = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "));
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut lines = render(hint, inner.width, DIM);
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::raw("> "),
                Span::styled(buf.clone(), BOLD),
                Span::styled("▏", Style::default().add_modifier(Modifier::SLOW_BLINK)),
            ]));
            if let Some(e) = error {
                lines.push(Line::default());
                lines.extend(render(e, inner.width, ERR));
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
    }
}

fn draw_list_popup(
    f: &mut Frame,
    app: &App,
    title: &str,
    items: &[ListItem],
    sel: usize,
    area: Rect,
) {
    // Full width (no background borders peeking out); the height follows the content.
    let inner_w = area.width.saturating_sub(2);
    let text_w = inner_w.saturating_sub(1);
    let avail = usize::from(area.height.saturating_sub(2).min(22).saturating_sub(2));

    // 1. What the key is (at most 6 lines), 2. the choices, 3. the focused option in full.
    let mut top: Vec<Line> = app
        .focused_field()
        .map(|fd| field_text(fd, text_w))
        .unwrap_or_default();
    top.truncate(6);
    let detail: Vec<Line> = items[sel]
        .option
        .and_then(|i| app.focused_field().and_then(|fd| fd.options.get(i)))
        .map(|o| option_detail(o, text_w))
        .unwrap_or_default();
    let detail_h = detail.len().min(6);
    let sep = |lines: &[Line]| usize::from(!lines.is_empty());
    let picker_h = avail
        .saturating_sub(top.len() + sep(&top))
        .saturating_sub(detail_h + usize::from(detail_h > 0))
        .max(3);

    // Each choice takes as many lines as its summary needs.
    let blocks: Vec<Vec<Line>> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mark = if i == sel { "> " } else { "  " };
            let head_style = if i == sel { selected() } else { BOLD };
            let mut lines = vec![Line::from(vec![
                Span::raw(mark),
                Span::styled(truncate(&it.label, usize::from(text_w) - 2), head_style),
            ])];
            if !it.summary.trim().is_empty() {
                for l in render(&it.summary, text_w.saturating_sub(4), DIM) {
                    let mut spans = vec![Span::raw("    ")];
                    spans.extend(l.spans);
                    lines.push(Line::from(spans));
                }
            }
            lines
        })
        .collect();
    let mut first = sel;
    let mut used = blocks[sel].len();
    while first > 0 && used + blocks[first - 1].len() <= picker_h {
        first -= 1;
        used += blocks[first].len();
    }
    let mut picker: Vec<Line> = Vec::new();
    for blk in &blocks[first..] {
        if picker.len() + blk.len() > picker_h {
            break;
        }
        picker.extend(blk.iter().cloned());
    }

    let rule = || Line::styled("─".repeat(usize::from(inner_w)), DIM);
    let mut lines = top;
    if !lines.is_empty() {
        lines.push(rule());
    }
    lines.extend(picker);
    if detail_h > 0 {
        lines.push(rule());
        lines.extend(detail.into_iter().take(detail_h));
    }
    let r = centered(area, area.width, lines.len() as u16 + 2);
    f.render_widget(Clear, r);
    let b = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    let inner = b.inner(r);
    f.render_widget(b, r);
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- review --------------------------------------------------------------------------------

fn draw_review(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(Paragraph::new(header(app)), rows[0]);
    let block = Block::default().borders(Borders::ALL).title(" Review ");
    let inner = block.inner(rows[1]);
    f.render_widget(block, rows[1]);
    let w = inner.width;
    let mut lines: Vec<Line> = Vec::new();

    if app.review_banner {
        lines.extend(render(
            "These are the values guided setup chose. Press f to edit any of them in the form.",
            w,
            OK,
        ));
        lines.push(Line::default());
    }
    lines.push(Line::styled("Problems", BOLD));
    let issues = app.issues();
    if issues.is_empty() {
        lines.push(Line::styled("  none", OK));
    }
    for i in &issues {
        let (mark, st) = if i.error { ("✗", ERR) } else { ("!", WARN) };
        let text = format!(
            "  {mark} {}{}",
            i.path
                .as_ref()
                .map(|p| format!("{p}: "))
                .unwrap_or_default(),
            i.message
        );
        lines.extend(render(&text, w, st));
    }

    lines.push(Line::default());
    lines.push(Line::styled("Device check", BOLD));
    match &app.probe {
        None if app.probe_running => lines.push(Line::styled("  probing...", DIM)),
        None => lines.push(Line::styled(
            "  press p to detect devices and check the configuration",
            DIM,
        )),
        Some(p) => match &p.alsa {
            Err(e) => lines.extend(render(&format!("  ALSA probe failed: {e}"), w, WARN)),
            Ok(_) => {
                let fs = app.capability_findings();
                if fs.is_empty() {
                    lines.push(Line::styled("  no findings", OK));
                }
                for x in fs {
                    let (mark, st) = match x.severity {
                        Severity::Error => ("✗", ERR),
                        Severity::Warning => ("!", WARN),
                        Severity::Info => ("i", DIM),
                    };
                    lines.extend(render(
                        &format!("  {mark} {}: {}", x.path, x.message),
                        w,
                        st,
                    ));
                }
            }
        },
    }

    lines.push(Line::default());
    lines.push(Line::styled("Validate on this machine (an estimate)", BOLD));
    let stale = app.bench_is_stale();
    match &app.bench {
        Bench::Idle => lines.push(Line::styled(
            "  press b to benchmark exactly this configuration",
            DIM,
        )),
        Bench::Running { stage, fraction } => {
            let n = (fraction.clamp(0.0, 1.0) * 30.0).round() as usize;
            let st = match stage {
                Stage::EndToEnd => "end to end",
                Stage::Sequential => "sequential",
                Stage::Workers => "workers",
            };
            lines.push(Line::from(format!(
                "  [{}{}] {st}  (c cancels)",
                "#".repeat(n),
                "-".repeat(30 - n)
            )));
        }
        Bench::Done(r) => {
            let (mark, st) = if r.passed() {
                ("pass", OK)
            } else {
                ("fail", ERR)
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(mark, st),
                Span::raw(format!(
                    ": {:.1}x real time (needs 2.0x), p99.9 {:.0}% of the block budget (max 70%)",
                    r.worker_throughput_x, r.p999_budget_percent
                )),
            ]));
            if stale {
                lines.push(Line::styled(
                    "  stale: the configuration changed since this run",
                    WARN,
                ));
            }
        }
        Bench::Failed(m) => lines.extend(render(&format!("  failed: {m}"), w, ERR)),
        Bench::Cancelled => lines.push(Line::styled("  cancelled", DIM)),
    }

    lines.push(Line::default());
    let touched: Vec<&String> = app.draft.touched().iter().collect();
    lines.push(Line::styled(
        format!("Changed keys ({})", touched.len()),
        BOLD,
    ));
    for p in &touched {
        let v = app
            .set_value(p)
            .map_or_else(|| "(reset to default)".to_string(), show_value);
        lines.push(Line::from(format!("  {p} = {v}")));
    }
    let total = lines.len() as u16;
    let max = total.saturating_sub(inner.height);
    app.review_max_scroll.set(max);
    let scroll = app.review_scroll.min(max);
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
    if max > 0 {
        let more = if scroll < max { "↓" } else { " " };
        let less = if scroll > 0 { "↑" } else { " " };
        let r = Rect::new(rows[1].right().saturating_sub(5), rows[1].y, 4, 1);
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {less}{more} "), DIM)),
            r,
        );
    }
    status_line(f, app, rows[2]);
    f.render_widget(
        hints("f form · p probe · b validate · g go to error · s save · ↑↓ scroll"),
        rows[3],
    );
}

// ---- save ----------------------------------------------------------------------------------

fn draw_save(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    f.render_widget(Paragraph::new(header(app)), rows[0]);
    let block = Block::default().borders(Borders::ALL).title(" Save ");
    let inner = block.inner(rows[1]);
    f.render_widget(block, rows[1]);
    let s = &app.save;
    let row = |i: usize, text: String| {
        Line::styled(
            format!("{} {text}", if s.focus == i { ">" } else { " " }),
            if s.focus == i {
                selected()
            } else {
                Style::default()
            },
        )
    };
    let level = |l: CommentLevel, name: &str| {
        if s.options.comment_level == l {
            format!("[{name}]")
        } else {
            format!(" {name} ")
        }
    };
    let mut lines = vec![
        row(0, format!("Write to: {}", s.path)),
        Line::styled(
            if s.exists {
                "    The file exists: it is updated in place (your # comments stay) after a backup."
            } else {
                "    The file does not exist: it is created."
            },
            DIM,
        ),
        Line::default(),
        row(
            1,
            format!(
                "[{}] Only write keys that differ from the defaults",
                if s.options.only_changed { "x" } else { " " }
            ),
        ),
        Line::default(),
        row(
            2,
            format!(
                "Comments: {} {} {}",
                level(CommentLevel::None, "none"),
                level(CommentLevel::Brief, "brief"),
                level(CommentLevel::Full, "full")
            ),
        ),
    ];
    lines.extend(render(
        "    Lines starting with `##` are generated and rewritten on every save. Use a single `#` for your own notes.",
        inner.width,
        DIM,
    ));
    lines.push(Line::default());
    lines.push(row(3, "Save".to_string()));
    match &s.outcome {
        None => {}
        Some(Err(e)) => {
            lines.push(Line::default());
            lines.extend(render(&format!("Could not save: {e}"), inner.width, ERR));
        }
        Some(Ok(r)) => {
            lines.push(Line::default());
            let what = if r.unchanged {
                "Nothing to change; the file is already up to date."
            } else if r.created {
                "Created the file."
            } else {
                "Updated the file."
            };
            lines.push(Line::styled(what, OK));
            if let Some(b) = &r.backup {
                lines.push(Line::from(format!("Backup: {}", b.display())));
            }
            lines.push(Line::default());
            lines.push(Line::from("Next:"));
            lines.push(Line::from(format!(
                "  audigma play -i <file> -c {}",
                s.path
            )));
            lines.push(Line::from(format!("  audigma run -c {}", s.path)));
            lines.push(Line::default());
            lines.push(Line::styled("Press q to exit.", DIM));
        }
    }
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Left), inner);
    status_line(f, app, rows[2]);
    f.render_widget(
        hints("↑↓ move · Enter/Space change · ←→ comment level · Esc back"),
        rows[3],
    );
}
