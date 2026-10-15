//! The one-line status bar.

use acc_core::event::ItemKind;
use acc_core::transcript::ItemStatus;

use crate::model::{App, FeedStatus};
use crate::overlay::option_label;
use crate::slot::Urgency;
use crate::style::{Line, Span, Style};
use crate::view::spinner;
use crate::wrap::{truncate, width};

/// Cells in a meter, when the bar has room.
const METER_W: u64 = 6;
/// Width of the longest state word ("attaching", "replaying"), so the
/// rest of the bar doesn't move when the state changes.
const STATE_W: usize = 9;

pub fn build(app: &App, w: usize) -> Line {
    // Full meters where they fit, then short ones, then bare percentages.
    let mut line = Line::default();
    for cells in [METER_W, 4, 0] {
        line = bar(app, w, cells);
        if line.width() <= w {
            break;
        }
    }
    line
}

/// The bar with meters `cells` wide; none at 0.
fn bar(app: &App, w: usize, cells: u64) -> Line {
    let t = app.theme;
    let muted = Style::new().fg(t.muted);
    let sep = Span::new("│ ", muted);
    let mut line = Line::default();
    let Some(slot) = app.selected() else {
        line.push(Span::new(" ◆ ", Style::new().fg(t.accent).bold()));
        line.push(Span::new("? help · n new · q quit", muted));
        return line;
    };
    let (glyph, word) = slot.badge();
    let (glyph, color) = match slot.urgency() {
        Urgency::Blocked => (glyph, t.permission),
        Urgency::Error => (glyph, t.error),
        Urgency::Working if glyph == "⚙" => (glyph, t.tool),
        Urgency::Working => (spinner(app.now_ms), t.thinking),
        Urgency::Done => (glyph, t.done),
        _ => (glyph, t.muted),
    };
    line.push(Span::new(
        format!(" {glyph} {word:<STATE_W$} "),
        Style::new().fg(color).bold(),
    ));
    line.push(Span::new(format!("{} ", slot.harness().display_name()), muted));
    if let Some(session) = &slot.session {
        let mut opts = Vec::new();
        for o in session.options() {
            if let Some(c) = &o.current {
                opts.push(format!("{} {}", option_label(o.id), c));
            }
        }
        if opts.is_empty() {
            if let Some(m) = &slot.spec.model {
                opts.push(format!("model {m}"));
            }
            if let Some(p) = &slot.spec.permission {
                opts.push(format!("permission {p}"));
            }
        }
        if !opts.is_empty() {
            line.push(sep.clone());
            line.push(Span::new(format!("{} ", opts.join(" · ")), Style::new()));
        }
        let u = session.usage();
        let mut usage: Vec<Span> = Vec::new();
        if let (Some(used), Some(window)) = (u.context_used, u.context_window)
            && window > 0
        {
            usage.extend(gauge("ctx", (used * 100 / window).min(100), cells, app));
        } else if let Some(used) = u.context_used {
            usage.push(Span::new("ctx ", muted));
            usage.push(Span::new(format!("{:<5}", short(used)), Style::new()));
        }
        if let Some(c) = u.cost_usd {
            if !usage.is_empty() {
                usage.push(Span::new(" · ", muted));
            }
            usage.push(Span::new(format!("{:<6}", format!("${c:.2}")), Style::new()));
        }
        if let (Some(i), Some(o)) = (u.input_tokens, u.output_tokens)
            && usage.is_empty()
        {
            usage.push(Span::new(
                format!("{:>5}↑ {:>5}↓", short(i), short(o)),
                Style::new(),
            ));
        }
        if !usage.is_empty() {
            line.push(sep.clone());
            line.spans.extend(usage);
            line.push(Span::raw(" "));
        }
        if let Some(q) = session.quota() {
            let mut parts: Vec<Span> = Vec::new();
            for win in &q.windows {
                if let Some(p) = win.used_percent {
                    if !parts.is_empty() {
                        parts.push(Span::raw("  "));
                    }
                    let label = short_window(&win.id, win.window_minutes);
                    parts.extend(gauge(&label, p.round().clamp(0.0, 100.0) as u64, cells, app));
                }
            }
            if !parts.is_empty() {
                line.push(sep.clone());
                line.spans.extend(parts);
                line.push(Span::raw(" "));
            }
        }
        let running = session.transcript().items().iter().rev().find(|i| {
            i.status == ItemStatus::Running
                && i.kind != ItemKind::AgentMessage
                && i.kind != ItemKind::Reasoning
        });
        if let Some(i) = running {
            let d = i.detail(session.spec().harness);
            let what = match d {
                acc_core::item::ItemDetail::Tool { name, title, .. } => format!("{name} {title}"),
                acc_core::item::ItemDetail::Command { command, .. } => command,
                acc_core::item::ItemDetail::FileChange { files } => {
                    files.first().map(|f| f.path.clone()).unwrap_or_default()
                }
                acc_core::item::ItemDetail::Subagent {
                    agent, description, ..
                } => {
                    format!("{} {description}", agent.as_deref().unwrap_or("subagent"))
                }
                acc_core::item::ItemDetail::Text => String::new(),
            };
            if !what.is_empty() {
                line.push(sep.clone());
                line.push(Span::new(
                    truncate(&what.replace('\n', " "), 40),
                    Style::new().fg(t.tool),
                ));
                line.push(Span::raw(" "));
            }
        }
    }
    if let Some(h) = app.host(&slot.key.host)
        && let FeedStatus::Down(e) = &h.feed
    {
        line.push(sep.clone());
        line.push(Span::new(
            format!("feed down: {} ", truncate(e, 30)),
            Style::new().fg(t.error),
        ));
    }
    if let Some(e) = &slot.error {
        line.push(sep);
        line.push(Span::new(truncate(e, 40), Style::new().fg(t.error)));
    }
    let right = "? help";
    let used = width(&line.text());
    if used + width(right) + 1 < w {
        line.push(Span::new(" ".repeat(w - used - width(right) - 1), Style::new()));
        line.push(Span::new(right, muted));
    }
    line
}

/// `ctx ▰▰▱▱▱▱  31%`: a label, a meter (the permission colour from 80%)
/// and the percentage; `ctx 31%` with no cells.
pub(crate) fn gauge(label: &str, pct: u64, cells: u64, app: &App) -> Vec<Span> {
    let t = app.theme;
    let hot = pct >= 80;
    let value = if hot {
        Style::new().fg(t.permission)
    } else {
        Style::new()
    };
    let mut out = vec![Span::new(format!("{label} "), Style::new().fg(t.muted))];
    if cells == 0 {
        out.push(Span::new(format!("{pct}%"), value));
        return out;
    }
    out.push(Span::new(
        meter(pct, cells),
        Style::new().fg(if hot { t.permission } else { t.accent }),
    ));
    out.push(Span::new(format!(" {pct:>3}%"), value));
    out
}

/// `▰▰▰▱▱▱` for a percentage, `cells` wide.
fn meter(pct: u64, cells: u64) -> String {
    let full = (pct * cells).div_ceil(100).min(cells) as usize;
    format!("{}{}", "▰".repeat(full), "▱".repeat(cells as usize - full))
}

pub(crate) fn short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

pub(crate) fn short_window(id: &str, minutes: Option<u64>) -> String {
    match minutes {
        Some(m) if m % 1440 == 0 && m >= 1440 => format!("{}d", m / 1440),
        Some(m) if m % 60 == 0 => format!("{}h", m / 60),
        Some(m) => format!("{m}m"),
        None => id.replace('_', " "),
    }
}
