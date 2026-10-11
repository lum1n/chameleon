//! Width-aware line wrapping and a per-item cache of wrapped lines.

use std::collections::HashMap;

use unicode_width::UnicodeWidthStr;

use crate::style::{Line, Span};

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
    let max = max.max(4);
    let mut out: Vec<Line> = Vec::new();
    let mut cur = Line::default();
    let mut cur_w = 0;
    let indent_w = width(indent);

    let push_cur = |out: &mut Vec<Line>, cur: &mut Line, cur_w: &mut usize| {
        trim_end(cur);
        out.push(std::mem::take(cur));
        *cur = Line::new(vec![Span::raw(indent)]);
        *cur_w = indent_w;
    };

    for span in &line.spans {
        // Split the span's text into words with their trailing spaces.
        let mut word = String::new();
        let flush = |word: &mut String, out: &mut Vec<Line>, cur: &mut Line, cur_w: &mut usize| {
            if word.is_empty() {
                return;
            }
            let ww = width(word);
            if *cur_w + ww > max && *cur_w > indent_w {
                push_cur(out, cur, cur_w);
            }
            if *cur_w + ww <= max {
                cur.push(Span::new(std::mem::take(word), span.style));
                *cur_w += ww;
                return;
            }
            // Longer than a line: hard-split by columns.
            let mut piece = String::new();
            let mut pw = 0;
            for c in word.chars() {
                let cw = width(c.encode_utf8(&mut [0u8; 4]));
                if *cur_w + pw + cw > max {
                    cur.push(Span::new(std::mem::take(&mut piece), span.style));
                    *cur_w += pw;
                    pw = 0;
                    push_cur(out, cur, cur_w);
                }
                piece.push(c);
                pw += cw;
            }
            if !piece.is_empty() {
                cur.push(Span::new(piece, span.style));
                *cur_w += pw;
            }
            word.clear();
        };
        for c in span.text.chars() {
            match c {
                '\n' => {
                    flush(&mut word, &mut out, &mut cur, &mut cur_w);
                    push_cur(&mut out, &mut cur, &mut cur_w);
                }
                ' ' => {
                    word.push(' ');
                    flush(&mut word, &mut out, &mut cur, &mut cur_w);
                }
                c => word.push(c),
            }
        }
        flush(&mut word, &mut out, &mut cur, &mut cur_w);
    }
    trim_end(&mut cur);
    if !cur.spans.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
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
