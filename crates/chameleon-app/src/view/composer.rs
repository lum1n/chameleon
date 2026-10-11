//! The prompt editor's pane.

use crate::composer::Composer;
use crate::model::{App, Pane};
use crate::style::{Line, Style};
use crate::view::{PaneView, Rect};
use crate::wrap::{width, wrap_line};

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

pub fn build(app: &App, rect: Rect, _lines: Vec<Line>) -> PaneView {
    let inner = rect.inner();
    let w = (inner.w as usize).max(1);
    let (mut lines, (cl, cc)) = wrapped(&app.composer, w);
    let focused = app.focus == Pane::Composer;
    let h = inner.h as usize;
    let offset = cl.saturating_sub(h.saturating_sub(1));
    lines = lines.into_iter().skip(offset).take(h.max(1)).collect();
    let cursor = focused.then(|| {
        (
            inner.x.saturating_add(cc.min(w.saturating_sub(1)) as u16),
            inner.y.saturating_add((cl - offset) as u16),
        )
    });
    let hint = match app.selected() {
        Some(s) if s.session.is_some() && s.is_alive() => " enter: send · alt-enter: newline · /: commands ",
        Some(_) => " session not running · n: new ",
        None => " n: new session ",
    };
    if app.composer.is_empty() && !focused {
        lines = vec![Line::styled(hint.trim(), Style::new().fg(app.theme.muted))];
    }
    PaneView {
        rect,
        title: Line::styled(
            if focused { " prompt " } else { "" },
            Style::new().fg(app.theme.muted),
        ),
        lines,
        focused,
        cursor,
        scrollbar: None,
        hits: Vec::new(),
    }
}
