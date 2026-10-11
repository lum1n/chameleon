//! The session list, grouped by host and sorted by urgency.

use crate::model::{App, FeedStatus, Pane};
use crate::slot::{Phase, Urgency};
use crate::style::{Color, Line, Span, Style};
use crate::view::{PaneView, Rect};
use crate::wrap::{fit, truncate, width};

pub fn build(app: &App, rect: Rect) -> PaneView {
    let inner = rect.inner();
    let w = inner.w as usize;
    let theme = app.theme;
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    let mut index = 0usize;
    let keys = app.sidebar_keys();
    let mut sel = None;
    for host in &app.hosts {
        let mut head = Line::new(vec![Span::new(
            truncate(&host.cfg.name, w.saturating_sub(4)),
            Style::new().bold().fg(theme.accent),
        )]);
        let n = keys.iter().filter(|k| k.host == host.cfg.name).count();
        let tail = match &host.feed {
            FeedStatus::Down(_) => " ⚡".to_owned(),
            _ if host.error.is_some() => " ✖".to_owned(),
            _ if host.listed_at_ms.is_none() => " …".to_owned(),
            _ => format!(" {n}"),
        };
        let pad = w.saturating_sub(width(&head.text()) + width(&tail));
        head.push(Span::new(" ".repeat(pad), Style::new()));
        head.push(Span::new(tail, Style::new().fg(theme.muted)));
        lines.push(head);
        hits.push(None);
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
            let color = match slot.urgency() {
                Urgency::Blocked => theme.permission,
                Urgency::Error => theme.error,
                Urgency::Working => {
                    if glyph == "⚙" {
                        theme.tool
                    } else {
                        theme.thinking
                    }
                }
                Urgency::Done => theme.done,
                Urgency::Idle => Color::Reset,
                Urgency::Exited | Urgency::Unknown => theme.muted,
            };
            let num = if index <= 9 {
                format!("{index}")
            } else if index == 10 {
                "0".into()
            } else {
                " ".into()
            };
            let mut line = Line::new(vec![
                Span::new(num, Style::new().fg(theme.muted)),
                Span::new(format!(" {glyph} "), Style::new().fg(color).bold()),
            ]);
            let harness = slot.harness().program();
            let right = if slot.attention {
                format!(" {harness} •")
            } else {
                format!(" {harness}")
            };
            let used = 4 + width(&right);
            let title_w = w.saturating_sub(used);
            let mut title_style = Style::new();
            if slot.attention {
                title_style = title_style.bold();
            }
            if matches!(slot.urgency(), Urgency::Exited) || slot.phase == Phase::Listed && !slot.is_alive() {
                title_style = title_style.fg(theme.muted);
            }
            line.push(Span::new(fit(&slot.title(), title_w), title_style));
            line.push(Span::new(
                right,
                Style::new().fg(if slot.attention { theme.accent } else { theme.muted }),
            ));
            if is_sel {
                for s in &mut line.spans {
                    s.style = s.style.reversed();
                }
            }
            lines.push(line);
            hits.push(Some(key.clone()));
        }
    }
    if keys.is_empty() {
        lines.push(Line::styled("no sessions", Style::new().fg(theme.muted)));
        lines.push(Line::styled("n: new  r: refresh", Style::new().fg(theme.muted)));
        hits.push(None);
        hits.push(None);
    }
    // Keep the selection in view.
    let h = inner.h as usize;
    let mut offset = 0;
    if let Some(s) = sel
        && s >= h
    {
        offset = s + 1 - h;
    }
    let lines: Vec<Line> = lines.into_iter().skip(offset).take(h).collect();
    let hits: Vec<_> = hits.into_iter().skip(offset).take(h).collect();
    PaneView {
        rect,
        title: Line::styled(" sessions ", Style::new().fg(theme.muted)),
        lines,
        focused: app.focus == Pane::Sidebar,
        cursor: None,
        scrollbar: None,
        hits,
    }
}
