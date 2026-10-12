//! Markdown-lite: enough structure for agent messages (headings, fenced
//! code, tables, lists, quotes, links, inline code, bold, italics),
//! rendered and wrapped to a width. Everything else is passed through as
//! text.

use crate::style::{Color, Line, Span, Style};
use crate::wrap::wrap_hang;

#[derive(Debug, Clone, Copy)]
pub struct MdStyle {
    pub base: Style,
    pub code: Style,
    pub heading: Style,
    pub quote: Style,
    pub bullet: Style,
    /// Rules, gutters, table lines and link targets.
    pub muted: Style,
    pub link: Style,
}

impl Default for MdStyle {
    fn default() -> Self {
        Self {
            base: Style::new(),
            code: Style::new().fg(Color::Cyan),
            heading: Style::new().bold().fg(Color::Magenta),
            quote: Style::new().italic(),
            bullet: Style::new().fg(Color::Magenta),
            muted: Style::new().fg(Color::DarkGray),
            link: Style::new().fg(Color::Blue).underline(),
        }
    }
}

/// Renders markdown wrapped to `w` columns. Paragraphs wrap flush left,
/// list items hang under their text, code keeps its gutter.
pub fn render(text: &str, s: &MdStyle, w: usize) -> Vec<Line> {
    let w = w.max(8);
    let mut out: Vec<Line> = Vec::new();
    let src: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut i = 0;
    while i < src.len() {
        let line = src[i];
        let trimmed = line.trim_start();

        if let Some(fence) = trimmed.strip_prefix("```") {
            let lang = fence.trim_start_matches('`').trim();
            let mut top = Line::styled("╭─", s.muted);
            if !lang.is_empty() {
                top.push(Span::new(format!(" {lang}"), s.muted.italic()));
            }
            out.push(top);
            let gutter = Line::styled("│ ", s.muted);
            i += 1;
            while i < src.len() && !src[i].trim_start().starts_with("```") {
                let mut l = gutter.clone();
                l.push(Span::new(src[i].replace('\t', "    "), s.code));
                out.extend(wrap_hang(&l, w, &gutter));
                i += 1;
            }
            out.push(Line::styled("╰─", s.muted));
            i += 1; // the closing fence, if any
            continue;
        }

        if is_table_row(trimmed) {
            let start = i;
            while i < src.len() && is_table_row(src[i].trim_start()) {
                i += 1;
            }
            out.extend(table(&src[start..i], s, w));
            continue;
        }

        if trimmed.is_empty() {
            // Collapse runs of blank lines.
            if out.last().is_some_and(|l| !l.spans.is_empty()) {
                out.push(Line::default());
            }
            i += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix('#') {
            let level = 1 + rest.chars().take_while(|&c| c == '#').count();
            let title = rest.trim_start_matches('#').trim();
            let style = if level <= 2 { s.heading } else { s.base.bold() };
            let mut l = Line::default();
            inline(title, style, s, &mut l);
            out.extend(wrap_hang(&l, w, &Line::default()));
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            let bar = Line::styled("▎ ", s.muted);
            let mut l = bar.clone();
            inline(rest.trim_start(), s.quote, s, &mut l);
            out.extend(wrap_hang(&l, w, &bar));
        } else if is_rule(trimmed) {
            out.push(Line::styled("─".repeat(w), s.muted));
        } else if let Some((marker, rest)) = list_item(trimmed) {
            // Two columns per nesting level, whatever the source used.
            let depth = (line.len() - trimmed.len()).div_ceil(2);
            let pad = " ".repeat(depth * 2);
            let marker = if marker == "•" && depth > 0 {
                "◦"
            } else {
                marker
            };
            let mut l = Line::new(vec![
                Span::raw(pad.clone()),
                Span::new(format!("{marker} "), s.bullet),
            ]);
            inline(rest, s.base, s, &mut l);
            let hang = " ".repeat(pad.len() + crate::wrap::width(marker) + 1);
            out.extend(wrap_hang(&l, w, &Line::raw(hang)));
        } else {
            let mut l = Line::default();
            inline(line, s.base, s, &mut l);
            out.extend(wrap_hang(&l, w, &Line::default()));
        }
        i += 1;
    }
    while out.last().is_some_and(|l| l.spans.is_empty()) {
        out.pop();
    }
    out
}

fn is_rule(s: &str) -> bool {
    let c: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3
        && (c.chars().all(|x| x == '-') || c.chars().all(|x| x == '*') || c.chars().all(|x| x == '_'))
}

/// `- x`, `* x`, `+ x` or `12. x`: the marker to show and the text.
fn list_item(s: &str) -> Option<(&str, &str)> {
    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        return Some(("•", rest));
    }
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = s[digits..]
        .strip_prefix(". ")
        .or_else(|| s[digits..].strip_prefix(") "))?;
    Some((&s[..digits + 1], rest))
}

fn is_table_row(s: &str) -> bool {
    s.starts_with('|') && s.len() > 1 && s[1..].contains('|')
}

fn cells(row: &str) -> Vec<&str> {
    let row = row.trim();
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    row.split('|').map(str::trim).collect()
}

fn is_separator(row: &[&str]) -> bool {
    row.iter()
        .all(|c| !c.is_empty() && c.chars().all(|x| matches!(x, '-' | ':' | ' ')))
}

/// An aligned grid when it fits, else one wrapped line per row.
fn table(rows: &[&str], s: &MdStyle, w: usize) -> Vec<Line> {
    let parsed: Vec<Vec<&str>> = rows.iter().map(|r| cells(r)).collect();
    let has_header = parsed.get(1).is_some_and(|r| is_separator(r));
    let body: Vec<(bool, Vec<Line>)> = parsed
        .iter()
        .enumerate()
        .filter(|(_, r)| !is_separator(r))
        .map(|(ri, r)| {
            let header = has_header && ri == 0;
            let base = if header { s.base.bold() } else { s.base };
            let cells = r
                .iter()
                .map(|c| {
                    let mut l = Line::default();
                    inline(c, base, s, &mut l);
                    l
                })
                .collect();
            (header, cells)
        })
        .collect();
    let ncols = body.iter().map(|(_, r)| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; ncols];
    for (_, r) in &body {
        for (c, cell) in r.iter().enumerate() {
            widths[c] = widths[c].max(cell.width());
        }
    }
    let sep = Span::new(" │ ", s.muted);
    let total: usize = widths.iter().sum::<usize>() + 3 * ncols.saturating_sub(1);
    let mut out = Vec::new();
    if total <= w {
        for (header, r) in &body {
            let mut l = Line::default();
            for (c, width) in widths.iter().enumerate() {
                if c > 0 {
                    l.push(sep.clone());
                }
                let cell = r.get(c).cloned().unwrap_or_default();
                let pad = width - cell.width();
                l.spans.extend(cell.spans);
                if c + 1 < ncols {
                    l.push(Span::raw(" ".repeat(pad)));
                }
            }
            out.push(l);
            if *header {
                let rule: Vec<String> = widths.iter().map(|n| "─".repeat(*n)).collect();
                out.push(Line::styled(rule.join("─┼─"), s.muted));
            }
        }
        return out;
    }
    // Too wide: "header: value · header: value" per row.
    let heads: Vec<String> = if has_header {
        body.first()
            .map(|(_, r)| r.iter().map(Line::text).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for (_, r) in body.iter().skip(usize::from(has_header)) {
        let mut l = Line::new(vec![Span::new("• ", s.bullet)]);
        for (c, cell) in r.iter().enumerate() {
            if c > 0 {
                l.push(Span::new(" · ", s.muted));
            }
            if let Some(h) = heads.get(c).filter(|h| !h.is_empty()) {
                l.push(Span::new(format!("{h}: "), s.muted));
            }
            l.spans.extend(cell.spans.clone());
        }
        out.extend(wrap_hang(&l, w, &Line::raw("  ")));
    }
    out
}

/// Inline spans: `code`, **bold**, *italic* / _italic_, [links](url).
fn inline(text: &str, base: Style, s: &MdStyle, out: &mut Line) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut buf = String::new();
    let mut style = base;
    let mut bold = false;
    let mut italic = false;
    let flush = |buf: &mut String, style: Style, out: &mut Line| {
        if !buf.is_empty() {
            out.push(Span::new(std::mem::take(buf), style));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(end) = chars[i + 1..].iter().position(|&x| x == '`')
        {
            flush(&mut buf, style, out);
            let code: String = chars[i + 1..i + 1 + end].iter().collect();
            out.push(Span::new(code, s.code));
            i += end + 2;
            continue;
        }
        if c == '['
            && let Some((label, url, len)) = link(&chars[i..])
        {
            flush(&mut buf, style, out);
            out.push(Span::new(label.clone(), s.link));
            if url != label && !url.starts_with('#') {
                out.push(Span::new(format!(" ({url})"), s.muted));
            }
            i += len;
            continue;
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            flush(&mut buf, style, out);
            bold = !bold;
            style = if bold { style.bold() } else { base };
            if italic {
                style = style.italic();
            }
            i += 2;
            continue;
        }
        let boundary_before = i == 0 || !chars[i - 1].is_alphanumeric();
        let boundary_after = chars.get(i + 1).is_none_or(|n| !n.is_whitespace());
        if (c == '*' || c == '_') && (italic || (boundary_before && boundary_after)) {
            flush(&mut buf, style, out);
            italic = !italic;
            style = base;
            if bold {
                style = style.bold();
            }
            if italic {
                style = style.italic();
            }
            i += 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, style, out);
}

/// `[label](url)` at the start of `chars`: label, url, chars consumed.
fn link(chars: &[char]) -> Option<(String, String, usize)> {
    let close = chars.iter().position(|&c| c == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = chars[close + 2..].iter().position(|&c| c == ')')? + close + 2;
    let label: String = chars[1..close].iter().collect();
    let url: String = chars[close + 2..end].iter().collect();
    if label.is_empty() || url.contains(' ') {
        return None;
    }
    Some((label, url, end + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    #[test]
    fn structures_and_inline() {
        let s = MdStyle::default();
        let lines = render("# Title\n- a `b` **c**\n```rs\nlet x;\n```\nplain *it*", &s, 40);
        assert_eq!(
            texts(&lines),
            vec!["Title", "• a b c", "╭─ rs", "│ let x;", "╰─", "plain it"]
        );
        assert!(lines[0].spans[0].style.bold);
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|sp| sp.text == "b" && sp.style.fg == Color::Cyan)
        );
        assert!(lines[1].spans.iter().any(|sp| sp.text == "c" && sp.style.bold));
        assert!(lines[5].spans.last().is_some_and(|sp| sp.style.italic));
    }

    #[test]
    fn stray_markers_are_text() {
        let s = MdStyle::default();
        let lines = render("2 * 3 = 6 and a_b_c", &s, 40);
        assert_eq!(lines[0].text(), "2 * 3 = 6 and a_b_c");
    }

    #[test]
    fn paragraphs_wrap_flush_and_lists_hang() {
        let s = MdStyle::default();
        let lines = render(
            "one two three four five\n\n- alpha beta gamma delta\n  - nested item here",
            &s,
            14,
        );
        assert_eq!(
            texts(&lines),
            vec![
                "one two three",
                "four five",
                "",
                "• alpha beta",
                "  gamma delta",
                "  ◦ nested",
                "    item here",
            ]
        );
        let code = render("```\nabcdefghijklmnop\n```", &s, 10);
        assert_eq!(texts(&code), vec!["╭─", "│ abcdefgh", "│ ijklmnop", "╰─"]);
    }

    #[test]
    fn tables_align_or_fall_back() {
        let s = MdStyle::default();
        let md = "| key | does |\n|---|---|\n| `b` | toggle |\n| q | quit it |";
        let lines = render(md, &s, 40);
        assert_eq!(
            texts(&lines),
            vec!["key │ does", "────┼────────", "b   │ toggle", "q   │ quit it"]
        );
        assert!(lines[0].spans[0].style.bold);
        let narrow = render(md, &s, 10);
        assert!(narrow[0].text().starts_with("• key: b"), "{:?}", narrow[0].text());
    }

    #[test]
    fn links_show_their_target() {
        let s = MdStyle::default();
        let lines = render("see [docs](https://x.y) and [§](#a)", &s, 60);
        assert_eq!(lines[0].text(), "see docs (https://x.y) and §");
        assert!(lines[0].spans[1].style.underline);
    }
}
