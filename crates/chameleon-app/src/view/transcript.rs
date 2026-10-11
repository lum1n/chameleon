//! The conversation pane: items rendered through `ItemDetail`, wrapped and
//! cached per item, windowed to the visible lines.

use acc_core::Harness;
use acc_core::event::ItemKind;
use acc_core::item::{FileDiff, ItemDetail, LineTag};
use acc_core::transcript::{Item, ItemStatus};

use crate::config::Theme;
use crate::markdown::{self, MdStyle};
use crate::model::{App, Pane};
use crate::slot::{Phase, Slot};
use crate::style::{Color, Line, Span, Style};
use crate::view::{PaneView, Rect, Scrollbar};
use crate::wrap::{truncate, wrap_line};

/// Output lines kept for a folded command or tool result.
const FOLD_TAIL: usize = 6;

pub fn build(app: &mut App, rect: Rect) -> PaneView {
    let inner = rect.inner();
    let w = (inner.w as usize).max(4);
    let h = inner.h as usize;
    let theme = app.theme;
    let focused = app.focus == Pane::Transcript;
    let key = app.selected.clone();
    let Some(slot) = key
        .as_ref()
        .and_then(|k| app.slots.iter_mut().find(|s| &s.key == k))
    else {
        return empty(
            rect,
            &theme,
            &["no session selected", "", "n  start one", "j/k  pick one"],
        );
    };
    let title = title_line(slot, &theme);
    let Some(session) = &slot.session else {
        let msg: &[&str] = match slot.phase {
            Phase::Listed if slot.is_alive() => &["press enter to attach"],
            Phase::Listed => &[
                "this session has exited",
                "",
                "enter  read its transcript",
                "D  remove it",
            ],
            _ => &["starting…"],
        };
        let mut v = empty(rect, &theme, msg);
        v.title = title;
        return v;
    };
    let harness = session.spec().harness;
    let items = session.transcript().items();
    let cursor = slot.cursor.filter(|c| *c < items.len());

    // Heights of every item (cached), then the window.
    let mut heights = Vec::with_capacity(items.len());
    let mut total = 0usize;
    for (idx, item) in items.iter().enumerate() {
        let rev = slot.revisions.get(&item.id).copied().unwrap_or(0);
        let expanded = slot.expanded.contains(&item.id);
        let selected = cursor == Some(idx);
        let rev = rev.wrapping_mul(4) + u64::from(expanded) * 2 + u64::from(selected);
        let cacheable = item.status != ItemStatus::Running;
        let n = slot
            .wrap
            .get_or_build(&item.id, w, rev, cacheable, || {
                render_item(item, harness, expanded, selected, &theme, w)
            })
            .len();
        heights.push(n);
        total += n;
    }
    let pending: Vec<Line> = if session.permissions().is_empty() {
        Vec::new()
    } else {
        pending_lines(session, &theme, w)
    };
    total += pending.len();

    if slot.scroll.to_cursor
        && let Some(c) = cursor
    {
        slot.scroll.to_cursor = false;
        let top: usize = heights[..c].iter().sum();
        let bottom = top + heights[c];
        let max_from_bottom = total.saturating_sub(h);
        let cur_from_bottom = slot.scroll.from_bottom.min(max_from_bottom);
        let view_top = total.saturating_sub(h + cur_from_bottom);
        let view_bottom = view_top + h;
        if top < view_top {
            slot.scroll.from_bottom = total.saturating_sub(h + top);
        } else if bottom > view_bottom {
            slot.scroll.from_bottom = total.saturating_sub(bottom);
        }
    }
    if slot.scroll.follow {
        slot.scroll.from_bottom = 0;
    }
    let max_from_bottom = total.saturating_sub(h);
    slot.scroll.from_bottom = slot.scroll.from_bottom.min(max_from_bottom);
    let start = total.saturating_sub(h + slot.scroll.from_bottom);
    let end = (start + h).min(total);

    let mut lines = Vec::with_capacity(h);
    let mut pos = 0usize;
    for (idx, item) in items.iter().enumerate() {
        let n = heights[idx];
        if pos + n <= start {
            pos += n;
            continue;
        }
        if pos >= end {
            break;
        }
        let cached = slot.wrap.get(&item.id);
        for (i, l) in cached.iter().enumerate() {
            let p = pos + i;
            if p >= start && p < end {
                lines.push(l.clone());
            }
        }
        pos += n;
    }
    for (i, l) in pending.into_iter().enumerate() {
        let p = pos + i;
        if p >= start && p < end {
            lines.push(l);
        }
    }
    log::debug!(
        "transcript: items={} total={} h={} w={} start={} end={} lines={} follow={} from_bottom={}",
        items.len(),
        total,
        h,
        w,
        start,
        end,
        lines.len(),
        slot.scroll.follow,
        slot.scroll.from_bottom
    );
    if items.is_empty() && lines.is_empty() {
        let hint = if session.is_alive() {
            "say something below"
        } else {
            "nothing recorded"
        };
        lines.push(Line::styled(hint, Style::new().fg(theme.muted)));
    }
    PaneView {
        rect,
        title,
        lines,
        focused,
        cursor: None,
        scrollbar: (total > h).then_some(Scrollbar {
            total,
            offset: start,
            visible: h,
        }),
        hits: Vec::new(),
    }
}

fn empty(rect: Rect, theme: &Theme, msg: &[&str]) -> PaneView {
    PaneView {
        rect,
        title: Line::default(),
        lines: msg
            .iter()
            .map(|m| Line::styled(*m, Style::new().fg(theme.muted)))
            .collect(),
        focused: false,
        cursor: None,
        scrollbar: None,
        hits: Vec::new(),
    }
}

fn title_line(slot: &Slot, theme: &Theme) -> Line {
    let mut l = Line::new(vec![Span::new(
        format!(" {} ", slot.title()),
        Style::new().bold(),
    )]);
    l.push(Span::new(
        format!("{} · {} ", slot.spec.cwd, slot.harness().display_name()),
        Style::new().fg(theme.muted),
    ));
    if let Some(m) = &slot.spec.model {
        l.push(Span::new(format!("{m} "), Style::new().fg(theme.muted)));
    }
    l
}

fn pending_lines(session: &acc_core::session::Session, theme: &Theme, w: usize) -> Vec<Line> {
    let mut out = vec![Line::default()];
    for p in session.permissions() {
        let glyph = if p.kind == acc_core::BlockKind::Question {
            "?"
        } else {
            "⚠"
        };
        let mut head = Line::new(vec![Span::new(
            format!("{glyph} "),
            Style::new().fg(theme.permission).bold(),
        )]);
        head.push(Span::new(
            p.title.clone(),
            Style::new().fg(theme.permission).bold(),
        ));
        out.extend(wrap_line(&head, w, "  "));
        let opts: Vec<String> = p
            .options
            .iter()
            .enumerate()
            .map(|(i, o)| format!("{} {}", i + 1, o.label))
            .collect();
        let mut l = Line::new(vec![Span::new("  ", Style::new())]);
        l.push(Span::new(opts.join("   "), Style::new().fg(theme.permission)));
        l.push(Span::new("   y approve · N deny", Style::new().fg(theme.muted)));
        out.extend(wrap_line(&l, w, "  "));
    }
    out
}

/// Renders one item to unwrapped-then-wrapped lines of width `w`.
pub fn render_item(
    item: &Item,
    harness: Harness,
    expanded: bool,
    selected: bool,
    theme: &Theme,
    w: usize,
) -> Vec<Line> {
    let mut raw: Vec<Line> = Vec::new();
    let muted = Style::new().fg(theme.muted);
    let md = MdStyle {
        base: Style::new().fg(theme.agent),
        code: Style::new().fg(theme.tool),
        heading: Style::new().bold().fg(theme.accent),
        quote: Style::new().dim().italic(),
        bullet: Style::new().fg(theme.accent),
    };
    let status_mark = match item.status {
        ItemStatus::Running => Some(Span::new(" …", Style::new().fg(theme.thinking))),
        ItemStatus::Failed => Some(Span::new(" ✖", Style::new().fg(theme.error))),
        ItemStatus::Completed => None,
    };
    let detail = item.detail(harness);
    match item.kind {
        ItemKind::UserMessage => {
            let text = item.text.trim_end();
            for (i, l) in text.lines().enumerate() {
                let prefix = if i == 0 { "› " } else { "  " };
                raw.push(Line::new(vec![
                    Span::new(prefix, Style::new().fg(theme.user).bold()),
                    Span::new(l, Style::new().fg(theme.user)),
                ]));
            }
            if text.is_empty() {
                raw.push(Line::styled("› ", Style::new().fg(theme.user).bold()));
            }
        }
        ItemKind::AgentMessage => {
            let text = item.text.trim_end();
            if text.is_empty() && item.status == ItemStatus::Running {
                raw.push(Line::styled("…", Style::new().fg(theme.thinking)));
            } else {
                raw.extend(markdown::render(text, &md));
                if let Some(m) = status_mark
                    && let Some(last) = raw.last_mut()
                {
                    last.push(m);
                }
            }
        }
        ItemKind::Reasoning => {
            let text = item.text.trim();
            if expanded {
                raw.push(Line::styled("∴ thinking", muted.italic()));
                for l in text.lines() {
                    raw.push(Line::styled(format!("  {l}"), muted.italic()));
                }
            } else {
                let first = text.lines().next().unwrap_or("");
                let mut l = Line::new(vec![Span::new("∴ ", muted)]);
                let summary = if first.is_empty() {
                    "thinking".to_owned()
                } else {
                    first.to_owned()
                };
                l.push(Span::new(truncate(&summary, w.saturating_sub(8)), muted.italic()));
                if let Some(m) = status_mark {
                    l.push(m);
                } else if text.lines().count() > 1 {
                    l.push(Span::new(" ⌄", muted));
                }
                raw.push(l);
            }
        }
        ItemKind::ToolCall | ItemKind::CommandExecution | ItemKind::FileChange => match &detail {
            ItemDetail::Command {
                command,
                output,
                exit_code,
                ..
            } => {
                let mut head = Line::new(vec![Span::new("$ ", Style::new().fg(theme.tool).bold())]);
                head.push(Span::new(command.trim(), Style::new().fg(theme.tool)));
                match exit_code {
                    Some(0) | None => {}
                    Some(c) => head.push(Span::new(format!(" exit {c}"), Style::new().fg(theme.error))),
                }
                if let Some(m) = status_mark {
                    head.push(m);
                }
                raw.push(head);
                raw.extend(folded(output, expanded, muted));
            }
            ItemDetail::Tool {
                name, title, output, ..
            } => {
                let mut head = Line::new(vec![Span::new("⚙ ", Style::new().fg(theme.tool).bold())]);
                head.push(Span::new(name.clone(), Style::new().fg(theme.tool).bold()));
                if !title.is_empty() {
                    head.push(Span::new(
                        format!(" {}", title.replace('\n', " ")),
                        Style::new().fg(theme.tool),
                    ));
                }
                if let Some(m) = status_mark {
                    head.push(m);
                }
                raw.push(head);
                if let Some(o) = output {
                    raw.extend(folded(o, expanded, muted));
                } else if !item.text.trim().is_empty() {
                    raw.extend(folded(&item.text, expanded, muted));
                }
            }
            ItemDetail::FileChange { files } => {
                for f in files {
                    raw.push(file_head(f, theme));
                    if expanded {
                        raw.extend(hunks(f, theme));
                    }
                }
                if files.is_empty() {
                    raw.push(Line::styled("± file change", Style::new().fg(theme.tool)));
                }
                if let Some(m) = status_mark
                    && let Some(last) = raw.first_mut()
                {
                    last.push(m);
                }
                if !expanded
                    && files.iter().any(|f| !f.hunks.is_empty())
                    && let Some(last) = raw.last_mut()
                {
                    last.push(Span::new("  space: diff", muted));
                }
            }
            ItemDetail::Text => {
                let text = item.text.trim_end();
                raw.extend(folded(text, expanded, muted));
                if raw.is_empty() {
                    raw.push(Line::styled("⚙", Style::new().fg(theme.tool)));
                }
            }
        },
    }
    // Children of a subagent are indented.
    let indent = if item.parent.is_some() { "  " } else { "" };
    let mut out: Vec<Line> = Vec::new();
    for l in &raw {
        let mut l = l.clone();
        if !indent.is_empty() {
            l.spans.insert(0, Span::raw(indent));
        }
        out.extend(wrap_line(&l, w, if indent.is_empty() { "  " } else { "    " }));
    }
    if selected {
        for l in &mut out {
            if let Some(first) = l.spans.first_mut() {
                first.style = first.style.bg(Color::DarkGray);
            }
            l.spans.insert(0, Span::new("▎", Style::new().fg(theme.accent)));
        }
    }
    out.push(Line::default());
    out
}

fn folded(text: &str, expanded: bool, style: Style) -> Vec<Line> {
    let text = text.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    if expanded || lines.len() <= FOLD_TAIL {
        for l in lines {
            out.push(Line::styled(format!("  {l}"), style));
        }
    } else {
        out.push(Line::styled(
            format!("  … {} more lines (space)", lines.len() - FOLD_TAIL),
            style.italic(),
        ));
        for l in &lines[lines.len() - FOLD_TAIL..] {
            out.push(Line::styled(format!("  {l}"), style));
        }
    }
    out
}

pub fn file_head(f: &FileDiff, theme: &Theme) -> Line {
    use acc_core::item::FileChangeKind::*;
    let (glyph, word) = match f.kind {
        Add => ("+", "add"),
        Delete => ("−", "delete"),
        Update => ("±", "edit"),
        Rename => ("→", "rename"),
    };
    let mut l = Line::new(vec![Span::new(
        format!("{glyph} "),
        Style::new().fg(theme.tool).bold(),
    )]);
    if let Some(old) = &f.old_path {
        l.push(Span::new(format!("{old} → "), Style::new().fg(theme.muted)));
    }
    l.push(Span::new(f.path.clone(), Style::new().fg(theme.tool).bold()));
    l.push(Span::new(format!(" {word} "), Style::new().fg(theme.muted)));
    l.push(Span::new(format!("+{}", f.added), Style::new().fg(theme.add)));
    l.push(Span::new(" ", Style::new()));
    l.push(Span::new(
        format!("−{}", f.removed),
        Style::new().fg(theme.remove),
    ));
    l
}

pub fn hunks(f: &FileDiff, theme: &Theme) -> Vec<Line> {
    let mut out = Vec::new();
    for h in &f.hunks {
        if h.old_start > 0 || h.new_start > 0 {
            out.push(Line::styled(
                format!(
                    "  @@ -{},{} +{},{} @@",
                    h.old_start, h.old_lines, h.new_start, h.new_lines
                ),
                Style::new().fg(theme.accent),
            ));
        }
        for l in &h.lines {
            let (p, s) = match l.tag {
                LineTag::Context => (" ", Style::new().fg(theme.muted)),
                LineTag::Add => ("+", Style::new().fg(theme.add)),
                LineTag::Remove => ("-", Style::new().fg(theme.remove)),
            };
            out.push(Line::styled(format!("  {p}{}", l.text), s));
        }
    }
    out
}
