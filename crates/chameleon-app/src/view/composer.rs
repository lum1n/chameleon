//! The prompt editor's pane: a `❯` and the text, with a hint while empty.

use crate::composer::Composer;
use crate::model::{App, Pane};
use crate::style::{Line, Span, Style};
use crate::view::{PaneView, Rect};
use crate::wrap::{width, wrap_line};

/// Columns taken by the prompt glyph and its space.
pub const PROMPT_W: usize = 2;

/// The composer's text wrapped to `w` columns, plus the cursor's (line, col).
pub fn wrapped(c: &Composer, w: usize) -> (Vec<Line>, (usize, usize)) {
    let (cl, cc) = c.cursor_pos();
    let mut lines = Vec::new();
    let mut cursor = (0, 0);
    for (i, src) in c.text().split('\n').enumerate() {
        let wrapped = wrap_line(&Line::raw(src), w, "");
        if i == cl {
            // Find the wrapped row holding column `cc`.
            let mut col = cc;
            let mut row = 0;
            for (r, l) in wrapped.iter().enumerate() {
                let n = l.text().chars().count();
                // Rows keep trailing spaces trimmed; count the source instead.
                if col <= n || r + 1 == wrapped.len() {
                    row = r;
                    break;
                }
                col -= n + 1;
            }
            let prefix: String = wrapped
                .get(row)
                .map(|l| l.text().chars().take(col).collect())
                .unwrap_or_default();
            cursor = (lines.len() + row, width(&prefix));
        }
        lines.extend(wrapped);
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    (lines, cursor)
}

pub fn wrapped_lines(c: &Composer, w: usize) -> Vec<Line> {
    wrapped(c, w).0
}

pub fn build(app: &App, rect: Rect) -> PaneView {
    let theme = app.theme;
    let w = (rect.w as usize).saturating_sub(PROMPT_W).max(1);
    let (lines, (cl, cc)) = wrapped(&app.composer, w);
    let focused = app.focus == Pane::Composer;
    let h = (rect.h as usize).max(1);
    let offset = cl.saturating_sub(h - 1);
    let prompt = Style::new()
        .fg(if focused { theme.accent } else { theme.muted })
        .bold();
    let mut out: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .skip(offset)
        .take(h)
        .map(|(i, l)| {
            let mut line = Line::new(vec![Span::new(if i == 0 { "❯ " } else { "  " }, prompt)]);
            line.spans.extend(l.spans);
            line
        })
        .collect();
    if app.composer.is_empty() {
        let hint = match app.selected() {
            Some(s) if s.session.is_some() && s.is_alive() => {
                if focused {
                    "say something · enter sends · alt-enter adds a line · / commands"
                } else {
                    "type to prompt · / commands"
                }
            }
            Some(_) => "this session isn't running · n starts a new one",
            None => "n starts a session",
        };
        if let Some(first) = out.first_mut() {
            first.push(Span::new(hint, Style::new().fg(theme.muted).italic()));
        }
    }
    let cursor = focused.then(|| {
        (
            rect.x
                .saturating_add(PROMPT_W as u16)
                .saturating_add(cc.min(w.saturating_sub(1)) as u16),
            rect.y.saturating_add((cl - offset) as u16),
        )
    });
    PaneView {
        rect,
        lines: out,
        focused,
        cursor,
        scrollbar: None,
        hits: Vec::new(),
    }
}
