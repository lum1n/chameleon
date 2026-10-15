//! The usage overview: what sessions spent, per account and per local day,
//! as the harnesses reported it.
//!
//! ```text
//! ╭ usage ───────────────────────────────────────────────────────────────╮
//! │                                                                      │
//! │  all sessions                in 1.2M · out 84k · cache 9.1M  $41.20  │
//! │                                                                      │
//! │  accounts                                                            │
//! │  Claude Code  me@example.com · max       12 sessions        $38.90  │
//! │               5h ▰▰▰▱▱▱  48%   7d ▰▱▱▱▱▱  12%                         │
//! │  Codex        plus                         3 sessions         $2.30  │
//! │                                                                      │
//! │  days                                                                │
//! │  2026-10-15   in 410k · out 31k            $12.10   claude $11.80 · …  │
//! ╰──────────────────────────────────────────────── j/k scroll · esc ───╯
//! ```

use acc_core::usage::{Spend, roll_up};

use crate::model::App;
use crate::style::{Line, Span, Style};
use crate::view::overlay::{centered, fit_content};
use crate::view::status::{gauge, short, short_window};
use crate::view::{OverlayView, Rect};
use crate::wrap::{fit, truncate, width};

/// Days shown, newest first.
const DAYS: usize = 30;

fn tokens(s: &Spend) -> String {
    let mut parts = vec![
        format!("in {}", short(s.input_tokens)),
        format!("out {}", short(s.output_tokens)),
    ];
    let cache = s.cache_read_tokens + s.cache_write_tokens;
    if cache > 0 {
        parts.push(format!("cache {}", short(cache)));
    }
    parts.join(" · ")
}

fn cost(s: &Spend) -> String {
    if s.cost_usd > 0.0 {
        format!("${:.2}", s.cost_usd)
    } else {
        String::new()
    }
}

/// A cost in a column of its own, right-aligned.
fn cost_col(s: &Spend) -> String {
    format!("{:>9}", cost(s))
}

/// `left`, then `right` flush right in `w` columns.
fn spread(mut left: Line, right: Vec<Span>, w: usize) -> Line {
    let rw: usize = right.iter().map(|s| width(&s.text)).sum();
    let pad = w.saturating_sub(left.width() + rw).max(1);
    left.push(Span::raw(" ".repeat(pad)));
    left.spans.extend(right);
    left
}

pub fn build(app: &App, scroll: usize, area: Rect) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let head = Style::new().fg(theme.accent).bold();
    let rect = centered(area, 84, area.h.saturating_sub(2).max(8));
    let inner = rect.inner();
    let w = (inner.w as usize).saturating_sub(4);
    let pad = |mut l: Line| {
        l.spans.insert(0, Span::raw("  "));
        l
    };
    let records = app.usage_records();
    let r = roll_up(&records, app.utc_offset_min);
    let mut lines = vec![Line::default()];
    if records.is_empty() {
        lines.push(pad(Line::styled(
            "nothing yet: usage appears as sessions report it",
            muted,
        )));
    } else {
        lines.push(pad(spread(
            Line::styled("all sessions", Style::new().bold()),
            vec![
                Span::new(tokens(&r.total), muted),
                Span::raw("  "),
                Span::new(cost_col(&r.total), Style::new().bold()),
            ],
            w,
        )));
        lines.push(Line::default());
        lines.push(pad(Line::styled("accounts", head)));
        for a in &r.accounts {
            let who: Vec<&str> = [a.account.as_deref(), a.plan.as_deref()]
                .into_iter()
                .flatten()
                .collect();
            let sessions = format!("{} session{}", a.sessions, if a.sessions == 1 { "" } else { "s" });
            let left = Line::new(vec![
                Span::new(fit(a.harness.display_name(), 13), Style::new().bold()),
                Span::new(truncate(&who.join(" · "), w.saturating_sub(40)), muted),
            ]);
            lines.push(pad(spread(
                left,
                vec![
                    Span::new(sessions, muted),
                    Span::raw("  "),
                    Span::new(cost_col(&a.spend), Style::new()),
                ],
                w,
            )));
            let mut gauges = Line::raw(" ".repeat(13));
            for win in a.quota.iter().flat_map(|q| &q.windows) {
                if let Some(p) = win.used_percent {
                    let label = short_window(&win.id, win.window_minutes);
                    gauges
                        .spans
                        .extend(gauge(&label, p.round().clamp(0.0, 100.0) as u64, 6, app));
                    gauges.push(Span::raw("   "));
                }
            }
            if gauges.spans.len() > 1 {
                lines.push(pad(gauges));
            }
        }
        lines.push(Line::default());
        lines.push(pad(Line::styled("days", head)));
        for d in r.days.iter().rev().take(DAYS) {
            let per: Vec<String> = d
                .harnesses
                .iter()
                .filter(|h| h.spend.cost_usd > 0.0)
                .map(|h| format!("{} {}", h.harness.program(), cost(&h.spend)))
                .collect();
            let mut l = Line::new(vec![
                Span::new(format!("{}   ", d.date), Style::new()),
                Span::new(fit(&tokens(&d.spend), 34), muted),
                Span::new(cost_col(&d.spend), Style::new().bold()),
            ]);
            let room = w.saturating_sub(l.width() + 2);
            if room > 6 && !per.is_empty() {
                l.push(Span::new(
                    format!("  {}", truncate(&per.join(" · "), room)),
                    muted,
                ));
            }
            lines.push(pad(l));
        }
    }
    lines.push(Line::default());
    lines.push(pad(Line::styled(
        "as the harnesses reported it, for the sessions this client has seen",
        muted.italic(),
    )));
    let room = (inner.h as usize).saturating_sub(1);
    let scroll = scroll.min(lines.len().saturating_sub(room));
    let lines: Vec<Line> = lines.into_iter().skip(scroll).take(room).collect();
    let (rect, lines, _) = fit_content(rect, lines, None);
    OverlayView {
        rect,
        title: "usage".into(),
        lines,
        footer: Some(Line::styled("j/k scroll · esc close", muted)),
        cursor: None,
        tone: None,
    }
}
