//! Mouse selection and links over the lines of one frame.
//!
//! Positions are `(column, row)` within a pane. A selection runs in reading
//! order from one cell to another, both included; its text leaves out
//! what [`Style::deco`] marks (bars, gutters, borders), so a copied code
//! block is the code. Links are the characters [`crate::wrap`] marked with
//! one id, wherever a wrap put them.

use unicode_width::UnicodeWidthChar;

use crate::style::{Line, Span, Style};

/// A drag over a pane, in its own cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Where the button went down.
    pub anchor: (u16, u16),
    /// Where it is now.
    pub head: (u16, u16),
    /// The mouse moved with the button down: a click is a selection now.
    pub dragged: bool,
}

impl Selection {
    pub fn at(pos: (u16, u16)) -> Self {
        Self {
            anchor: pos,
            head: pos,
            dragged: false,
        }
    }

    /// The first and last cells, in reading order.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (a, h) = (self.anchor, self.head);
        if (a.1, a.0) <= (h.1, h.0) { (a, h) } else { (h, a) }
    }

    /// The columns of `row` it covers, `start..end`.
    fn columns(&self, row: u16) -> Option<(usize, usize)> {
        let (from, to) = self.ordered();
        if row < from.1 || row > to.1 {
            return None;
        }
        let start = if row == from.1 { from.0 as usize } else { 0 };
        let end = if row == to.1 {
            to.0 as usize + 1
        } else {
            usize::MAX
        };
        Some((start, end))
    }
}

/// Each character of `line` with the column it starts at and its width.
fn cells(line: &Line) -> impl Iterator<Item = (usize, usize, char, Style)> + '_ {
    let mut col = 0;
    line.spans
        .iter()
        .flat_map(|s| s.text.chars().map(move |c| (c, s.style)))
        .map(move |(c, style)| {
            let w = if c == '\t' { 4 } else { c.width().unwrap_or(0) };
            let at = col;
            col += w;
            (at, w, c, style)
        })
}

/// The selected text: rows joined by newlines, each without its
/// decoration and trailing spaces. Rows that were only a rule (a code
/// fence, a card's edge) are dropped; a gutter alone is a blank line.
pub fn text(lines: &[Line], sel: &Selection) -> String {
    let mut rows = Vec::new();
    for (r, line) in lines.iter().enumerate() {
        let Some((start, end)) = sel.columns(r as u16) else {
            continue;
        };
        let mut row = String::new();
        let mut only_deco = true;
        for (at, w, c, style) in cells(line) {
            if at + w.max(1) <= start || at >= end {
                continue;
            }
            if style.deco {
                continue;
            }
            only_deco = false;
            row.push(c);
        }
        if only_deco && line.spans.iter().any(|s| s.text.contains('─')) {
            continue;
        }
        rows.push(row.trim_end().to_owned());
    }
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows.join("\n")
}

/// Shows the selection: its cells reversed.
pub fn highlight(lines: &mut [Line], sel: &Selection) {
    for (r, line) in lines.iter_mut().enumerate() {
        let Some((start, end)) = sel.columns(r as u16) else {
            continue;
        };
        let mut out = Line::default();
        for (at, w, c, mut style) in cells(line).collect::<Vec<_>>() {
            if at + w.max(1) > start && at < end {
                style.reversed = !style.reversed;
            }
            match out.spans.last_mut() {
                Some(last) if last.style == style => last.text.push(c),
                _ => out.push(Span::new(c.to_string(), style)),
            }
        }
        *line = out;
    }
}

/// The URL under `pos`, put back together from every row it was wrapped
/// over.
pub fn link_at(lines: &[Line], pos: (u16, u16)) -> Option<String> {
    let line = lines.get(pos.1 as usize)?;
    let col = pos.0 as usize;
    let id = cells(line)
        .find(|(at, w, ..)| col >= *at && col < at + (*w).max(1))
        .map(|(.., style)| style.link)
        .filter(|id| *id != 0)?;
    let url: String = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.style.link == id)
        .map(|s| s.text.as_str())
        .collect();
    Some(url).filter(|u| u.starts_with("http://") || u.starts_with("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrap::wrap_hang;

    fn sel(anchor: (u16, u16), head: (u16, u16)) -> Selection {
        Selection {
            anchor,
            head,
            dragged: true,
        }
    }

    #[test]
    fn a_wrapped_url_opens_whole_and_its_punctuation_stays_out() {
        let gutter = Line::styled("│ ", Style::new().deco());
        let mut l = gutter.clone();
        l.push(Span::raw("see https://example.com/a/very/long/path?q=1, then go"));
        let lines = wrap_hang(&l, 20, &gutter);
        assert!(lines.len() > 2, "{lines:?}");
        let url = "https://example.com/a/very/long/path?q=1";
        // "see" stays on the first row; the URL is split over the next ones.
        assert_eq!(link_at(&lines, (2, 0)), None);
        assert_eq!(link_at(&lines, (3, 1)).as_deref(), Some(url));
        assert_eq!(
            link_at(&lines, (3, 2)).as_deref(),
            Some(url),
            "from a later row too"
        );
        assert_eq!(link_at(&lines, (0, 1)), None, "the gutter isn't the link");
        let last = lines.len() - 1;
        assert_eq!(
            link_at(&lines, (lines[last].width() as u16 - 1, last as u16)),
            None
        );
    }

    #[test]
    fn brackets_the_url_did_not_open_are_the_sentence_s() {
        let lines = wrap_hang(
            &Line::raw("(docs: https://en.wikipedia.org/wiki/Rust_(language)) ok"),
            80,
            &Line::default(),
        );
        assert_eq!(
            link_at(&lines, (10, 0)).as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        let bare = wrap_hang(&Line::raw("https:// alone"), 80, &Line::default());
        assert_eq!(link_at(&bare, (2, 0)), None, "a scheme alone isn't a link");
    }

    #[test]
    fn copied_text_leaves_the_gutters_and_borders_out() {
        let bar = Style::new().deco();
        let lines = vec![
            Line::styled("╭─ rs", bar),
            Line::new(vec![Span::new("│ ", bar), Span::raw("let x = 1;")]),
            Line::styled("│", bar),
            Line::new(vec![Span::new("│ ", bar), Span::raw("let y = 2;   ")]),
            Line::styled("╰─", bar),
            Line::default(),
        ];
        assert_eq!(text(&lines, &sel((0, 0), (30, 4))), "let x = 1;\n\nlet y = 2;");
        // Backwards, and from the middle of a row.
        assert_eq!(text(&lines, &sel((6, 1), (5, 3))), "x = 1;\n\nlet");
    }

    #[test]
    fn the_highlight_reverses_just_the_selected_cells() {
        let mut lines = vec![Line::raw("hello world"), Line::raw("second")];
        highlight(&mut lines, &sel((6, 0), (2, 1)));
        let reversed: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .filter(|s| s.style.reversed)
                    .map(|s| s.text.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(reversed, vec!["world".to_owned(), "sec".to_owned()]);
        assert_eq!(lines[0].text(), "hello world");
    }
}
