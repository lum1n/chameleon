//! Rendering of the modal overlays.

use acc_core::state::BlockKind;

use crate::keymap::Action;
use crate::model::App;
use crate::overlay::{ConfirmAction, Overlay, Wizard, file_changes, option_label};
use crate::style::{Line, Span, Style};
use crate::view::transcript::{file_head, hunks};
use crate::view::{OverlayView, Rect};
use crate::wrap::{fit, truncate, wrap_line};

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.w);
    let h = h.min(area.h);
    Rect::new(area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h)
}

pub fn build(app: &App, area: Rect) -> Option<OverlayView> {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let accent = Style::new().fg(theme.accent);
    let overlay = app.overlay.as_ref()?;
    Some(match overlay {
        Overlay::Help { scroll } => {
            let rect = centered(area, 70, area.h.saturating_sub(2).max(5));
            let inner = rect.inner();
            let mut lines = Vec::new();
            lines.push(Line::styled("keys", accent.bold()));
            for a in Action::ALL {
                let keys: Vec<String> = app.keymap.keys_for(a).iter().map(ToString::to_string).collect();
                lines.push(Line::new(vec![
                    Span::new(fit(&keys.join(" "), 18), accent),
                    Span::raw(a.describe()),
                ]));
            }
            lines.push(Line::default());
            lines.push(Line::styled("composer", accent.bold()));
            for (k, d) in [
                ("enter", "send the prompt"),
                ("alt-enter", "new line"),
                ("ctrl-enter", "send (also inside the launch wizard)"),
                ("/", "command palette (on an empty prompt)"),
                ("up / down", "prompt history"),
                ("ctrl-w ctrl-u ctrl-k", "delete word / to start / to end"),
            ] {
                lines.push(Line::new(vec![Span::new(fit(k, 18), accent), Span::raw(d)]));
            }
            if !app.cfg.hooks.is_empty() {
                lines.push(Line::default());
                lines.push(Line::styled("hooks", accent.bold()));
                for h in &app.cfg.hooks {
                    let when = if let Some(k) = &h.key {
                        k.clone()
                    } else {
                        h.on.join(",")
                    };
                    lines.push(Line::new(vec![
                        Span::new(fit(&when, 18), accent),
                        Span::raw(h.name.clone()),
                    ]));
                }
            }
            if !app.problems.is_empty() {
                lines.push(Line::default());
                lines.push(Line::styled(
                    "config problems",
                    Style::new().fg(theme.error).bold(),
                ));
                for p in &app.problems {
                    lines.push(Line::styled(p.clone(), Style::new().fg(theme.error)));
                }
            }
            lines.push(Line::default());
            lines.push(Line::styled(
                format!(
                    "chameleon {} · acc-core {}",
                    env!("CARGO_PKG_VERSION"),
                    acc_core::VERSION
                ),
                muted,
            ));
            let lines = lines.into_iter().skip(*scroll).take(inner.h as usize).collect();
            OverlayView {
                rect,
                title: "help".into(),
                lines,
                footer: Some(Line::styled("j/k scroll · esc close", muted)),
                cursor: None,
            }
        }
        Overlay::Confirm { action, title, .. } => {
            let rect = centered(area, 50, 5);
            let what = match action {
                ConfirmAction::Kill => "Stop the harness of",
                ConfirmAction::Remove => "Stop and delete",
            };
            OverlayView {
                rect,
                title: "confirm".into(),
                lines: vec![Line::raw(format!("{what} \"{}\"?", truncate(title, 30)))],
                footer: Some(Line::styled("y / enter: yes · anything else: no", muted)),
                cursor: None,
            }
        }
        Overlay::Permission {
            key,
            request_id,
            selected,
            text,
        } => {
            let rect = centered(area, 72, area.h.saturating_sub(4).max(8));
            let inner = rect.inner();
            let w = inner.w as usize;
            let slot = app.slots.iter().find(|s| &s.key == key);
            let perm = slot
                .and_then(|s| s.session.as_ref())
                .and_then(|s| s.permissions().iter().find(|p| &p.request_id == request_id));
            let mut lines = Vec::new();
            let Some(p) = perm else {
                return Some(OverlayView {
                    rect,
                    title: "request".into(),
                    lines: vec![Line::styled("already answered", muted)],
                    footer: Some(Line::styled("esc", muted)),
                    cursor: None,
                });
            };
            let question = p.kind == BlockKind::Question;
            lines.extend(wrap_line(
                &Line::styled(p.title.clone(), Style::new().bold()),
                w,
                "  ",
            ));
            // Preview of what is being asked for.
            let mut preview: Vec<Line> = Vec::new();
            if let (Some(s), Some(id)) = (slot.and_then(|s| s.session.as_ref()), &p.item_id)
                && let Some(item) = s.transcript().get(id)
            {
                let d = item.detail(s.spec().harness);
                match &d {
                    acc_core::item::ItemDetail::Command { command, cwd, .. } => {
                        preview.push(Line::new(vec![
                            Span::new("$ ", Style::new().fg(theme.tool).bold()),
                            Span::new(command.clone(), Style::new().fg(theme.tool)),
                        ]));
                        if let Some(c) = cwd {
                            preview.push(Line::styled(format!("in {c}"), muted));
                        }
                    }
                    acc_core::item::ItemDetail::FileChange { files } => {
                        for f in files {
                            preview.push(file_head(f, &theme));
                            preview.extend(hunks(f, &theme));
                        }
                    }
                    acc_core::item::ItemDetail::Tool {
                        name, title, input, ..
                    } => {
                        preview.push(Line::new(vec![
                            Span::new(format!("⚙ {name} "), Style::new().fg(theme.tool).bold()),
                            Span::new(title.clone(), Style::new().fg(theme.tool)),
                        ]));
                        preview.extend(json_lines(input, muted));
                    }
                    acc_core::item::ItemDetail::Text => {}
                }
            }
            if preview.is_empty() && !p.input.is_null() {
                preview.extend(json_lines(&p.input, muted));
            }
            if !preview.is_empty() {
                lines.push(Line::default());
                let room = (inner.h as usize).saturating_sub(lines.len() + p.options.len() + 5);
                let n = preview.len();
                for l in preview.iter().take(room) {
                    lines.extend(wrap_line(l, w, "    ").into_iter().take(1));
                }
                if n > room {
                    lines.push(Line::styled(format!("… {} more lines", n - room), muted));
                }
            }
            lines.push(Line::default());
            for (i, o) in p.options.iter().enumerate() {
                let style = if i == *selected && text.is_empty() {
                    Style::new().fg(theme.accent).bold().reversed()
                } else {
                    Style::new()
                };
                lines.push(Line::new(vec![
                    Span::new(format!(" {} ", i + 1), muted),
                    Span::new(format!(" {} ", o.label), style),
                ]));
            }
            let mut cursor = None;
            if question {
                lines.push(Line::default());
                let prompt = "answer: ";
                let shown = truncate(text, w.saturating_sub(prompt.len() + 1));
                lines.push(Line::new(vec![
                    Span::new(prompt, accent),
                    Span::raw(shown.clone()),
                ]));
                cursor = Some((
                    inner.x + (prompt.len() + crate::wrap::width(&shown)) as u16,
                    inner.y + (lines.len() as u16).saturating_sub(1),
                ));
            }
            let footer = if question {
                "enter: answer · 1-9: pick · esc: later"
            } else {
                "y / 1: allow · n: deny · 1-9: pick · enter: selected · esc: later"
            };
            let lines = lines.into_iter().take(inner.h as usize).collect();
            OverlayView {
                rect,
                title: if question {
                    "question".into()
                } else {
                    "approval".into()
                },
                lines,
                footer: Some(Line::styled(footer, muted)),
                cursor,
            }
        }
        Overlay::Launch(w) => launch_view(app, w, area),
        Overlay::Palette { query, selected } => {
            let rect = centered(area, 72, area.h.saturating_sub(4).clamp(6, 24));
            let inner = rect.inner();
            let w = inner.w as usize;
            let matches = app.palette_matches(query);
            let mut lines = vec![Line::new(vec![
                Span::new("› ", accent.bold()),
                Span::raw(query.clone()),
            ])];
            let cursor = Some((inner.x + 2 + crate::wrap::width(query) as u16, inner.y));
            lines.push(Line::default());
            let room = (inner.h as usize).saturating_sub(2);
            let start = selected.saturating_sub(room.saturating_sub(1));
            for (i, it) in matches.iter().enumerate().skip(start).take(room) {
                let style = if i == *selected {
                    Style::new().reversed()
                } else {
                    Style::new()
                };
                let label_w = 28.min(w / 2);
                lines.push(Line::new(vec![
                    Span::new(fit(&it.label, label_w), style.bold()),
                    Span::new(" ", style),
                    Span::new(
                        fit(
                            &truncate(&it.hint, w.saturating_sub(label_w + 1)),
                            w.saturating_sub(label_w + 1),
                        ),
                        style.fg(theme.muted),
                    ),
                ]));
            }
            if matches.is_empty() {
                lines.push(Line::styled("no matches", muted));
            }
            OverlayView {
                rect,
                title: "commands".into(),
                lines,
                footer: Some(Line::styled(
                    "type to filter · enter: run · words after the name are arguments",
                    muted,
                )),
                cursor,
            }
        }
        Overlay::Options { key, row, pending } => {
            let rect = centered(area, 64, 12);
            let defs = app
                .slots
                .iter()
                .find(|s| &s.key == key)
                .and_then(|s| s.session.as_ref())
                .map(|s| s.options().to_vec())
                .unwrap_or_default();
            let mut lines = Vec::new();
            for (i, d) in defs.iter().enumerate() {
                let cur = d
                    .current
                    .as_ref()
                    .and_then(|c| d.values.iter().position(|v| v == c))
                    .unwrap_or(0);
                let p = pending.get(i).copied().unwrap_or(cur);
                let value = d.values.get(p).cloned().unwrap_or_default();
                let changed = p != cur;
                let style = if i == *row {
                    Style::new().reversed()
                } else {
                    Style::new()
                };
                let apply = match d.apply {
                    acc_core::command::Apply::Live => "",
                    acc_core::command::Apply::NextTurn => " (next turn)",
                    acc_core::command::Apply::Restart => " (restarts)",
                };
                lines.push(Line::new(vec![
                    Span::new(fit(option_label(d.id), 12), style.bold()),
                    Span::new(
                        format!(" ◂ {value} ▸ "),
                        if changed {
                            style.fg(theme.accent).bold()
                        } else {
                            style
                        },
                    ),
                    Span::new(apply, style.fg(theme.muted)),
                ]));
                lines.push(Line::styled(
                    format!(
                        "             {}",
                        truncate(&d.values.join(" · "), rect.w.saturating_sub(16) as usize)
                    ),
                    muted,
                ));
            }
            OverlayView {
                rect,
                title: "options".into(),
                lines,
                footer: Some(Line::styled("←/→ change · enter: apply · esc: cancel", muted)),
                cursor: None,
            }
        }
        Overlay::Diff { key, item, scroll } => {
            let rect = centered(area, area.w.saturating_sub(4), area.h.saturating_sub(2));
            let inner = rect.inner();
            let mut lines = Vec::new();
            if let Some(slot) = app.slots.iter().find(|s| &s.key == key)
                && let Some(s) = &slot.session
                && let Some(it) = s.transcript().get(item)
                && let Some(files) = file_changes(&it.detail(s.spec().harness))
            {
                for f in files {
                    lines.push(file_head(f, &theme));
                    lines.extend(hunks(f, &theme));
                    lines.push(Line::default());
                }
            }
            let total = lines.len();
            let lines: Vec<Line> = lines
                .into_iter()
                .skip(*scroll)
                .take(inner.h as usize)
                .map(|l| {
                    wrap_line(&l, inner.w as usize, "    ")
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                })
                .collect();
            OverlayView {
                rect,
                title: "diff".into(),
                lines,
                footer: Some(Line::styled(
                    format!("j/k scroll · {} lines · esc close", total),
                    muted,
                )),
                cursor: None,
            }
        }
    })
}

fn json_lines(v: &serde_json::Value, style: Style) -> Vec<Line> {
    let text = match v {
        serde_json::Value::Object(m) => m
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                format!("{k}: {v}")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    };
    text.lines().map(|l| Line::styled(l.to_owned(), style)).collect()
}

fn launch_view(app: &App, w: &Wizard, area: Rect) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let accent = Style::new().fg(theme.accent);
    let rect = centered(area, 70, 16);
    let inner = rect.inner();
    let width = inner.w as usize;
    let mut lines = Vec::new();
    let mut cursor = None;
    let host = app
        .hosts
        .get(w.host)
        .map(|h| h.cfg.name.clone())
        .unwrap_or_default();
    let harnesses = app.wizard_harnesses(w.host);
    let probed = app.hosts.get(w.host).is_some_and(|h| h.probed);
    let label = |i: usize, name: &str| -> Span {
        if w.field == i {
            Span::new(fit(name, 12), accent.bold())
        } else {
            Span::new(fit(name, 12), muted)
        }
    };
    let mut hl = Line::new(vec![label(0, "host")]);
    for (i, h) in app.hosts.iter().enumerate() {
        let s = if i == w.host {
            Style::new().reversed()
        } else {
            Style::new()
        };
        hl.push(Span::new(format!(" {} ", h.cfg.name), s));
        hl.push(Span::raw(" "));
    }
    lines.push(hl);
    let mut hal = Line::new(vec![label(1, "harness")]);
    for h in &harnesses {
        let s = if *h == w.harness {
            Style::new().reversed()
        } else {
            Style::new()
        };
        hal.push(Span::new(format!(" {} ", h.display_name()), s));
        hal.push(Span::raw(" "));
    }
    lines.push(hal);
    if !probed {
        lines.push(Line::new(vec![
            Span::new(fit("", 12), muted),
            Span::new("probing the host for harnesses…", muted),
        ]));
    } else if harnesses.len() == Harness_ALL_LEN
        && app.hosts.get(w.host).is_some_and(|h| h.harnesses.is_empty())
    {
        lines.push(Line::new(vec![
            Span::new(fit("", 12), muted),
            Span::new(
                "no harness found on the host's PATH",
                Style::new().fg(theme.error),
            ),
        ]));
    } else {
        lines.push(Line::default());
    }
    let names = ["directory", "model", "permission", "first prompt"];
    let hints = [
        "where the agent works (~ allowed)",
        "blank: the harness's default",
        "claude: default|acceptEdits|plan|bypassPermissions · codex: profile",
        "optional; alt-enter for a new line",
    ];
    for (i, name) in names.iter().enumerate() {
        let field = i + 2;
        let value = &w.fields[i];
        let shown_w = width.saturating_sub(13);
        let shown = tail(value, shown_w);
        let mut l = Line::new(vec![label(field, name)]);
        if value.is_empty() && w.field != field {
            l.push(Span::new(hints[i], muted.italic()));
        } else {
            l.push(Span::new(shown.clone(), Style::new()));
        }
        if w.field == field {
            cursor = Some((
                inner.x + 12 + crate::wrap::width(&shown) as u16,
                inner.y + lines.len() as u16,
            ));
        }
        lines.push(l);
        if w.field == field && !value.is_empty() {
            lines.push(Line::new(vec![
                Span::new(fit("", 12), muted),
                Span::new(hints[i], muted.italic()),
            ]));
        }
    }
    lines.push(Line::default());
    if let Some(e) = &w.error {
        lines.push(Line::styled(e.clone(), Style::new().fg(theme.error)));
    } else if w.busy {
        lines.push(Line::styled("checking the directory…", muted));
    } else {
        lines.push(Line::styled(
            format!(
                "starts {} on {host}, detached; it keeps running if chameleon quits",
                w.harness.display_name()
            ),
            muted,
        ));
    }
    let footer = "tab/↓: next · ←/→: choose · enter on the prompt or ctrl-enter: start · esc: cancel";
    OverlayView {
        rect,
        title: "new session".into(),
        lines: lines.into_iter().take(inner.h as usize).collect(),
        footer: Some(Line::styled(footer, muted)),
        cursor,
    }
}

#[allow(non_upper_case_globals)]
const Harness_ALL_LEN: usize = acc_core::Harness::ALL.len();

/// The last `max` columns of a single-line value.
fn tail(s: &str, max: usize) -> String {
    let s = s.replace('\n', "⏎");
    if crate::wrap::width(&s) <= max {
        return s;
    }
    let mut out: Vec<char> = Vec::new();
    let mut w = 0;
    for c in s.chars().rev() {
        let cw = crate::wrap::width(c.encode_utf8(&mut [0u8; 4]));
        if w + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out.iter().rev().collect()
}
