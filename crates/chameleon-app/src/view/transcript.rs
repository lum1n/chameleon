//! The conversation pane: items rendered through `ItemDetail`, wrapped and
//! cached per item, windowed to the visible lines.
//!
//! The pane shows one conversation at a time: the main one, or a
//! subagent's (the children of its [`ItemKind::Subagent`] item) once it is
//! opened, under a breadcrumb back to main.

use std::collections::HashMap;

use acc_core::Harness;
use acc_core::event::ItemKind;
use acc_core::item::{FileDiff, ItemDetail, LineTag};
use acc_core::transcript::{Item, ItemStatus};

use crate::config::{Blocks, Theme};
use crate::markdown::{self, MdStyle};
use crate::model::{App, Pane};
use crate::slot::Phase;
use crate::style::{Color, Line, Span, Style};
use crate::view::{PaneView, Rect, Scrollbar, spinner};
use crate::wrap::{truncate, width, wrap_hang, wrap_line};

pub fn build(app: &mut App, rect: Rect) -> PaneView {
    let w = (rect.w as usize).max(4);
    let h = rect.h as usize;
    let theme = app.theme;
    let spin = spinner(app.now_ms);
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
        return empty(rect, &theme, msg);
    };
    let ctx = ItemCtx {
        harness: session.spec().harness,
        theme: &theme,
        w,
        spin,
        cwd: &session.spec().cwd,
        blocks: app.cfg.general.blocks,
    };
    let items = session.transcript().items();
    let cursor = slot.cursor.filter(|c| *c < items.len());
    let scope = slot.scope().map(str::to_owned);
    let scope = scope.as_deref();
    // How many items each subagent's conversation has so far.
    let mut children: HashMap<&str, usize> = HashMap::new();
    for p in items.iter().filter_map(|it| it.parent.as_deref()) {
        *children.entry(p).or_default() += 1;
    }
    let header = scope
        .map(|id| agent_header(items, id, ctx.harness, &theme, spin, w))
        .unwrap_or_default();
    let h = h.saturating_sub(header.len());

    // Heights of every item (cached), then the window. Items of other
    // conversations stay in the flat list (so the cursor and scroll math
    // need no second indexing scheme) but render to zero lines.
    let mut heights = Vec::with_capacity(items.len());
    let mut total = 0usize;
    for (idx, item) in items.iter().enumerate() {
        if hidden(item, scope) {
            heights.push(0);
            continue;
        }
        let rev = slot.revisions.get(&item.id).copied().unwrap_or(0);
        let look = Look {
            expanded: slot.expanded.contains(&item.id),
            selected: cursor == Some(idx),
            // Steps of one turn (thinking, tools) stack without gaps.
            gap: !(is_step(item) && next_visible(items, scope, idx).is_some_and(is_step)),
            children: children.get(item.id.as_str()).copied().unwrap_or(0),
        };
        let rev = rev
            .wrapping_mul(1 << 20)
            .wrapping_add(look.children as u64)
            .wrapping_mul(8)
            + u64::from(look.gap) * 4
            + u64::from(look.expanded) * 2
            + u64::from(look.selected);
        let cacheable = item.status != ItemStatus::Running;
        let n = slot
            .wrap
            .get_or_build(&item.id, w, rev, cacheable, || render_item(item, look, &ctx))
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

    let mut lines = header;
    let mut pos = 0usize;
    for (idx, item) in items.iter().enumerate() {
        let n = heights[idx];
        // Another conversation's: not rendered this pass, so its cache
        // entry (if any, from when it was last shown) is stale.
        if n == 0 {
            continue;
        }
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
    if total == 0 {
        let hint = if scope.is_some() {
            "nothing from this agent yet"
        } else if session.is_alive() {
            "say something below"
        } else {
            "nothing recorded"
        };
        lines.push(Line::styled(hint, Style::new().fg(theme.muted)));
    }
    PaneView {
        rect,
        lines,
        focused,
        cursor: None,
        scrollbar: (total > h).then_some(Scrollbar {
            total: total + rect.h as usize - h,
            offset: start,
            visible: rect.h as usize,
        }),
        hits: Vec::new(),
    }
}

fn empty(rect: Rect, theme: &Theme, msg: &[&str]) -> PaneView {
    // Centred vertically: an empty transcript is a landing page.
    let pad = (rect.h as usize).saturating_sub(msg.len()) / 2;
    let mut lines = vec![Line::default(); pad];
    lines.extend(msg.iter().map(|m| Line::styled(*m, Style::new().fg(theme.muted))));
    PaneView {
        rect,
        lines,
        focused: false,
        cursor: None,
        scrollbar: None,
        hits: Vec::new(),
    }
}

/// Open requests as cards at the end of the transcript.
///
/// ```text
/// ╭─ ⚠ approval ───────────────────────────────╮
/// │ Bash wants to run a command                 │
/// │ $ cargo test --workspace                    │
/// │ ❯ 1 Allow   2 Always allow   3 Deny         │
/// ╰─────────────── y approve · N deny · enter ──╯
/// ```
fn pending_lines(session: &acc_core::session::Session, theme: &Theme, w: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let cw = w.saturating_sub(4).max(8);
    for p in session.permissions() {
        let question = p.kind == acc_core::BlockKind::Question;
        let mut body = wrap_line(&Line::styled(p.title.clone(), Style::new().bold()), cw, "");
        let preview = request_preview(session, p, theme, cw);
        let more = preview.len() > 3;
        body.extend(preview.into_iter().take(3));
        if more {
            body.push(Line::styled("…", Style::new().fg(theme.muted)));
        }
        let mut opts = Line::default();
        for (i, o) in p.options.iter().enumerate() {
            if i > 0 {
                opts.push(Span::raw("   "));
            }
            opts.push(Span::new(format!("{} ", i + 1), Style::new().fg(theme.muted)));
            opts.push(Span::new(
                o.label.clone(),
                Style::new().fg(option_tone(&o.id, &o.label, theme)),
            ));
        }
        body.extend(wrap_line(&opts, cw, ""));
        let steps = crate::overlay::steps(p);
        if steps.len() > 1 {
            body.push(Line::styled(
                format!("+ {} more questions", steps.len() - 1),
                Style::new().fg(theme.muted),
            ));
        }
        let (title, footer) = if question {
            ("? question", "enter: answer")
        } else {
            ("⚠ approval", "y approve · N deny · enter: card")
        };
        out.push(Line::default());
        out.extend(card(body, title, footer, theme.permission, theme.muted, w));
    }
    out
}

/// `body` in a rounded box `w` wide, with a title and a footer in its rules.
pub(crate) fn card(
    body: Vec<Line>,
    title: &str,
    footer: &str,
    tone: Color,
    muted: Color,
    w: usize,
) -> Vec<Line> {
    let w = w.max(12);
    let cw = w - 4;
    let rule = Style::new().fg(tone);
    let title = truncate(title, cw.saturating_sub(2));
    let footer = truncate(footer, cw.saturating_sub(2));
    let mut out = vec![Line::new(vec![
        Span::new("╭─ ", rule),
        Span::new(title.clone(), rule.bold()),
        Span::new(
            format!(" {}╮", "─".repeat(w.saturating_sub(width(&title) + 5))),
            rule,
        ),
    ])];
    for mut l in body {
        let pad = cw.saturating_sub(l.width());
        l.spans.insert(0, Span::new("│ ", rule));
        l.push(Span::raw(" ".repeat(pad)));
        l.push(Span::new(" │", rule));
        out.push(l);
    }
    if footer.is_empty() {
        out.push(Line::styled(format!("╰{}╯", "─".repeat(w - 2)), rule));
        return out;
    }
    out.push(Line::new(vec![
        Span::new(
            format!("╰{} ", "─".repeat(w.saturating_sub(width(&footer) + 5))),
            rule,
        ),
        Span::new(footer, Style::new().fg(muted)),
        Span::new(" ─╯", rule),
    ]));
    out
}

/// What a request is about: the command, the file change or the tool
/// call it was raised for, wrapped to `w`.
pub(crate) fn request_preview(
    session: &acc_core::session::Session,
    p: &acc_core::session::Permission,
    theme: &Theme,
    w: usize,
) -> Vec<Line> {
    let muted = Style::new().fg(theme.muted);
    let cwd = session.spec().cwd.as_str();
    let mut raw: Vec<Line> = Vec::new();
    let detail = p
        .item_id
        .as_ref()
        .and_then(|id| session.transcript().get(id))
        .map(|item| item.detail(session.spec().harness));
    match detail {
        Some(ItemDetail::Command {
            command, cwd: dir, ..
        }) => {
            raw.push(Line::new(vec![
                Span::new("$ ", Style::new().fg(theme.tool)),
                Span::raw(short_command(command.trim(), cwd)),
            ]));
            if let Some(d) = dir.filter(|d| d.trim_end_matches('/') != cwd.trim_end_matches('/')) {
                raw.push(Line::styled(format!("in {d}"), muted));
            }
        }
        Some(ItemDetail::FileChange { files }) => {
            for f in &files {
                raw.push(file_head(f, theme));
                raw.extend(hunks(f, theme));
            }
        }
        Some(ItemDetail::Tool {
            name, title, input, ..
        }) => {
            raw.push(Line::new(vec![
                Span::new(format!("{name}  "), Style::new().fg(theme.tool)),
                Span::new(short_path(&title, cwd), muted),
            ]));
            raw.extend(json_lines(&input, muted));
        }
        Some(ItemDetail::Text) | Some(ItemDetail::Subagent { .. }) | None => {
            raw.extend(json_lines(&p.input, muted))
        }
    }
    raw.iter().flat_map(|l| wrap_line(l, w, "  ")).collect()
}

/// `key: value` lines of a JSON object (a tool's input).
pub(crate) fn json_lines(v: &serde_json::Value, style: Style) -> Vec<Line> {
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

/// The colour of a request's answer: denials in red, standing grants in
/// the accent, one-off grants in green.
pub(crate) fn option_tone(id: &str, label: &str, theme: &Theme) -> Color {
    let l = label.to_ascii_lowercase();
    if crate::overlay::is_denial(id, label) {
        theme.error
    } else if l.contains("always") || l.contains("session") || l.contains("don't ask") {
        theme.accent
    } else {
        theme.done
    }
}

/// What every item renders against.
#[derive(Debug)]
pub struct ItemCtx<'a> {
    pub harness: Harness,
    pub theme: &'a Theme,
    pub w: usize,
    /// The spinner frame shown on running items.
    pub spin: &'a str,
    /// The session's directory, trimmed from paths and commands.
    pub cwd: &'a str,
    pub blocks: Blocks,
}

/// How one item is shown; part of its cache key.
#[derive(Debug, Clone, Copy, Default)]
pub struct Look {
    pub expanded: bool,
    pub selected: bool,
    /// A blank line after the item.
    pub gap: bool,
    /// Items in the conversation of a subagent run, to open.
    pub children: usize,
}

/// Thinking and tool calls: the steps between messages.
fn is_step(item: &Item) -> bool {
    !matches!(item.kind, ItemKind::UserMessage | ItemKind::AgentMessage)
}

/// Not part of the conversation shown: `scope` is the open subagent's id,
/// `None` for the main conversation.
pub(crate) fn hidden(item: &Item, scope: Option<&str>) -> bool {
    item.parent.as_deref() != scope
}

/// The next item of the conversation shown, for gap and layout decisions.
fn next_visible<'a>(items: &'a [Item], scope: Option<&str>, idx: usize) -> Option<&'a Item> {
    items[idx + 1..].iter().find(|it| !hidden(it, scope))
}

/// The breadcrumb over a subagent's conversation:
///
/// ```text
/// main › Explore › general-purpose                     ⠋ running · 14 items
/// find where the config is loaded and parsed
/// ─────────────────────────────────────────────── esc back · [ ] prev/next
/// ```
fn agent_header(
    items: &[Item],
    id: &str,
    harness: Harness,
    theme: &Theme,
    spin: &str,
    w: usize,
) -> Vec<Line> {
    let muted = Style::new().fg(theme.muted);
    let mut chain = Vec::new();
    let mut at = Some(id);
    while let Some(cur) = at {
        let Some(item) = items.iter().find(|it| it.id == cur) else {
            break;
        };
        chain.push(item);
        at = item.parent.as_deref();
    }
    chain.reverse();
    let Some(last) = chain.last() else {
        return Vec::new();
    };
    let mut crumbs = Line::new(vec![Span::new("main", muted)]);
    for (i, item) in chain.iter().enumerate() {
        crumbs.push(Span::new(" › ", muted));
        let style = if i + 1 == chain.len() {
            Style::new().fg(theme.accent).bold()
        } else {
            muted
        };
        crumbs.push(Span::new(agent_name(item, harness), style));
    }
    let n = items.iter().filter(|it| it.parent.as_deref() == Some(id)).count();
    let (glyph, word, tone) = match last.status {
        ItemStatus::Running => (spin, "running", theme.thinking),
        ItemStatus::Completed => ("✔", "done", theme.done),
        ItemStatus::Failed => ("✖", "failed", theme.error),
    };
    let meta = vec![
        Span::new(format!("{glyph} {word}"), Style::new().fg(tone)),
        Span::new(format!(" · {n} items"), muted),
    ];
    let meta_w: usize = meta.iter().map(|s| width(&s.text)).sum();
    let mut top = clip(&crumbs, w.saturating_sub(meta_w + 2));
    top.push(Span::raw(" ".repeat(w.saturating_sub(top.width() + meta_w))));
    top.spans.extend(meta);
    let mut out = vec![top];
    if let ItemDetail::Subagent { description, .. } = last.detail(harness)
        && !description.trim().is_empty()
    {
        out.push(Line::styled(
            truncate(description.trim(), w),
            Style::new().fg(theme.agent),
        ));
    }
    let keys = " esc back · [ ] prev/next · a agents";
    out.push(Line::new(vec![
        Span::new("─".repeat(w.saturating_sub(width(keys))), muted),
        Span::new(keys, muted),
    ]));
    out
}

/// A subagent run's name: its agent type, or "Task".
pub(crate) fn agent_name(item: &Item, harness: Harness) -> String {
    match item.detail(harness) {
        ItemDetail::Subagent { agent: Some(a), .. } if !a.is_empty() => a,
        _ => "Task".to_owned(),
    }
}

/// Output lines shown under a failed or running step.
const TAIL: usize = 3;

/// Renders one item to lines of width `ctx.w`.
///
/// ```text
/// ▎ fix the failing tests
///
/// ∴ Looking at the test output first
/// • Bash  cargo test                                 exit 101
///   │ test result: FAILED. 3 passed; 1 failed
/// • Read  src/lib.rs                                  212 lines
/// • Edit  src/lib.rs                                     +4 −1
///
/// The test compared against the old fixture; it passes now.
/// ```
pub fn render_item(item: &Item, look: Look, ctx: &ItemCtx) -> Vec<Line> {
    let theme = ctx.theme;
    // The cursor's item gets a marker.
    let cw = ctx.w.saturating_sub(usize::from(look.selected)).max(8);
    let muted = Style::new().fg(theme.muted);
    // What sets the block apart: a bar in front of each line (with the
    // columns it takes) or a box around it.
    let (bar, boxed) = match (ctx.blocks, item.kind) {
        (Blocks::Cards, ItemKind::UserMessage) => (None, Some(("you", theme.user))),
        (Blocks::Cards, ItemKind::AgentMessage) => (None, Some((ctx.harness.display_name(), theme.border))),
        (_, ItemKind::UserMessage) => (Some(Span::new("▎ ", Style::new().fg(theme.user).bold())), None),
        (Blocks::Bars, ItemKind::AgentMessage) => {
            (Some(Span::new("▎ ", Style::new().fg(theme.accent))), None)
        }
        (Blocks::Plain, _) => (None, None),
        _ => (Some(Span::new("│ ", Style::new().fg(theme.border))), None),
    };
    let cw = match (&bar, boxed) {
        (Some(b), _) => cw.saturating_sub(width(&b.text)).max(8),
        (None, Some(_)) => cw.saturating_sub(4).max(8),
        (None, None) => cw,
    };
    let mut out: Vec<Line> = Vec::new();
    match item.kind {
        ItemKind::UserMessage => {
            for l in item.text.trim_end().lines() {
                out.extend(wrap_line(&Line::styled(l, Style::new().fg(theme.user)), cw, ""));
            }
        }
        ItemKind::AgentMessage => {
            let text = item.text.trim_end();
            if text.is_empty() {
                if item.status == ItemStatus::Running {
                    out.push(Line::styled(ctx.spin, Style::new().fg(theme.thinking)));
                }
            } else {
                let md = MdStyle {
                    base: Style::new().fg(theme.agent),
                    code: Style::new().fg(theme.tool),
                    heading: Style::new().bold().fg(theme.accent),
                    quote: Style::new().fg(theme.muted).italic(),
                    bullet: Style::new().fg(theme.accent),
                    muted,
                    link: Style::new().fg(theme.user).underline(),
                };
                out.extend(markdown::render(text, &md, cw));
                if item.status == ItemStatus::Running
                    && let Some(last) = out.last_mut()
                {
                    last.push(Span::new(
                        format!(" {}", ctx.spin),
                        Style::new().fg(theme.thinking),
                    ));
                }
            }
        }
        ItemKind::Reasoning => out.extend(reasoning(item, look, ctx, cw)),
        ItemKind::ToolCall | ItemKind::CommandExecution | ItemKind::FileChange | ItemKind::Subagent => {
            out.extend(step(item, look, ctx, cw));
        }
    }
    if item.kind == ItemKind::UserMessage && out.is_empty() {
        out.push(Line::default());
    }
    if let Some(bar) = bar {
        for l in &mut out {
            l.spans.insert(0, bar.clone());
        }
    } else if let Some((title, tone)) = boxed
        && !out.is_empty()
    {
        out = card(out, title, "", tone, theme.muted, cw + 4);
    }
    for l in &mut out {
        if look.selected {
            l.spans.insert(0, Span::new("▌", Style::new().fg(theme.accent)));
        }
    }
    if look.gap {
        out.push(Line::default());
    }
    out
}

fn reasoning(item: &Item, look: Look, ctx: &ItemCtx, cw: usize) -> Vec<Line> {
    let muted = Style::new().fg(ctx.theme.muted);
    let text = item.text.trim();
    let glyph = if item.status == ItemStatus::Running {
        Span::new(format!("{} ", ctx.spin), Style::new().fg(ctx.theme.thinking))
    } else {
        Span::new("∴ ", muted)
    };
    if look.expanded && !text.is_empty() {
        let md = MdStyle {
            base: muted,
            code: muted,
            heading: muted.bold(),
            quote: muted.italic(),
            bullet: muted,
            muted,
            link: muted.underline(),
        };
        let mut out = vec![Line::new(vec![glyph, Span::new("thinking", muted)])];
        for l in markdown::render(text, &md, cw.saturating_sub(2)) {
            let mut line = Line::raw("  ");
            line.spans.extend(l.spans);
            out.push(line);
        }
        return out;
    }
    // Summaries often come as "**Planning the fix**"; the stars are noise.
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("thinking")
        .replace("**", "");
    let more = text.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let mut line = Line::new(vec![glyph]);
    let room = cw.saturating_sub(2 + if more { 2 } else { 0 });
    line.push(Span::new(truncate(first.trim(), room), muted));
    if more && look.selected {
        line.push(Span::new(" ⌄", muted));
    }
    vec![line]
}

/// A tool call, command or file change: one quiet line, its output under
/// a gutter when it failed, is running, or is expanded. Dimmed like
/// thinking, so the agent's own messages stand out.
fn step(item: &Item, look: Look, ctx: &ItemCtx, cw: usize) -> Vec<Line> {
    let theme = ctx.theme;
    let muted = Style::new().fg(theme.muted);
    let failed = |exit: Option<i32>| item.status == ItemStatus::Failed || exit.is_some_and(|c| c != 0);
    let glyph = |failed: bool| match item.status {
        ItemStatus::Running => Span::new(format!("{} ", ctx.spin), Style::new().fg(theme.thinking)),
        _ if failed => Span::new("✖ ", Style::new().fg(theme.error)),
        _ => Span::new("• ", muted),
    };
    let name = |n: &str| Span::new(format!("{n}  "), muted);
    let mut out = Vec::new();
    match item.detail(ctx.harness) {
        ItemDetail::Command {
            command,
            output,
            exit_code,
            ..
        } => {
            let bad = failed(exit_code);
            let mut meta = Vec::new();
            match exit_code {
                Some(c) if c != 0 => meta.push(Span::new(format!("exit {c}"), Style::new().fg(theme.error))),
                _ if item.status != ItemStatus::Running => meta.extend(count(&output, muted)),
                _ => {}
            }
            let cmd = short_command(command.trim(), ctx.cwd);
            let head = Line::new(vec![glyph(bad), Span::new("$ ", muted), Span::new(cmd, muted)]);
            out.extend(head_line(head, meta, look, has_more(&output), cw, muted));
            out.extend(output_lines(&output, look, bad, item.status, ctx, cw));
        }
        ItemDetail::Tool {
            name: tool,
            title,
            output,
            ..
        } => {
            let bad = failed(None);
            let output = output.unwrap_or_else(|| item.text.clone());
            let meta = if item.status == ItemStatus::Running {
                Vec::new()
            } else {
                count(&output, muted)
            };
            let title = short_path(&title.replace('\n', " "), ctx.cwd);
            let head = Line::new(vec![glyph(bad), name(&tool), Span::new(title, muted)]);
            out.extend(head_line(head, meta, look, has_more(&output), cw, muted));
            out.extend(output_lines(&output, look, bad, item.status, ctx, cw));
        }
        ItemDetail::FileChange { files } => {
            use acc_core::item::FileChangeKind::*;
            let bad = failed(None);
            for (i, f) in files.iter().enumerate() {
                let word = match f.kind {
                    Add => "Add",
                    Delete => "Delete",
                    Update => "Edit",
                    Rename => "Rename",
                };
                let mut path = short_path(&f.path, ctx.cwd);
                if let Some(old) = &f.old_path {
                    path = format!("{} → {path}", short_path(old, ctx.cwd));
                }
                let g = if i == 0 { glyph(bad) } else { Span::raw("  ") };
                let head = Line::new(vec![g, name(word), Span::new(path, muted)]);
                let meta = vec![
                    Span::new(format!("+{}", f.added), Style::new().fg(theme.add)),
                    Span::raw(" "),
                    Span::new(format!("−{}", f.removed), Style::new().fg(theme.remove)),
                ];
                out.extend(head_line(head, meta, look, !f.hunks.is_empty(), cw, muted));
                if look.expanded {
                    out.extend(diff_lines(f, theme, cw));
                }
            }
            if files.is_empty() {
                out.push(Line::new(vec![glyph(bad), name("Edit")]));
            }
        }
        ItemDetail::Text => {
            let text = item.text.trim_end();
            let first = text.lines().next().unwrap_or("");
            let rest = text.lines().skip(1).collect::<Vec<_>>().join("\n");
            let head = Line::new(vec![glyph(failed(None)), Span::new(first.to_owned(), muted)]);
            out.extend(head_line(
                head,
                count(&rest, muted),
                look,
                has_more(&rest),
                cw,
                muted,
            ));
            out.extend(output_lines(&rest, look, failed(None), item.status, ctx, cw));
        }
        ItemDetail::Subagent {
            agent,
            description,
            output,
            ..
        } => {
            let bad = failed(None);
            let output = output.unwrap_or_default();
            let mut meta = match look.children {
                0 => Vec::new(),
                1 => vec![Span::new("1 item", muted)],
                n => vec![Span::new(format!("{n} items"), muted)],
            };
            if look.selected && look.children > 0 {
                meta.push(Span::new("  ⏎ open", Style::new().fg(theme.accent)));
            }
            let label = agent
                .filter(|a| !a.is_empty())
                .unwrap_or_else(|| "Task".to_owned());
            let head = Line::new(vec![glyph(bad), name(&label), Span::new(description, muted)]);
            out.extend(head_line(head, meta, look, has_more(&output), cw, muted));
            out.extend(output_lines(&output, look, bad, item.status, ctx, cw));
        }
    }
    out
}

/// The head of a step: clipped to one line with `meta` at the right edge,
/// or wrapped in full when expanded.
fn head_line(head: Line, mut meta: Vec<Span>, look: Look, more: bool, cw: usize, muted: Style) -> Vec<Line> {
    if look.selected && more {
        meta.push(Span::new(if look.expanded { "  ⌃" } else { "  ⌄" }, muted));
    }
    let meta_w: usize = meta.iter().map(|s| width(&s.text)).sum();
    if look.expanded {
        let mut out = wrap_hang(&head, cw, &Line::raw("    "));
        if let Some(last) = out.last_mut() {
            let pad = cw.saturating_sub(last.width() + meta_w).max(2);
            last.push(Span::raw(" ".repeat(pad)));
            last.spans.extend(meta);
        }
        return out;
    }
    let room = cw.saturating_sub(if meta_w > 0 { meta_w + 2 } else { 0 });
    let mut line = clip(&head, room);
    if meta_w > 0 {
        line.push(Span::raw(" ".repeat(cw.saturating_sub(line.width() + meta_w))));
        line.spans.extend(meta);
    }
    vec![line]
}

/// Output under a gutter: all of it when expanded, the tail while it runs
/// or when the step failed, nothing otherwise.
fn output_lines(
    text: &str,
    look: Look,
    failed: bool,
    status: ItemStatus,
    ctx: &ItemCtx,
    cw: usize,
) -> Vec<Line> {
    let text = text.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let shown: &[&str] = if look.expanded {
        &lines
    } else if failed || status == ItemStatus::Running {
        &lines[lines.len().saturating_sub(TAIL)..]
    } else {
        &[]
    };
    let style = if failed && !look.expanded {
        Style::new().fg(ctx.theme.error)
    } else {
        Style::new().fg(ctx.theme.muted)
    };
    let gutter = Line::styled("  │ ", Style::new().fg(ctx.theme.muted));
    let mut out = Vec::new();
    for l in shown {
        let mut line = gutter.clone();
        line.push(Span::new(l.replace('\t', "    "), style));
        out.extend(wrap_hang(&line, cw, &gutter));
    }
    out
}

fn diff_lines(f: &FileDiff, theme: &Theme, cw: usize) -> Vec<Line> {
    let gutter = Line::styled("  │ ", Style::new().fg(theme.muted));
    let mut out = Vec::new();
    for h in &f.hunks {
        if h.old_start > 0 || h.new_start > 0 {
            let mut l = gutter.clone();
            l.push(Span::new(
                format!(
                    "@@ -{},{} +{},{} @@",
                    h.old_start, h.old_lines, h.new_start, h.new_lines
                ),
                Style::new().fg(theme.muted).italic(),
            ));
            out.push(l);
        }
        for dl in &h.lines {
            let (p, s) = match dl.tag {
                LineTag::Context => (" ", Style::new().fg(theme.muted)),
                LineTag::Add => ("+", Style::new().fg(theme.add)),
                LineTag::Remove => ("-", Style::new().fg(theme.remove)),
            };
            let mut l = gutter.clone();
            l.push(Span::new(format!("{p}{}", dl.text.replace('\t', "    ")), s));
            out.extend(wrap_hang(&l, cw, &gutter));
        }
    }
    out
}

/// "212 lines", or nothing for empty output.
fn count(text: &str, style: Style) -> Vec<Span> {
    match text.trim_end().lines().count() {
        0 => Vec::new(),
        1 => vec![Span::new("1 line", style)],
        n => vec![Span::new(format!("{n} lines"), style)],
    }
}

fn has_more(text: &str) -> bool {
    !text.trim().is_empty()
}

/// A path relative to the session's directory when it is inside it.
fn short_path(s: &str, cwd: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    if cwd.is_empty() {
        return s.to_owned();
    }
    s.replace(&format!("{cwd}/"), "").replace(cwd, ".")
}

/// The command without a leading `cd <session dir> &&`.
fn short_command(cmd: &str, cwd: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    for prefix in [
        format!("cd {cwd} && "),
        format!("cd \"{cwd}\" && "),
        format!("cd '{cwd}' && "),
    ] {
        if let Some(rest) = cmd.strip_prefix(&prefix) {
            return rest.to_owned();
        }
    }
    cmd.to_owned()
}

/// `line` cut to `max` columns, with `…` when cut. Newlines end it.
fn clip(line: &Line, max: usize) -> Line {
    let mut out = Line::default();
    let mut used = 0;
    for span in &line.spans {
        let text = span.text.trim_end_matches('\n');
        let (first, more) = match text.split_once('\n') {
            Some((first, _)) => (format!("{first} …"), true),
            None => (text.to_owned(), false),
        };
        let fits = used + width(&first) <= max;
        out.push(Span::new(truncate(&first, max.saturating_sub(used)), span.style));
        if !fits || more {
            break;
        }
        used += width(&first);
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
