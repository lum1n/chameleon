//! The one-line header: the wordmark, the selected session and a tally of
//! every session's state, so the list can stay hidden.

use crate::model::App;
use crate::slot::Urgency;
use crate::style::{Line, Span, Style};
use crate::view::spinner;
use crate::wrap::{truncate, width};

pub fn build(app: &App, w: usize) -> Line {
    let t = app.theme;
    let muted = Style::new().fg(t.muted);
    let mut line = Line::new(vec![Span::new(" ◆ chameleon ", Style::new().fg(t.accent).bold())]);

    // Right: how many sessions are in each state, loudest first.
    let mut right = Line::default();
    let tally = |u: Urgency| app.slots.iter().filter(|s| s.urgency() == u).count();
    let groups = [
        (Urgency::Blocked, "⚠", t.permission),
        (Urgency::Error, "✖", t.error),
        (Urgency::Working, spinner(app.now_ms), t.thinking),
        (Urgency::Done, "✓", t.done),
        (Urgency::Idle, "·", t.muted),
    ];
    for (u, glyph, color) in groups {
        let n = tally(u);
        if n > 0 {
            right.push(Span::new(format!("{glyph}{n} "), Style::new().fg(color)));
        }
    }
    if !app.sidebar_shown {
        right.push(Span::new("ctrl-b sessions ", muted));
    }

    match app.selected() {
        Some(slot) => {
            let room = w
                .saturating_sub(width(&line.text()) + width(&right.text()) + 1)
                .max(8);
            let mut detail = format!("{} · {}", slot.spec.cwd, slot.harness().display_name());
            if let Some(m) = &slot.spec.model {
                detail.push_str(&format!(" · {m}"));
            }
            let title = truncate(&slot.title(), room.saturating_sub(4).min(40));
            line.push(Span::new("› ", muted));
            line.push(Span::new(title.clone(), Style::new().bold()));
            let left = room.saturating_sub(width(&title) + 4);
            if left > 4 {
                line.push(Span::new(format!("  {}", truncate(&detail, left)), muted));
            }
        }
        None => line.push(Span::new("no session · n: new", muted)),
    }
    let used = width(&line.text());
    let rw = width(&right.text());
    if used + rw < w {
        line.push(Span::new(" ".repeat(w - used - rw), Style::new()));
        line.spans.extend(right.spans);
    }
    line
}
