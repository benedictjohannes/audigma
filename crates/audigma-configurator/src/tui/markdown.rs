//! Renders the metadata markdown subset (§4.4: paragraphs, `- ` bullets, `` `code` ``,
//! `*emphasis*`) as wrapped, styled ratatui lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Style of `code` spans.
pub fn code_style() -> Style {
    Style::default().fg(Color::Yellow)
}

/// A styled word; `\u{a0}` stands for a space that must not break (inside code spans).
struct Word {
    text: String,
    style: Style,
    /// No space before this word (it continues the previous one, e.g. punctuation after code).
    glued: bool,
}

fn inline(text: &str, base: Style) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let (mut code, mut emph) = (false, false);
    let mut cur = String::new();
    // The next word continues the previous one unless whitespace came between them.
    let mut glued = false;

    let style = |code: bool, emph: bool| {
        let mut st = base;
        if code {
            st = st.patch(code_style());
        }
        if emph {
            st = st.add_modifier(Modifier::ITALIC);
        }
        st
    };
    let flush = |words: &mut Vec<Word>, cur: &mut String, st: Style, glued: &mut bool| {
        if !cur.is_empty() {
            words.push(Word {
                text: std::mem::take(cur),
                style: st,
                glued: *glued,
            });
            *glued = true;
        }
    };

    for ch in text.chars() {
        match ch {
            '`' => {
                flush(&mut words, &mut cur, style(code, emph), &mut glued);
                code = !code;
            }
            '*' if !code => {
                flush(&mut words, &mut cur, style(code, emph), &mut glued);
                emph = !emph;
            }
            c if c.is_whitespace() => {
                if code {
                    cur.push('\u{a0}');
                } else {
                    flush(&mut words, &mut cur, style(code, emph), &mut glued);
                    glued = false;
                }
            }
            c => cur.push(c),
        }
    }
    flush(&mut words, &mut cur, style(code, emph), &mut glued);
    // The first word never has anything to be glued to.
    if let Some(w) = words.first_mut() {
        w.glued = false;
    }
    words
}

fn wrap(words: Vec<Word>, width: usize, first: &str, rest: &str, out: &mut Vec<Line<'static>>) {
    let width = width.max(8);
    let mut line: Vec<Span<'static>> = vec![Span::raw(first.to_string())];
    let mut len = first.chars().count();
    let mut empty = true;
    for w in words {
        let text = w.text.replace('\u{a0}', " ");
        let n = text.chars().count();
        let sep = usize::from(!empty && !w.glued);
        if !empty && len + sep + n > width {
            out.push(Line::from(std::mem::take(&mut line)));
            line = vec![Span::raw(rest.to_string())];
            len = rest.chars().count();
            empty = true;
        }
        if !empty && !w.glued {
            line.push(Span::raw(" "));
            len += 1;
        }
        line.push(Span::styled(text, w.style));
        len += n;
        empty = false;
    }
    out.push(Line::from(line));
}

/// Renders `md` for a pane `width` columns wide.
pub fn render(md: &str, width: u16, base: Style) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let mut out: Vec<Line<'static>> = Vec::new();
    for block in md.trim().split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Line::default());
        }
        let mut items: Vec<(bool, String)> = Vec::new();
        for line in block.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("- ") {
                items.push((true, rest.to_string()));
            } else if let Some(last) = items.last_mut() {
                last.1.push(' ');
                last.1.push_str(t);
            } else {
                items.push((false, t.to_string()));
            }
        }
        for (bullet, text) in items {
            let (first, rest) = if bullet { ("- ", "  ") } else { ("", "") };
            wrap(inline(&text, base), width, first, rest, &mut out);
        }
    }
    out
}

/// One-line inline rendering (summaries); not wrapped.
pub fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, w) in inline(text, base).into_iter().enumerate() {
        if i > 0 && !w.glued {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(w.text.replace('\u{a0}', " "), w.style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wraps_paragraphs_and_hangs_bullets() {
        let md = "Short intro that wraps around the pane.\n\n- `a` first item that is long enough to wrap\n- `b` second";
        let got = plain(&render(md, 24, Style::default()));
        assert_eq!(
            got,
            vec![
                "Short intro that wraps",
                "around the pane.",
                "",
                "- a first item that is",
                "  long enough to wrap",
                "- b second",
            ]
        );
    }

    #[test]
    fn code_spans_stay_whole_and_keep_punctuation_attached() {
        let got = plain(&render(
            "Use `device_wait_ms = -1`, then stop.",
            20,
            Style::default(),
        ));
        assert_eq!(got, vec!["Use", "device_wait_ms = -1,", "then stop."]);
    }

    #[test]
    fn code_and_emphasis_are_styled() {
        let lines = render("a `b` *c*", 40, Style::default());
        let styles: Vec<_> = lines[0].spans.iter().map(|s| s.style).collect();
        assert!(styles.contains(&code_style()));
        assert!(
            styles
                .iter()
                .any(|s| s.add_modifier.contains(Modifier::ITALIC))
        );
    }
}
