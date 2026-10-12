//! The session list, grouped by host and sorted by urgency.
//!
//! ```text
//!  local ──────── 3
//! ▌1 ⚠ fix the te…  claude
//!      …/repos/sessh
//!  2 ⠋ bump deps     codex
//!      ~/acc
//! ```

use crate::model::{App, FeedStatus, Pane};
use crate::slot::{Phase, Urgency};
use crate::style::{Color, Line, Span, Style};
use crate::view::{PaneView, Rect, spinner};
use crate::wrap::{fit, truncate, truncate_path, width};

pub fn build(app: &App, rect: Rect) -> PaneView {
    let w = rect.w as usize;
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let focused = app.focus == Pane::Sidebar;
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    let mut index = 0usize;
    let keys = app.sidebar_keys();
    let mut sel = None;
    for (hi, host) in app.hosts.iter().enumerate() {
        if hi > 0 {
            lines.push(Line::default());
            hits.push(None);
        }
        // "name ──── n": the host as a section rule.
        let n = keys.iter().filter(|k| k.host == host.cfg.name).count();
        let tail = match &host.feed {
            FeedStatus::Down(_) => Span::new("⚡", Style::new().fg(theme.error)),
            _ if host.error.is_some() => Span::new("✖", Style::new().fg(theme.error)),
            _ if host.listed_at_ms.is_none() => Span::new("…", muted),
            _ => Span::new(n.to_string(), muted),
        };
        let name = truncate(&host.cfg.name, w.saturating_sub(5 + width(&tail.text)));
        let fill = w.saturating_sub(width(&name) + width(&tail.text) + 3);
        let mut head = Line::new(vec![
            Span::raw(" "),
            Span::new(name, Style::new().bold().fg(theme.accent)),
            Span::new(format!(" {} ", "─".repeat(fill)), muted),
        ]);
        head.push(tail);
        lines.push(head);
        hits.push(None);
        if let Some(e) = &host.error {
            let last = e.lines().last().unwrap_or(e).trim();
            lines.push(Line::styled(
                format!("  ✖ {}", truncate(last, w.saturating_sub(4))),
                Style::new().fg(theme.error),
            ));
            hits.push(None);
        } else if let FeedStatus::Down(e) = &host.feed {
            lines.push(Line::styled(
                format!("  ⚡ {}", truncate(e, w.saturating_sub(4))),
                Style::new().fg(theme.error),
            ));
            hits.push(None);
        }
        for key in keys.iter().filter(|k| k.host == host.cfg.name) {
            let Some(slot) = app.slots.iter().find(|s| &s.key == key) else {
                continue;
            };
            index += 1;
            let is_sel = app.selected.as_ref() == Some(key);
            if is_sel {
                sel = Some(lines.len());
            }
            let (glyph, _) = slot.badge();
            let urgency = slot.urgency();
            let (glyph, color) = match urgency {
                Urgency::Blocked => (glyph, theme.permission),
                Urgency::Error => (glyph, theme.error),
                Urgency::Working if glyph == "⚙" => (glyph, theme.tool),
                Urgency::Working => (spinner(app.now_ms), theme.thinking),
                Urgency::Done => (glyph, theme.done),
                Urgency::Idle => (glyph, Color::Reset),
                Urgency::Exited | Urgency::Unknown => (glyph, theme.muted),
            };
            let num = if index <= 9 {
                format!("{index}")
            } else if index == 10 {
                "0".into()
            } else {
                " ".into()
            };
            let marker = if is_sel {
                Span::new("▌", Style::new().fg(theme.accent))
            } else {
                Span::raw(" ")
            };
            let mut line = Line::new(vec![
                marker,
                Span::new(num, muted),
                Span::new(format!(" {glyph} "), Style::new().fg(color).bold()),
            ]);
            let harness = slot.harness().program();
            let right = if slot.attention {
                format!(" {harness} •")
            } else {
                format!(" {harness}")
            };
            let used = 5 + width(&right);
            let title_w = w.saturating_sub(used);
            let dead = matches!(urgency, Urgency::Exited) || slot.phase == Phase::Listed && !slot.is_alive();
            let mut title_style = Style::new();
            if slot.attention || is_sel {
                title_style = title_style.bold();
            }
            if is_sel && focused {
                title_style = title_style.fg(theme.accent);
            } else if dead {
                title_style = title_style.fg(theme.muted);
            }
            line.push(Span::new(fit(&slot.title(), title_w), title_style));
            line.push(Span::new(
                right,
                Style::new().fg(if slot.attention { theme.accent } else { theme.muted }),
            ));
            lines.push(line);
            hits.push(Some(key.clone()));
            // Where it runs, under the title: the last directories count most.
            let cwd = slot.spec.cwd.trim_end_matches('/');
            if !cwd.is_empty() {
                lines.push(Line::styled(
                    format!("     {}", truncate_path(cwd, w.saturating_sub(6))),
                    muted,
                ));
                hits.push(Some(key.clone()));
            }
        }
    }
    if keys.is_empty() {
        lines.push(Line::default());
        lines.push(Line::styled(" no sessions yet", muted));
        lines.push(Line::default());
        lines.push(Line::new(vec![
            Span::new(" n", Style::new().fg(theme.accent)),
            Span::new("  start one", muted),
        ]));
        lines.push(Line::new(vec![
            Span::new(" r", Style::new().fg(theme.accent)),
            Span::new("  refresh", muted),
        ]));
        hits.extend(std::iter::repeat_n(None, 5));
    }
    // Keep the selection, and the path under it, in view.
    let h = rect.h as usize;
    let mut offset = 0;
    if let Some(s) = sel {
        let last = if hits.get(s + 1) == hits.get(s) { s + 1 } else { s };
        if last >= h {
            offset = last + 1 - h;
        }
    }
    let lines: Vec<Line> = lines.into_iter().skip(offset).take(h).collect();
    let hits: Vec<_> = hits.into_iter().skip(offset).take(h).collect();
    PaneView {
        rect,
        lines,
        focused,
        cursor: None,
        scrollbar: None,
        hits,
    }
}
