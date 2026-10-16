//! Skills and MCP servers on a host, one column per harness:
//!
//! ```text
//! ╭ skills & MCP · mini ─────────────────────────────────────────────────────╮
//! │ skills  mcp                                                        user   │
//! │                                                                          │
//! │                     claude codex  pi                                     │
//! │ ❯ review              ●     ◆     ◆    Review a change against the repo… │
//! │   release             ·     ◆     ◆    Cut a release                     │
//! │   notes               only in claude: A adopts it into the store         │
//! │                                                                          │
//! │ review  ~/.agents/skills/review · from vercel-labs/skills                │
//! ╰ tab skills/mcp · h/l harness · space link · a all · u none · X delete … ╯
//! ```

use acc_core::Harness;
use acc_core::loadout::skills::LinkState;

use crate::loadout::{Load, LoadoutView, McpState, SkillRow, Tab, scope_name, transport_summary};
use crate::model::App;
use crate::style::{Line, Span, Style};
use crate::view::overlay::centered;
use crate::view::{OverlayView, Rect};
use crate::wrap::{fit, truncate, width};

/// Width of a harness's column.
fn col_w(h: Harness) -> usize {
    width(h.program()).max(5) + 1
}

/// `s` centred in `w` columns.
fn center(s: &str, w: usize) -> String {
    let left = w.saturating_sub(width(s)) / 2;
    fit(&format!("{}{s}", " ".repeat(left)), w)
}

pub fn build(app: &App, view: &LoadoutView, area: Rect) -> OverlayView {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let accent = Style::new().fg(theme.accent);
    let rect = centered(
        area,
        area.w.saturating_sub(2).min(120),
        area.h.saturating_sub(2).max(10),
    );
    let inner = rect.inner();
    let w = (inner.w as usize).saturating_sub(2);
    let pad = |mut l: Line| {
        l.spans.insert(0, Span::raw(" "));
        l
    };

    let tab = |label: &str, on: bool| {
        if on {
            Span::new(format!(" {label} "), accent.bold().reversed())
        } else {
            Span::new(format!(" {label} "), muted)
        }
    };
    let mut top = Line::new(vec![
        tab("skills", view.tab == Tab::Skills),
        Span::raw(" "),
        tab("mcp", view.tab == Tab::Mcp),
    ]);
    let scope = scope_name(&view.scope);
    let gap = w.saturating_sub(top.width() + width(&scope));
    top.push(Span::raw(" ".repeat(gap.max(1))));
    top.push(Span::new(truncate(&scope, w.saturating_sub(top.width())), muted));
    let mut lines = vec![pad(top), Line::default()];

    let name_w = 22.min(w / 3);
    let cols_w: usize = view.harnesses.iter().map(|h| col_w(*h)).sum();
    let rest = w.saturating_sub(2 + name_w + cols_w + 2);
    let mut head = Line::raw(" ".repeat(2 + name_w));
    for (i, h) in view.harnesses.iter().enumerate() {
        let style = if i == view.col { accent.bold() } else { muted };
        head.push(Span::new(center(h.program(), col_w(*h)), style));
    }
    lines.push(pad(head));

    let (rows, detail, notes) = match view.tab {
        Tab::Skills => skill_lines(app, view, name_w, rest),
        Tab::Mcp => mcp_lines(app, view, name_w, rest),
    };
    // Rows, then a blank, the details, the notes and the input line.
    let fixed = lines.len() + 1 + detail.len() + notes.len() + usize::from(view.input.is_some());
    let room = (inner.h as usize).saturating_sub(fixed + 1).max(1);
    let start = (view.row + 1).saturating_sub(room);
    lines.extend(rows.into_iter().skip(start).take(room).map(pad));
    lines.push(Line::default());
    lines.extend(detail.into_iter().map(pad));
    lines.extend(notes.into_iter().map(pad));

    let mut cursor = None;
    if let Some(input) = &view.input {
        let h = view.harnesses.get(view.col).map_or("", |h| h.program());
        let prompt = format!("new server for {h}: ");
        let shown = truncate(input, w.saturating_sub(width(&prompt) + 1));
        cursor = Some(((1 + width(&prompt) + width(&shown)) as u16, lines.len() as u16));
        lines.push(pad(Line::new(vec![Span::new(prompt, accent), Span::raw(shown)])));
    }
    lines.truncate(inner.h as usize);

    let footer = if view.input.is_some() {
        "name command args…  or  name https://url · enter add · esc cancel"
    } else if view.armed.is_some() {
        "X again deletes it · any other key keeps it"
    } else {
        match view.tab {
            Tab::Skills => {
                "tab mcp · h/l harness · space link · a all · u none · A adopt · F replace · X delete · p project · r reload · esc"
            }
            Tab::Mcp => {
                "tab skills · h/l harness · space add/remove · a all · e on/off · n new · X delete · p project · r reload · esc"
            }
        }
    };
    let host = app
        .hosts
        .iter()
        .find(|h| h.cfg.name == view.host)
        .map_or(view.host.as_str(), |h| h.cfg.name.as_str());
    OverlayView {
        rect,
        title: format!("skills & MCP · {host}"),
        lines,
        footer: Some(Line::styled(truncate(footer, w), muted)),
        cursor,
        tone: view.armed.is_some().then_some(theme.error),
    }
}

type Parts = (Vec<Line>, Vec<Line>, Vec<Line>);

/// A row's start: the selection mark and the name.
fn row_head(app: &App, selected: bool, name: &str, name_w: usize, style: Style) -> Line {
    let mark = if selected { "❯ " } else { "  " };
    Line::new(vec![
        Span::new(mark, Style::new().fg(app.theme.accent)),
        Span::new(fit(&truncate(name, name_w.saturating_sub(1)), name_w), style),
    ])
}

fn cell(view: &LoadoutView, row: usize, col: usize, h: Harness, glyph: &str, style: Style) -> Span {
    let style = if row == view.row && col == view.col {
        style.reversed()
    } else {
        style
    };
    Span::new(center(glyph, col_w(h)), style)
}

fn skill_lines(app: &App, view: &LoadoutView, name_w: usize, rest: usize) -> Parts {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let listing = match &view.skills {
        Load::Loading => return (vec![Line::styled("  reading the skills…", muted)], vec![], vec![]),
        Load::Failed(e) => {
            return (
                vec![Line::styled(
                    format!("  skills: {e}"),
                    Style::new().fg(theme.error),
                )],
                vec![],
                vec![],
            );
        }
        Load::Ready(l) => l,
    };
    let rows = view.skill_rows();
    if rows.is_empty() {
        return (
            vec![Line::styled(
                "  no skills in the store yet (~/.agents/skills, or the project's .agents/skills)",
                muted,
            )],
            vec![],
            vec![],
        );
    }
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let selected = r == view.row;
        match row {
            SkillRow::Store(i) => {
                let skill = &listing.skills[*i];
                let mut l = row_head(app, selected, &skill.name, name_w, Style::new().bold());
                for (c, h) in view.harnesses.iter().enumerate() {
                    let (glyph, style) = match view.link_state(*i, *h) {
                        Some(LinkState::Linked) => ("●", Style::new().fg(theme.done)),
                        Some(LinkState::Native) => ("◆", muted),
                        Some(LinkState::Missing) => ("·", muted),
                        Some(LinkState::Foreign { .. }) => ("!", Style::new().fg(theme.permission)),
                        None => (" ", muted),
                    };
                    l.push(cell(view, r, c, *h, glyph, style));
                }
                l.push(Span::raw("  "));
                l.push(Span::new(truncate(skill.description(), rest), muted));
                out.push(l);
            }
            SkillRow::Unmanaged { harness, name } => {
                let mut l = row_head(app, selected, name, name_w, muted);
                l.push(Span::new(
                    truncate(
                        &format!("only in {}: A adopts it into the store", harness.program()),
                        rest + view.harnesses.iter().map(|h| col_w(*h)).sum::<usize>(),
                    ),
                    muted.italic(),
                ));
                out.push(l);
            }
        }
    }
    let mut detail = Vec::new();
    match rows.get(view.row) {
        Some(SkillRow::Store(i)) => {
            let skill = &listing.skills[*i];
            let mut l = Line::new(vec![
                Span::new(skill.name.clone(), Style::new().bold()),
                Span::new(format!("  {}/{}", listing.store, skill.name), muted),
            ]);
            if let Some(src) = &skill.source {
                l.push(Span::new(format!(" · from {}", src.source), muted));
            }
            detail.push(l);
            if let Some(LinkState::Foreign { what }) =
                view.harnesses.get(view.col).and_then(|h| view.link_state(*i, *h))
            {
                detail.push(Line::styled(
                    format!("in the way for this harness: {what}"),
                    Style::new().fg(theme.permission),
                ));
            }
        }
        Some(SkillRow::Unmanaged { harness, name }) => {
            let dir = listing
                .places
                .iter()
                .find(|p| p.harness == *harness)
                .map_or(String::new(), |p| p.dir.clone());
            detail.push(Line::new(vec![
                Span::new(name.clone(), Style::new().bold()),
                Span::new(format!("  {dir}/{name}"), muted),
            ]));
        }
        None => {}
    }
    let notes = vec![Line::styled(
        "● linked  ◆ reads the store itself  · not there  ! something else is in the way",
        muted,
    )];
    (out, detail, notes)
}

fn mcp_lines(app: &App, view: &LoadoutView, name_w: usize, rest: usize) -> Parts {
    let theme = app.theme;
    let muted = Style::new().fg(theme.muted);
    let mut notes = Vec::new();
    for (h, f) in &view.mcp {
        if let Load::Failed(e) = f {
            notes.push(Line::styled(
                format!("{}: {e}", h.program()),
                Style::new().fg(theme.error),
            ));
        }
    }
    let names = view.mcp_names();
    if names.is_empty() {
        let loading = view.mcp.iter().any(|(_, f)| *f == Load::Loading);
        let text = if loading {
            "  reading the MCP configs…"
        } else {
            "  no MCP servers configured: n adds one"
        };
        return (vec![Line::styled(text, muted)], vec![], notes);
    }
    let mut out = Vec::new();
    for (r, name) in names.iter().enumerate() {
        let mut l = row_head(app, r == view.row, name, name_w, Style::new().bold());
        for (c, h) in view.harnesses.iter().enumerate() {
            let (glyph, style) = match view.mcp_state(name, *h) {
                McpState::Enabled => ("●", Style::new().fg(theme.done)),
                McpState::Disabled => ("○", muted),
                McpState::Absent => ("·", muted),
                McpState::Unknown => (" ", muted),
            };
            l.push(cell(view, r, c, *h, glyph, style));
        }
        if let Some((_, s)) = view.server(name) {
            l.push(Span::raw("  "));
            l.push(Span::new(truncate(&transport_summary(&s.transport), rest), muted));
        }
        out.push(l);
    }
    let mut detail = Vec::new();
    if let Some((h, s)) = names.get(view.row).and_then(|n| view.server(n)) {
        detail.push(Line::new(vec![
            Span::new(s.name.clone(), Style::new().bold()),
            Span::new(format!("  as {} has it", h.program()), muted),
        ]));
        detail.push(Line::raw(transport_summary(&s.transport)));
    }
    notes.insert(
        0,
        Line::styled(
            "● on  ○ kept but off  · not there  (blank: no config file for the harness here)",
            muted,
        ),
    );
    (out, detail, notes)
}
