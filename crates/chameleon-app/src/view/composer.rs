//! The prompt editor's pane: a dock under the transcript with a `❯` and
//! the text, padded on every side, a hint while empty and the keys while
//! typing.
//!
//! ```text
//! ├──────────────────────────────────────────────
//!
//!   ❯ ▣ shot.png 240 KB  ▣ clipboard 1.2 MB
//!     fix the failing tests and explain what
//!     broke
//!                     enter send · alt-enter line
//! ```

use crate::composer::Composer;
use crate::model::{App, Pane};
use crate::style::{Color, Line, Span, Style};
use crate::view::{PaneView, Rect};
use crate::wrap::{width, wrap_line};

/// Columns taken by the prompt glyph and its space.
pub const PROMPT_W: usize = 2;
/// Columns of air left of the prompt and right of the text.
pub const PAD_X: usize = 2;
/// Rows of air above and below the text.
pub const PAD_Y: usize = 1;

/// The width text wraps to in a composer `w` columns wide.
pub fn text_width(w: usize) -> usize {
    w.saturating_sub(2 * PAD_X + PROMPT_W).max(1)
}

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
            // Wrapping trims trailing spaces; the cursor still steps over them.
            let spaces = col.saturating_sub(prefix.chars().count());
            cursor = (lines.len() + row, width(&prefix) + spaces);
        }
        lines.extend(wrapped);
    }
    if lines.is_empty() {
        lines.push(Line::default());
    }
    (lines, cursor)
}

/// The rows the composer's content takes: its images, then its text.
pub fn rows(c: &Composer, w: usize) -> usize {
    wrapped(c, w).0.len() + usize::from(!c.images().is_empty())
}

/// The images, one row of `▣ name size`, cut to `w` columns.
fn image_row(c: &Composer, w: usize, style: Style) -> Line {
    let text = c
        .images()
        .iter()
        .map(|i| format!("▣ {}", i.label()))
        .collect::<Vec<_>>()
        .join("  ");
    let text = if width(&text) > w {
        let mut cut = String::new();
        for ch in text.chars() {
            if width(&cut) + 2 > w {
                break;
            }
            cut.push(ch);
        }
        cut.push('…');
        cut
    } else {
        text
    };
    Line::new(vec![Span::new(text, style)])
}

pub fn build(app: &App, rect: Rect) -> PaneView {
    let theme = app.theme;
    let w = text_width(rect.w as usize);
    let (mut lines, (cl, cc)) = wrapped(&app.composer, w);
    let has_images = !app.composer.images().is_empty();
    if has_images {
        lines.insert(0, image_row(&app.composer, w, Style::new().fg(theme.accent)));
    }
    let cl = cl + usize::from(has_images);
    let focused = app.focus == Pane::Composer;
    let muted = Style::new().fg(theme.muted);
    // Too short for air: the text alone.
    let pad = if rect.h as usize > 2 * PAD_Y { PAD_Y } else { 0 };
    let h = (rect.h as usize).saturating_sub(2 * pad).max(1);
    let offset = cl.saturating_sub(h - 1);
    let prompt = Style::new()
        .fg(if focused { theme.accent } else { theme.muted })
        .bold();
    let indent = " ".repeat(PAD_X);
    let mut out = vec![Line::default(); pad];
    out.extend(lines.iter().enumerate().skip(offset).take(h).map(|(i, l)| {
        let glyph = if i == 0 { "❯ " } else { "  " };
        let mut line = Line::new(vec![Span::raw(indent.clone()), Span::new(glyph, prompt)]);
        line.spans.extend(l.spans.iter().cloned());
        line
    }));
    if app.composer.is_empty() {
        let hint = match app.selected() {
            Some(s) if s.session.is_some() && s.is_alive() => {
                if focused {
                    "say something · / commands"
                } else {
                    "type to prompt · / commands"
                }
            }
            Some(_) => "this session isn't running · n starts a new one",
            None => "n starts a session",
        };
        if let Some(first) = out.get_mut(pad) {
            first.push(Span::new(hint, muted.italic()));
        }
    }
    if pad > 0 {
        // The keys, right-aligned in the bottom air; how much is above
        // when the text scrolls.
        let mut keys = Vec::new();
        if offset > 0 {
            keys.push(format!("↑ {offset} more"));
        }
        if focused {
            keys.push("enter send · alt-enter line".to_owned());
            keys.push(if has_images && app.composer.cursor() == 0 {
                "backspace drops an image".to_owned()
            } else {
                "ctrl-v image".to_owned()
            });
        }
        let keys = keys.join(" · ");
        let mut bottom = Line::default();
        if !keys.is_empty() {
            let room = (rect.w as usize).saturating_sub(PAD_X);
            bottom.push(Span::raw(" ".repeat(room.saturating_sub(width(&keys)))));
            bottom.push(Span::new(keys, muted));
        }
        out.push(bottom);
    }
    let cursor = focused.then(|| {
        (
            rect.x
                .saturating_add((PAD_X + PROMPT_W) as u16)
                .saturating_add(cc.min(w.saturating_sub(1)) as u16),
            rect.y.saturating_add((pad + cl - offset) as u16),
        )
    });
    PaneView {
        rect,
        lines: out,
        focused,
        cursor,
        scrollbar: None,
        hits: Vec::new(),
        // A raised dock where the theme has a surface colour.
        bg: (theme.surface != Color::Reset).then_some(theme.surface),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_steps_over_trailing_spaces() {
        let mut c = Composer::default();
        c.insert("hi");
        assert_eq!(wrapped(&c, 40).1, (0, 2));
        c.insert("   ");
        assert_eq!(wrapped(&c, 40).1, (0, 5));
        c.insert("\nx ");
        assert_eq!(wrapped(&c, 40).1, (1, 2));
    }
}
