//! Markdown-lite: enough structure for agent messages (headings, fenced
//! code, inline code, bullets, bold, italics) in a hundred-odd lines.
//! Everything else is passed through as text.

use crate::style::{Color, Line, Span, Style};

#[derive(Debug, Clone, Copy)]
pub struct MdStyle {
    pub base: Style,
    pub code: Style,
    pub heading: Style,
    pub quote: Style,
    pub bullet: Style,
}

impl Default for MdStyle {
    fn default() -> Self {
        Self {
            base: Style::new(),
            code: Style::new().fg(Color::Cyan),
            heading: Style::new().bold().fg(Color::Magenta),
            quote: Style::new().dim().italic(),
            bullet: Style::new().fg(Color::Magenta),
        }
    }
}

/// Renders markdown to unwrapped lines (one per source line; fenced code
/// lines are kept verbatim).
pub fn render(text: &str, s: &MdStyle) -> Vec<Line> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            let lang = line.trim_start().trim_start_matches('`').trim();
            let label = if in_fence && !lang.is_empty() {
                format!("─ {lang} ")
            } else {
                "─".to_owned()
            };
            out.push(Line::styled(label, s.code.dim()));
            continue;
        }
        if in_fence {
            out.push(Line::styled(format!("  {line}"), s.code));
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let level = 1 + rest.chars().take_while(|&c| c == '#').count();
            let title = rest.trim_start_matches('#').trim();
            let mut l = Line::styled(title, s.heading);
            if level > 2 {
                l = Line::styled(title, s.heading.fg(s.base.fg));
            }
            out.push(l);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            let mut l = Line::styled("│ ", s.quote);
            inline(rest, s.quote, s, &mut l);
            out.push(l);
            continue;
        }
        let indent = line.len() - trimmed.len();
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            let mut l = Line::styled(format!("{}• ", " ".repeat(indent)), s.bullet);
            inline(rest, s.base, s, &mut l);
            out.push(l);
            continue;
        }
        if let Some((num, rest)) = ordered(trimmed) {
            let mut l = Line::styled(format!("{}{num}. ", " ".repeat(indent)), s.bullet);
            inline(rest, s.base, s, &mut l);
            out.push(l);
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            out.push(Line::styled("────", s.quote));
            continue;
        }
        let mut l = Line::default();
        inline(line, s.base, s, &mut l);
        out.push(l);
    }
    out
}

fn ordered(s: &str) -> Option<(&str, &str)> {
    let digits = s.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = s[digits..].strip_prefix(". ")?;
    Some((&s[..digits], rest))
}

/// Inline spans: `code`, **bold**, *italic* / _italic_.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_and_inline() {
        let s = MdStyle::default();
        let lines = render("# Title\n- a `b` **c**\n```rs\nlet x;\n```\nplain *it*", &s);
        assert_eq!(lines[0].text(), "Title");
        assert!(lines[0].spans[0].style.bold);
        assert_eq!(lines[1].text(), "• a b c");
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|sp| sp.text == "b" && sp.style.fg == Color::Cyan)
        );
        assert!(lines[1].spans.iter().any(|sp| sp.text == "c" && sp.style.bold));
        assert_eq!(lines[3].text(), "  let x;");
        assert_eq!(lines[5].text(), "plain it");
        assert!(lines[5].spans.last().is_some_and(|sp| sp.style.italic));
    }

    #[test]
    fn stray_markers_are_text() {
        let s = MdStyle::default();
        let lines = render("2 * 3 = 6 and a_b_c", &s);
        assert_eq!(lines[0].text(), "2 * 3 = 6 and a_b_c");
    }
}
