//! Rendering of the modal overlays.

use acc_core::state::BlockKind;

use crate::keymap::Action;
use crate::model::App;
use crate::overlay::{ConfirmAction, Overlay, Wizard, option_label, panel_actions, permission_modes, steps};
use crate::style::{Line, Span, Style};
use crate::view::spinner;
use crate::view::transcript::{option_tone, request_preview};
use crate::view::{OverlayView, Rect};
use crate::wrap::{fit, truncate, wrap_hang, wrap_line};

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
                tone: None,
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
                tone: None,
            }
        }
        Overlay::Permission {
            key,
            request_id,
            selected,
            text,
            answers,
            marked,
        } => permission_view(app, key, request_id, *selected, text, answers, marked, area),
        Overlay::Themes { selected, .. } => themes_view(app, *selected, area),
        Overlay::Agents { key, selected } => agents_view(app, key, *selected, area),
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
                tone: None,
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
                tone: None,
            }
        }
        Overlay::Pick {
            command,
            choices,
            current,
            selected,
            ..
        } => {
            let rect = centered(
                area,
                64,
                (choices.len() as u16 + 4).clamp(6, area.h.saturating_sub(4).max(6)),
            );
            let inner = rect.inner();
            let w = inner.w as usize;
            let room = inner.h as usize;
            let start = selected.saturating_sub(room.saturating_sub(1));
            let mut lines = Vec::new();
            for (i, c) in choices.iter().enumerate().skip(start).take(room) {
                let style = if i == *selected {
                    Style::new().reversed()
                } else {
                    Style::new()
                };
                let mark = if current.as_ref() == Some(&c.value) {
                    "● "
                } else {
                    "  "
                };
                let label_w = 28.min(w / 2);
                let hint = c.description.clone().unwrap_or_default();
                lines.push(Line::new(vec![
                    Span::new(mark, style.fg(theme.accent)),
                    Span::new(fit(&c.label, label_w), style.bold()),
                    Span::new(" ", style),
                    Span::new(
                        fit(
                            &truncate(&hint, w.saturating_sub(label_w + 3)),
                            w.saturating_sub(label_w + 3),
                        ),
                        style.fg(theme.muted),
                    ),
                ]));
            }
            OverlayView {
                rect,
                title: format!("/{command}"),
                lines,
                footer: Some(Line::styled("↑/↓ choose · enter: set · esc: cancel", muted)),
                cursor: None,
                tone: None,
            }
        }
        Overlay::Panel {
            key,
            id,
            selected,
            input,
        } => panel_view(app, key, id, *selected, input.as_deref(), area),
        Overlay::Diff(view) => crate::view::diff::build(app, view, area),
    })
}

fn panel_view(
    app: &App,
    key: &crate::slot::SessionKey,
    id: &str,
    selected: usize,
    input: Option<&str>,
    area: Rect,
) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let accent = Style::new().fg(theme.accent);
    let rect = centered(area, 80, area.h.saturating_sub(4).max(8));
    let inner = rect.inner();
    let w = inner.w as usize;
    let Some(panel) = app
        .slots
        .iter()
        .find(|s| &s.key == key)
        .and_then(|s| s.panels.get(id))
    else {
        return OverlayView {
            rect,
            title: format!("/{id}"),
            lines: vec![Line::styled("waiting for the harness…", muted)],
            footer: Some(Line::styled("esc: close", muted)),
            cursor: None,
            tone: None,
        };
    };
    let chosen = panel_actions(panel).get(selected).map(|a| (*a).clone());
    let mut lines: Vec<Line> = Vec::new();
    let mut focus_line = 0;
    let mut n = 0;
    // One row of buttons; numbering follows `panel_actions`.
    let mut buttons = |actions: &[acc_core::event::PanelAction], lines: &mut Vec<Line>, indent: &str| {
        if actions.is_empty() {
            return;
        }
        let mut l = Line::new(vec![Span::raw(indent.to_owned())]);
        for a in actions {
            let mut style = if a.destructive {
                Style::new().fg(theme.error)
            } else {
                accent
            };
            if n == selected {
                style = style.bold().reversed();
                focus_line = lines.len();
            }
            l.push(Span::new(format!("[{}]", a.label), style));
            l.push(Span::raw(" "));
            n += 1;
        }
        lines.push(l);
    };
    for (si, section) in panel.sections.iter().enumerate() {
        if si > 0 {
            lines.push(Line::default());
        }
        if let Some(t) = &section.title {
            lines.push(Line::styled(t.clone(), accent.bold()));
        }
        if section.rows.is_empty() && section.actions.is_empty() {
            lines.push(Line::styled("  (nothing)", muted));
        }
        for row in &section.rows {
            let value = row.value.clone().unwrap_or_default();
            let label_w = w.saturating_sub(crate::wrap::width(&value) + 3).max(8);
            lines.push(Line::new(vec![
                Span::raw("  "),
                Span::new(fit(&truncate(&row.label, label_w), label_w), Style::new()),
                Span::raw(" "),
                Span::new(value, Style::new().bold()),
            ]));
            if let Some(d) = row.detail.as_deref().filter(|d| !d.is_empty()) {
                lines.push(Line::styled(
                    format!("    {}", truncate(d, w.saturating_sub(4))),
                    muted,
                ));
            }
            buttons(&row.actions, &mut lines, "    ");
        }
        buttons(&section.actions, &mut lines, "  ");
    }
    let mut cursor = None;
    if let (Some(text), Some(a)) = (input, &chosen) {
        lines.push(Line::default());
        let prompt = format!("{}: ", a.prompt.clone().unwrap_or_else(|| a.label.clone()));
        let shown = truncate(text, w.saturating_sub(prompt.len() + 1));
        focus_line = lines.len();
        lines.push(Line::new(vec![
            Span::new(prompt.clone(), accent),
            Span::raw(shown.clone()),
        ]));
        cursor = Some((
            inner.x + crate::wrap::width(&prompt) as u16 + crate::wrap::width(&shown) as u16,
            0,
        ));
    }
    let room = inner.h as usize;
    let start = focus_line.saturating_sub(room.saturating_sub(1));
    if let Some((x, _)) = cursor {
        cursor = Some((x, inner.y + (focus_line - start) as u16));
    }
    let footer = if input.is_some() {
        "enter: run · esc: back".to_owned()
    } else if n == 0 {
        "esc: close".to_owned()
    } else {
        "↑/↓ choose · enter: run · esc: close".to_owned()
    };
    OverlayView {
        rect,
        title: if panel.title.is_empty() {
            format!("/{id}")
        } else {
            panel.title.clone()
        },
        lines: lines.into_iter().skip(start).take(room).collect(),
        footer: Some(Line::styled(footer, muted)),
        cursor,
        tone: None,
    }
}

/// Two columns of air inside an overlay's border.
const PAD: &str = "  ";

/// Shrinks `max` (vertically centred) to `lines` and a row of air, and
/// makes a content-relative cursor absolute.
fn fit_content(
    max: Rect,
    mut lines: Vec<Line>,
    cursor: Option<(u16, u16)>,
) -> (Rect, Vec<Line>, Option<(u16, u16)>) {
    lines.push(Line::default());
    let h = (lines.len() as u16 + 2).min(max.h);
    let rect = Rect::new(max.x, max.y + (max.h - h) / 2, max.w, h);
    let inner = rect.inner();
    lines.truncate(inner.h as usize);
    let cursor = cursor
        .filter(|(_, y)| *y < inner.h)
        .map(|(x, y)| (inner.x + x, inner.y + y));
    (rect, lines, cursor)
}

fn padded(mut l: Line) -> Line {
    l.spans.insert(0, Span::raw(PAD));
    l
}

/// The approval / question card.
///
/// ```text
/// ╭ approval ──────────────────────────────────────────╮
/// │                                                    │
/// │  Claude Code · fix the tests                       │
/// │  Bash wants to run a command                       │
/// │                                                    │
/// │  ▎ $ cargo test --workspace                        │
/// │                                                    │
/// │  ❯ 1  Allow                                        │
/// │    2  Always allow                                 │
/// │    3  Deny                                         │
/// ╰──────── y allow · n deny · 1-9 pick · esc later ───╯
/// ```
#[allow(clippy::too_many_arguments)]
fn permission_view(
    app: &App,
    key: &crate::slot::SessionKey,
    request_id: &str,
    selected: usize,
    text: &str,
    answers: &[Vec<String>],
    marked: &[usize],
    area: Rect,
) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let tone = theme.permission;
    // The most it may take; it shrinks to its content below.
    let rect = centered(area, 76, area.h.saturating_sub(4).max(10));
    let inner = rect.inner();
    let w = (inner.w as usize).saturating_sub(2 * PAD.len()).max(8);
    let slot = app.slots.iter().find(|s| &s.key == key);
    let session = slot.and_then(|s| s.session.as_ref());
    let Some((session, p)) = session.and_then(|s| {
        s.permissions()
            .iter()
            .find(|p| p.request_id == request_id)
            .map(|p| (s, p))
    }) else {
        return OverlayView {
            rect,
            title: "request".into(),
            lines: vec![Line::default(), padded(Line::styled("already answered", muted))],
            footer: Some(Line::styled("esc", muted)),
            cursor: None,
            tone: Some(tone),
        };
    };
    let question = p.kind == BlockKind::Question;
    let mut lines = vec![Line::default()];
    if let Some(s) = slot {
        lines.push(padded(Line::styled(
            truncate(&format!("{} · {}", s.harness().display_name(), s.title()), w),
            muted,
        )));
    }
    let steps = steps(p);
    if let Some(q) = steps.get(answers.len()) {
        return question_step(app, rect, lines, w, &steps, q, selected, text, answers, marked);
    }
    for l in wrap_line(&Line::styled(p.title.clone(), Style::new().bold()), w, "") {
        lines.push(padded(l));
    }
    // The preview gets what the options and the answer box leave.
    let reserved = p.options.len() + if question { 5 } else { 0 } + 3;
    let preview = request_preview(session, p, &theme, w.saturating_sub(2));
    if !preview.is_empty() {
        lines.push(Line::default());
        let room = (inner.h as usize).saturating_sub(lines.len() + reserved).max(1);
        let bar = Span::new("▎ ", Style::new().fg(tone));
        let n = preview.len();
        for mut l in preview.into_iter().take(if n > room { room - 1 } else { room }) {
            l.spans.insert(0, bar.clone());
            lines.push(padded(l));
        }
        if n > room {
            lines.push(padded(Line::styled(
                format!("… {} more lines", n - room + 1),
                muted,
            )));
        }
    }
    lines.push(Line::default());
    for (i, o) in p.options.iter().enumerate() {
        let ot = option_tone(&o.id, &o.label, &theme);
        let chosen = i == selected && text.is_empty();
        let label = if chosen {
            Style::new().fg(ot).bold()
        } else {
            Style::new()
        };
        lines.push(padded(Line::new(vec![
            Span::new(if chosen { "❯ " } else { "  " }, Style::new().fg(ot).bold()),
            Span::new(format!("{}  ", i + 1), muted),
            Span::new(o.label.clone(), label),
        ])));
    }
    let mut cursor = None;
    if question {
        lines.push(Line::default());
        lines.push(padded(Line::styled("or type an answer", muted)));
        let box_w = w.saturating_sub(4);
        let shown = tail(text, box_w);
        let edge = Style::new().fg(if text.is_empty() {
            theme.border
        } else {
            theme.accent
        });
        lines.push(padded(Line::styled(format!("╭{}╮", "─".repeat(box_w + 2)), edge)));
        cursor = Some((
            PAD.len() as u16 + 2 + crate::wrap::width(&shown) as u16,
            lines.len() as u16,
        ));
        lines.push(padded(Line::new(vec![
            Span::new("│ ", edge),
            Span::raw(fit(&shown, box_w)),
            Span::new(" │", edge),
        ])));
        lines.push(padded(Line::styled(format!("╰{}╯", "─".repeat(box_w + 2)), edge)));
    }
    let footer = if question {
        "enter answer · 1-9 pick · esc later"
    } else {
        "y allow · n deny · 1-9 pick · enter selected · esc later"
    };
    let (rect, lines, cursor) = fit_content(rect, lines, cursor);
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
        tone: Some(tone),
    }
}

/// One question of several, or one taking several picks.
///
/// ```text
/// ╭─ question ─────────────────────────────────────────╮
/// │  Claude Code · fix the tests                       │
/// │  DB · 1 of 2                                       │
/// │  Which database?                                   │
/// │                                                    │
/// │  ❯ 1 [x] Postgres  the big one                     │
/// │    2 [ ] SQLite                                    │
/// ╰─────── space tick · enter next · ⌫ back · esc later ╯
/// ```
#[allow(clippy::too_many_arguments)]
fn question_step(
    app: &App,
    rect: Rect,
    mut lines: Vec<Line>,
    w: usize,
    steps: &[acc_core::question::Question],
    q: &acc_core::question::Question,
    selected: usize,
    text: &str,
    answers: &[Vec<String>],
    marked: &[usize],
) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let tone = theme.permission;
    let i = answers.len();
    let mut head = format!("{} of {}", i + 1, steps.len());
    if let Some(h) = &q.header {
        head = format!("{h} · {head}");
    }
    if q.multi_select {
        head.push_str(" · pick any");
    }
    lines.push(padded(Line::styled(truncate(&head, w), Style::new().fg(tone))));
    for l in wrap_line(&Line::styled(q.question.clone(), Style::new().bold()), w, "") {
        lines.push(padded(l));
    }
    for (prev, a) in steps.iter().zip(answers) {
        let label = prev.header.as_deref().unwrap_or(&prev.question);
        lines.push(padded(Line::styled(
            truncate(&format!("✓ {label}: {}", a.join(", ")), w),
            muted,
        )));
    }
    lines.push(Line::default());
    for (n, o) in q.options.iter().enumerate() {
        let chosen = n == selected && text.is_empty();
        let mut l = Line::new(vec![
            Span::new(if chosen { "❯ " } else { "  " }, Style::new().fg(tone).bold()),
            Span::new(format!("{}  ", n + 1), muted),
        ]);
        if q.multi_select {
            let ticked = marked.contains(&n);
            l.push(Span::new(
                if ticked { "[x] " } else { "[ ] " },
                Style::new().fg(if ticked { theme.done } else { theme.muted }),
            ));
        }
        l.push(Span::new(
            o.label.clone(),
            if chosen {
                Style::new().fg(tone).bold()
            } else {
                Style::new()
            },
        ));
        if let Some(d) = &o.description {
            l.push(Span::new(format!("  {d}"), muted));
        }
        lines.extend(wrap_hang(&l, w, &Line::raw("       ")).into_iter().map(padded));
    }
    lines.push(Line::default());
    lines.push(padded(Line::styled("or type an answer", muted)));
    let box_w = w.saturating_sub(4);
    let shown = tail(text, box_w);
    let edge = Style::new().fg(if text.is_empty() {
        theme.border
    } else {
        theme.accent
    });
    lines.push(padded(Line::styled(format!("╭{}╮", "─".repeat(box_w + 2)), edge)));
    let cursor = Some((
        PAD.len() as u16 + 2 + crate::wrap::width(&shown) as u16,
        lines.len() as u16,
    ));
    lines.push(padded(Line::new(vec![
        Span::new("│ ", edge),
        Span::raw(fit(&shown, box_w)),
        Span::new(" │", edge),
    ])));
    lines.push(padded(Line::styled(format!("╰{}╯", "─".repeat(box_w + 2)), edge)));
    let next = if i + 1 == steps.len() { "send" } else { "next" };
    let mut footer = if q.multi_select {
        format!("space tick · enter {next}")
    } else {
        format!("1-9 pick · enter {next}")
    };
    if i > 0 {
        footer.push_str(" · ⌫ back");
    }
    footer.push_str(" · esc later");
    let (rect, lines, cursor) = fit_content(rect, lines, cursor);
    OverlayView {
        rect,
        title: "question".into(),
        lines,
        footer: Some(Line::styled(footer, muted)),
        cursor,
        tone: Some(tone),
    }
}

/// The theme picker: every theme with a strip of its colours.
/// ```text
/// ╭ agents ────────────────────────────────────────────────╮
/// │ ❯ main         fix the flaky test               42 items │
/// │   ⠋ Explore    find where the config is parsed  14 items │
/// │   ✔ Plan       design the loader                 6 items │
/// ╰─────────────────────────── j/k move · enter view · esc ─╯
/// ```
fn agents_view(app: &App, key: &crate::slot::SessionKey, selected: usize, area: Rect) -> OverlayView {
    use acc_core::transcript::ItemStatus;
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let rect = centered(area, 80, area.h.saturating_sub(4).clamp(6, 30));
    let inner = rect.inner();
    let w = inner.w as usize;
    let slot = app.slots.iter().find(|s| &s.key == key);
    let rows = slot
        .and_then(|s| s.session.as_ref())
        .map(crate::overlay::agent_rows)
        .unwrap_or_default();
    let shown = slot.and_then(|s| s.scope());
    let spin = spinner(app.now_ms);
    let name_w = rows
        .iter()
        .map(|r| crate::wrap::width(&r.name) + 2 * r.depth)
        .max()
        .unwrap_or(4)
        .min(24);
    let mut lines = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let chosen = i == selected;
        let (glyph, tone) = match r.status {
            None => ("◆", theme.accent),
            Some(ItemStatus::Running) => (spin, theme.thinking),
            Some(ItemStatus::Completed) => ("✔", theme.done),
            Some(ItemStatus::Failed) => ("✖", theme.error),
        };
        let name_style = if chosen {
            Style::new().fg(theme.accent).bold()
        } else if r.id.as_deref() == shown {
            Style::new().bold()
        } else {
            Style::new()
        };
        let indent = "  ".repeat(r.depth.saturating_sub(1));
        let count = format!("{} items", r.items);
        let mut l = Line::new(vec![
            Span::new(
                if chosen { "❯ " } else { "  " },
                Style::new().fg(theme.accent).bold(),
            ),
            Span::raw(indent.clone()),
            Span::new(format!("{glyph} "), Style::new().fg(tone)),
            Span::new(fit(&r.name, name_w.saturating_sub(indent.len()) + 1), name_style),
            Span::raw(" "),
        ]);
        let room = w.saturating_sub(l.width() + count.len() + 2);
        l.push(Span::new(fit(&truncate(r.description.trim(), room), room), muted));
        l.push(Span::raw("  "));
        l.push(Span::new(count, muted));
        lines.push(l);
    }
    let room = inner.h as usize;
    let start = selected.saturating_sub(room.saturating_sub(1));
    OverlayView {
        rect,
        title: "agents".into(),
        lines: lines.into_iter().skip(start).take(room).collect(),
        footer: Some(Line::styled("j/k move · enter view · esc close", muted)),
        cursor: None,
        tone: None,
    }
}

fn themes_view(app: &App, selected: usize, area: Rect) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let rect = centered(area, 60, area.h.saturating_sub(4).clamp(8, 32));
    let inner = rect.inner();
    let custom = app.cfg.custom_themes.len();
    let mut entries: Vec<(&str, String, crate::config::Theme)> =
        vec![("", "terminal colours".to_owned(), crate::config::Theme::default())];
    let choices = app.theme_choices();
    for (i, t) in choices.iter().enumerate() {
        let section = if i < custom {
            "custom"
        } else if t.is_light() {
            "light"
        } else {
            "dark"
        };
        entries.push((section, t.name.clone(), t.resolve()));
    }
    let mut lines = Vec::new();
    let mut focus = 0;
    let mut section = "";
    for (i, (sec, name, t)) in entries.iter().enumerate() {
        if *sec != section {
            section = sec;
            lines.push(Line::default());
            lines.push(padded(Line::styled(*sec, muted.bold())));
        }
        let chosen = i == selected;
        if chosen {
            focus = lines.len();
        }
        let mut l = padded(Line::new(vec![
            Span::new(
                if chosen { "❯ " } else { "  " },
                Style::new().fg(theme.accent).bold(),
            ),
            Span::new(
                fit(name, 26),
                if chosen {
                    Style::new().fg(theme.accent).bold()
                } else {
                    Style::new()
                },
            ),
        ]));
        for c in [
            t.bg,
            t.surface,
            t.accent,
            t.user,
            t.tool,
            t.done,
            t.permission,
            t.error,
        ] {
            l.push(if c == crate::style::Color::Reset {
                Span::new("··", muted)
            } else {
                Span::new("██", Style::new().fg(c))
            });
        }
        lines.push(l);
    }
    let room = inner.h as usize;
    let start = focus
        .saturating_sub(room / 2)
        .min(lines.len().saturating_sub(room));
    OverlayView {
        rect,
        title: "theme".into(),
        lines: lines.into_iter().skip(start).take(room).collect(),
        footer: Some(Line::styled("↑/↓ preview · enter keep · esc revert", muted)),
        cursor: None,
        tone: None,
    }
}

/// The launch wizard.
///
/// ```text
/// ╭ new session ───────────────────────────────────────────────╮
/// │                                                            │
/// │    host          local   box                               │
/// │    harness       Claude Code   Codex   pi   opencode       │
/// │                                                            │
/// │  ❯ directory     ~/repos/chameleon                         │
/// │    model         the harness's default                     │
/// │    permission    default                                   │
/// │                                                            │
/// │    first prompt                                            │
/// │    ┃ fix the failing tests                                 │
/// │                                                            │
/// │    ⏵ starts Claude Code on local, detached                 │
/// ╰──────────── tab next · ←/→ choose · ctrl-enter start · esc ╯
/// ```
fn launch_view(app: &App, w: &Wizard, area: Rect) -> OverlayView {
    const LABEL: usize = 14;
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let accent = Style::new().fg(theme.accent);
    // The most it may take; it shrinks to its content below.
    let rect = centered(area, 76, area.h.saturating_sub(2));
    let inner = rect.inner();
    let width = (inner.w as usize).saturating_sub(PAD.len()).max(LABEL + 8);
    let value_w = width.saturating_sub(2 + LABEL + 1);
    let mut lines = vec![Line::default()];
    let mut cursor = None;
    let host = app
        .hosts
        .get(w.host)
        .map(|h| h.cfg.name.clone())
        .unwrap_or_default();
    let harnesses = app.wizard_harnesses(w.host);
    let probed = app.hosts.get(w.host).is_some_and(|h| h.probed);
    let models = app.wizard_models(w.host, w.harness);

    let label = |field: usize, name: &str| -> Vec<Span> {
        if w.field == field {
            vec![
                Span::new("❯ ", accent.bold()),
                Span::new(fit(name, LABEL), accent.bold()),
            ]
        } else {
            vec![Span::raw("  "), Span::new(fit(name, LABEL), muted)]
        }
    };
    // A choice; the chosen one is a pill while its row has focus.
    let chip = |text: &str, chosen: bool, focused: bool| -> Span {
        let text = format!(" {} ", text.replace(' ', "\u{a0}"));
        let style = match (chosen, focused) {
            (true, true) => accent.bold().reversed(),
            (true, false) => accent.bold(),
            _ => muted,
        };
        Span::new(text, style)
    };
    let hang = Line::raw(" ".repeat(PAD.len() + 2 + LABEL));
    let choices = |field: usize, name: &str, items: Vec<(String, bool)>| -> Vec<Line> {
        let mut l = padded(Line::new(label(field, name)));
        for (text, chosen) in items {
            l.push(chip(&text, chosen, w.field == field));
            l.push(Span::raw(" "));
        }
        wrap_hang(&l, width + PAD.len(), &hang)
    };

    lines.extend(choices(
        0,
        "host",
        app.hosts
            .iter()
            .enumerate()
            .map(|(i, h)| (h.cfg.name.clone(), i == w.host))
            .collect(),
    ));
    lines.extend(choices(
        1,
        "harness",
        harnesses
            .iter()
            .map(|h| (h.display_name().to_owned(), *h == w.harness))
            .collect(),
    ));
    let note = |text: &str, style: Style| {
        padded(Line::new(vec![
            Span::raw(" ".repeat(2 + LABEL + 1)),
            Span::new(text.to_owned(), style),
        ]))
    };
    if !probed {
        lines.push(note(
            &format!("{} looking for harnesses on {host}…", spinner(app.now_ms)),
            muted.italic(),
        ));
    } else if app.hosts.get(w.host).is_some_and(|h| h.harnesses.is_empty()) {
        lines.push(note(
            "no harness found on the host's PATH",
            Style::new().fg(theme.error),
        ));
    }
    lines.push(Line::default());

    let names = ["directory", "model", "permission"];
    let placeholders = [
        "~ is the home directory",
        "the harness's default",
        "the harness's default",
    ];
    for (i, name) in names.iter().enumerate() {
        let field = i + 2;
        let focused = w.field == field;
        let value = &w.fields[i];
        let shown = tail(value, value_w);
        let mut l = padded(Line::new(label(field, name)));
        if value.is_empty() && !focused {
            l.push(Span::new(placeholders[i], muted.italic()));
        } else {
            l.push(Span::raw(shown.clone()));
        }
        if focused {
            cursor = Some((
                (PAD.len() + 2 + LABEL + crate::wrap::width(&shown)) as u16,
                lines.len() as u16,
            ));
        }
        lines.push(l);
        if field == 3 && focused && !models.is_empty() {
            // Long lists show a window around the pick.
            const SHOWN: usize = 8;
            let at = models.iter().position(|m| m == value.trim());
            let start = at
                .map_or(0, |i| i.saturating_sub(SHOWN / 2))
                .min(models.len().saturating_sub(SHOWN));
            let end = (start + SHOWN).min(models.len());
            let mut m = padded(Line::raw(" ".repeat(2 + LABEL)));
            if start > 0 {
                m.push(Span::new(format!("+{start} … "), muted));
            }
            for (i, model) in models.iter().enumerate().take(end).skip(start) {
                m.push(chip(model, at == Some(i), false));
                m.push(Span::raw(" "));
            }
            if end < models.len() {
                m.push(Span::new(format!("… +{} ", models.len() - end), muted));
            }
            m.push(Span::new(" ←/→", muted));
            lines.extend(wrap_hang(&m, width + PAD.len(), &hang));
        }
        let modes = permission_modes(w.harness);
        if field == 4 && focused && !modes.is_empty() {
            let mut m = padded(Line::raw(" ".repeat(2 + LABEL)));
            for mode in modes {
                m.push(chip(mode, value == mode, false));
                m.push(Span::raw(" "));
            }
            m.push(Span::new(" ←/→", muted));
            lines.extend(wrap_hang(&m, width + PAD.len(), &hang));
        }
    }
    lines.push(Line::default());

    // The first prompt: a few wrapped lines behind a bar.
    let focused = w.field == 5;
    lines.push(padded(Line::new(label(5, "first prompt"))));
    let bar = Span::new(
        "┃ ",
        Style::new().fg(if focused { theme.accent } else { theme.border }),
    );
    let text_w = width.saturating_sub(4).max(8);
    let prompt = &w.fields[3];
    let mut rows = if prompt.is_empty() {
        vec![Line::default()]
    } else {
        wrap_line(&Line::raw(prompt.clone()), text_w, "")
    };
    if prompt.ends_with('\n') {
        rows.push(Line::default());
    }
    let rows: Vec<Line> = rows.split_off(rows.len().saturating_sub(4));
    let last = rows.len() - 1;
    for (r, row) in rows.into_iter().enumerate() {
        let mut l = padded(Line::new(vec![Span::raw("  "), bar.clone()]));
        if prompt.is_empty() {
            l.push(Span::new("optional · alt-enter adds a line", muted.italic()));
        } else {
            if focused && r == last {
                cursor = Some(((PAD.len() + 4 + row.width()) as u16, lines.len() as u16));
            }
            l.spans.extend(row.spans);
        }
        if focused && prompt.is_empty() {
            cursor = Some(((PAD.len() + 4) as u16, lines.len() as u16));
        }
        lines.push(l);
    }
    lines.push(Line::default());

    let status = if let Some(e) = &w.error {
        Line::styled(format!("✖ {e}"), Style::new().fg(theme.error))
    } else if w.busy {
        Line::styled(format!("{} checking the directory…", spinner(app.now_ms)), muted)
    } else {
        Line::new(vec![
            Span::new("⏵ ", accent),
            Span::new(
                format!(
                    "starts {} on {host}, detached; it keeps running if chameleon quits",
                    w.harness.display_name()
                ),
                muted,
            ),
        ])
    };
    lines.extend(wrap_hang(
        &padded(status),
        width,
        &Line::raw(PAD.to_owned() + "  "),
    ));
    let footer = "tab next · ←/→ choose · ctrl-enter start · esc cancel";
    let (rect, lines, cursor) = fit_content(rect, lines, cursor);
    OverlayView {
        rect,
        title: "new session".into(),
        lines,
        footer: Some(Line::styled(footer, muted)),
        cursor,
        tone: None,
    }
}

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
