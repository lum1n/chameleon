//! Width-aware line wrapping and a per-item cache of wrapped lines.

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use crate::style::{Line, Span, Style};

/// Display width of text, tabs counted as four columns.
pub fn width(s: &str) -> usize {
    s.chars()
        .map(|c| match c {
            '\t' => 4,
            c if c.is_control() => 0,
            c => {
                let mut b = [0u8; 4];
                UnicodeWidthStr::width(&*c.encode_utf8(&mut b))
            }
        })
        .sum()
}

/// Truncates `s` to at most `max` columns, adding `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = width(c.encode_utf8(&mut [0u8; 4]));
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// A path cut from the left to at most `max` columns, so its last
/// directories stay: `…/repos/sessh`. Cuts at a `/` when one fits.
pub fn truncate_path(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut tail = String::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = width(c.encode_utf8(&mut [0u8; 4]));
        if w + cw > max.saturating_sub(1) {
            break;
        }
        tail.insert(0, c);
        w += cw;
    }
    if let Some(i) = tail.find('/')
        && tail[i + 1..].contains(|c| c != '/')
    {
        tail.drain(..i);
    }
    format!("…{tail}")
}

/// Pads or truncates to exactly `cols` columns.
pub fn fit(s: &str, cols: usize) -> String {
    let mut t = truncate(s, cols);
    let w = width(&t);
    if w < cols {
        t.extend(std::iter::repeat_n(' ', cols - w));
    }
    t
}

/// Wraps one styled line to `max` columns at spaces (or anywhere, for a
/// word longer than the line). Spans keep their styles across the break.
/// `indent` prefixes every continuation line.
pub fn wrap_line(line: &Line, max: usize, indent: &str) -> Vec<Line> {
    wrap_hang(line, max, &Line::raw(indent))
}

/// [`wrap_line`] with a styled prefix on continuation lines (a code
/// block's gutter, a quote's bar).
pub fn wrap_hang(line: &Line, max: usize, hang: &Line) -> Vec<Line> {
    let max = max.max(4);
    let mut out: Vec<Line> = Vec::new();
    let mut cur = Line::default();
    let mut cur_w = 0;
    let indent_w = hang.width();
    let hang = Line::new(
        hang.spans
            .iter()
            .filter(|s| !s.text.is_empty())
            .cloned()
            .collect(),
    );

    let push_cur = |out: &mut Vec<Line>, cur: &mut Line, cur_w: &mut usize| {
        trim_end(cur);
        out.push(std::mem::take(cur));
        *cur = hang.clone();
        *cur_w = indent_w;
    };

    // A word (with its trailing space) may cross spans — `code`, then a
    // comma — so it is kept as styled characters until it is placed.
    let mut word: Vec<(char, Style)> = Vec::new();
    let flush = |word: &mut Vec<(char, Style)>, out: &mut Vec<Line>, cur: &mut Line, cur_w: &mut usize| {
        if word.is_empty() {
            return;
        }
        let cw = |c: char| width(c.encode_utf8(&mut [0u8; 4]));
        let ww: usize = word.iter().map(|(c, _)| cw(*c)).sum();
        if *cur_w + ww > max && *cur_w > indent_w {
            push_cur(out, cur, cur_w);
        }
        // Longer than a line: hard-split by columns.
        for (c, style) in word.drain(..) {
            if *cur_w + cw(c) > max && *cur_w > indent_w {
                if c == ' ' {
                    continue; // never start a line with the break's space
                }
                push_cur(out, cur, cur_w);
            }
            push_char(cur, c, style);
            *cur_w += cw(c);
        }
    };
    for span in &line.spans {
        for c in span.text.chars() {
            match c {
                '\n' => {
                    flush(&mut word, &mut out, &mut cur, &mut cur_w);
                    push_cur(&mut out, &mut cur, &mut cur_w);
                }
                ' ' => {
                    word.push((' ', span.style));
                    flush(&mut word, &mut out, &mut cur, &mut cur_w);
                }
                c => word.push((c, span.style)),
            }
        }
    }
    flush(&mut word, &mut out, &mut cur, &mut cur_w);
    trim_end(&mut cur);
    if !cur.spans.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

/// Appends to the last span when it has the same style.
fn push_char(line: &mut Line, c: char, style: Style) {
    match line.spans.last_mut() {
        Some(last) if last.style == style => last.text.push(c),
        _ => line.push(Span::new(c.to_string(), style)),
    }
}

fn trim_end(line: &mut Line) {
    while let Some(last) = line.spans.last_mut() {
        let t = last.text.trim_end_matches(' ');
        if t.len() == last.text.len() {
            break;
        }
        last.text.truncate(t.len());
        if last.text.is_empty() {
            line.spans.pop();
        } else {
            break;
        }
    }
}

/// Wrapped lines of one transcript item, keyed by item id and valid for
/// one width and item revision.
#[derive(Debug, Default)]
pub struct WrapCache {
    entries: HashMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    width: usize,
    revision: u64,
    lines: Vec<Line>,
}

impl WrapCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached lines, or the result of `build`, cached when `cacheable`.
    pub fn get_or_build(
        &mut self,
        id: &str,
        width: usize,
        revision: u64,
        cacheable: bool,
        build: impl FnOnce() -> Vec<Line>,
    ) -> &[Line] {
        let fresh = self
            .entries
            .get(id)
            .is_some_and(|e| e.width == width && e.revision == revision);
        if !fresh {
            let lines = build();
            if cacheable {
                self.entries.insert(
                    id.to_owned(),
                    Entry {
                        width,
                        revision,
                        lines,
                    },
                );
            } else {
                self.entries.insert(
                    id.to_owned(),
                    Entry {
                        width,
                        revision: u64::MAX, // never fresh: rebuilt next frame
                        lines,
                    },
                );
            }
        }
        self.entries.get(id).map(|e| e.lines.as_slice()).unwrap_or(&[])
    }

    /// The lines cached for `id` by the last `get_or_build`, whatever their revision.
    pub fn get(&self, id: &str) -> &[Line] {
        self.entries.get(id).map(|e| e.lines.as_slice()).unwrap_or(&[])
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn paths_keep_their_last_directories() {
        assert_eq!(truncate_path("~/repos/sessh", 20), "~/repos/sessh");
        assert_eq!(truncate_path("/home/vegard/repos/sessh", 16), "…/repos/sessh");
        assert_eq!(truncate_path("/home/vegard/repos/sessh", 8), "…/sessh");
        assert_eq!(truncate_path("/home/vegard/repos/chameleon", 6), "…eleon");
    }

    #[test]
    fn wraps_at_spaces_and_hard_splits_long_words() {
        let l = Line::raw("the quick brown fox jumps");
        assert_eq!(
            texts(&wrap_line(&l, 10, "")),
            vec!["the quick", "brown fox", "jumps"]
        );
        let l = Line::raw("abcdefghijklmnop");
        assert_eq!(texts(&wrap_line(&l, 6, "")), vec!["abcdef", "ghijkl", "mnop"]);
        let l = Line::raw("a\nb");
        assert_eq!(texts(&wrap_line(&l, 6, "  ")), vec!["a", "  b"]);
    }

    #[test]
    fn words_cross_spans() {
        // "use `fold`, then" must not leave the comma alone on a line.
        let code = Style::new().bold();
        let l = Line::new(vec![
            Span::raw("use "),
            Span::new("fold", code),
            Span::raw(", then"),
        ]);
        let out = wrap_line(&l, 8, "");
        assert_eq!(texts(&out), vec!["use", "fold,", "then"]);
        assert_eq!(out[1].spans[0].style, code, "the word keeps both styles");
        assert_eq!(out[1].spans[1].text, ",");
        let hang = Line::styled("│ ", code);
        assert_eq!(
            texts(&wrap_hang(&Line::raw("abcdefgh"), 6, &hang)),
            vec!["abcdef", "│ gh"]
        );
    }

    #[test]
    fn widths_and_truncation() {
        assert_eq!(width("héllo"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(truncate("hello world", 7), "hello …");
        assert_eq!(fit("hi", 4), "hi  ");
    }

    #[test]
    fn cache_invalidates_on_width_and_revision() {
        let mut c = WrapCache::new();
        let mut builds = 0;
        for (w, r) in [(10, 1), (10, 1), (12, 1), (12, 2)] {
            c.get_or_build("x", w, r, true, || {
                builds += 1;
                vec![Line::raw("a")]
            });
        }
        assert_eq!(builds, 3);
    }
}
