//! The diff viewer, full screen:
//!
//! ```text
//! ╭ diff ──────────────────────────────────────────────────────────────────────────╮
//! │ git  session         +52 −9 │ src/lib.rs  edit                        +12 −3  1/3 │
//! │─────────────────────────────┼──────────────────────────────────────────────────│
//! │❯± src/lib.rs        +12 −3 │ ── @@ -10,6 +10,9 @@ ──────────────────────────── │
//! │ + src/new.rs        +40    │  10  10   use std::fmt;                           │
//! │ − old.rs              −6   │  11     - fn old() {}                              │
//! │                             │      11 + fn new() {}                              │
//! ╰──────────── s git/session · n/p file · ]/[ hunk · j/k scroll · r reload · esc ─╯
//! ```

use acc_core::item::{FileChangeKind, FileDiff, LineTag};

use crate::config::Theme;
use crate::diff::{DiffSource, DiffView, GitState};
use crate::model::App;
use crate::style::{Line, Span, Style};
use crate::view::{OverlayView, Rect};
use crate::wrap::{truncate, truncate_path, width, wrap_hang};

/// The file list's width, the diff's width and the diff's rows for a
/// screen of `w` × `h`. Narrow screens get no list.
pub fn layout(w: u16, h: u16) -> (usize, usize, usize) {
    // The overlay covers everything but the status bar.
    let inner = Rect::new(0, 0, w, h.saturating_sub(1)).inner();
    let iw = inner.w as usize;
    let list = if iw >= 90 { (iw / 3).min(44) } else { 0 };
    let body = iw.saturating_sub(if list > 0 { list + 3 } else { 0 });
    (list, body, (inner.h as usize).saturating_sub(2))
}

/// One file's diff, wrapped to `w`, with where each hunk starts.
#[derive(Debug, Default)]
pub struct Body {
    pub lines: Vec<Line>,
    pub hunks: Vec<usize>,
}

pub fn body(f: &FileDiff, theme: &Theme, w: usize) -> Body {
    let muted = Style::new().fg(theme.muted);
    let mut out = Body::default();
    if f.hunks.iter().all(|h| h.lines.is_empty()) {
        out.lines.push(Line::styled(
            match f.kind {
                FileChangeKind::Rename => "renamed, content unchanged",
                FileChangeKind::Delete => "deleted",
                _ => "binary, or no textual change",
            },
            muted,
        ));
        return out;
    }
    let nw = f
        .hunks
        .iter()
        .map(|h| (h.old_start + h.old_lines).max(h.new_start + h.new_lines))
        .max()
        .unwrap_or(0)
        .to_string()
        .len();
    let hang = Line::raw(" ".repeat(2 * nw + 4));
    for (i, h) in f.hunks.iter().enumerate() {
        if i > 0 {
            out.lines.push(Line::default());
        }
        out.hunks.push(out.lines.len());
        let positioned = h.old_start > 0 || h.new_start > 0;
        let label = if positioned {
            format!(
                " @@ -{},{} +{},{} @@ ",
                h.old_start, h.old_lines, h.new_start, h.new_lines
            )
        } else {
            " edit ".to_owned()
        };
        out.lines.push(Line::new(vec![
            Span::new("──", muted),
            Span::new(label.clone(), Style::new().fg(theme.accent)),
            Span::new("─".repeat(w.saturating_sub(width(&label) + 2)), muted),
        ]));
        let (mut old, mut new) = (h.old_start, h.new_start);
        for l in &h.lines {
            let num = |n: u32, shown: bool| {
                if positioned && shown {
                    format!("{n:>nw$}")
                } else {
                    " ".repeat(nw)
                }
            };
            let (o, n) = (
                num(old, l.tag != LineTag::Add),
                num(new, l.tag != LineTag::Remove),
            );
            let (sign, style) = match l.tag {
                LineTag::Context => (' ', Style::new()),
                LineTag::Add => ('+', Style::new().fg(theme.add)),
                LineTag::Remove => ('-', Style::new().fg(theme.remove)),
            };
            match l.tag {
                LineTag::Context => {
                    old += 1;
                    new += 1;
                }
                LineTag::Add => new += 1,
                LineTag::Remove => old += 1,
            }
            let line = Line::new(vec![
                Span::new(format!("{o} {n} "), muted),
                Span::new(format!("{sign} "), style.bold()),
                Span::new(l.text.replace('\t', "    "), style),
            ]);
            out.lines.extend(wrap_hang(&line, w, &hang));
        }
    }
    out
}

fn kind_glyph(k: FileChangeKind, theme: &Theme) -> Span {
    match k {
        FileChangeKind::Add => Span::new("+ ", Style::new().fg(theme.add).bold()),
        FileChangeKind::Delete => Span::new("− ", Style::new().fg(theme.remove).bold()),
        FileChangeKind::Update => Span::new("± ", Style::new().fg(theme.tool).bold()),
        FileChangeKind::Rename => Span::new("→ ", Style::new().fg(theme.tool).bold()),
    }
}

fn counts(added: u32, removed: u32, theme: &Theme) -> Vec<Span> {
    let mut out = Vec::new();
    if added > 0 {
        out.push(Span::new(format!("+{added}"), Style::new().fg(theme.add)));
    }
    if removed > 0 {
        if added > 0 {
            out.push(Span::raw(" "));
        }
        out.push(Span::new(format!("−{removed}"), Style::new().fg(theme.remove)));
    }
    out
}

/// A path relative to the session's directory when it is inside it.
fn rel(path: &str, cwd: &str) -> String {
    let cwd = cwd.trim_end_matches('/');
    path.strip_prefix(&format!("{cwd}/")).unwrap_or(path).to_owned()
}

/// `left` and `right` on one line of `w` columns, `right` flush right.
fn spread(left: Line, right: Vec<Span>, w: usize) -> Line {
    let rw: usize = right.iter().map(|s| width(&s.text)).sum();
    let mut l = clip(left, w.saturating_sub(rw + 1));
    l.push(Span::raw(" ".repeat(w.saturating_sub(l.width() + rw))));
    l.spans.extend(right);
    l
}

/// `line` cut to `w` columns.
fn clip(line: Line, w: usize) -> Line {
    let mut out = Line::default();
    let mut used = 0;
    for s in line.spans {
        if used >= w {
            break;
        }
        let t = truncate(&s.text, w - used);
        used += width(&t);
        out.push(Span::new(t, s.style));
    }
    out
}

pub fn build(app: &App, view: &DiffView, area: Rect) -> OverlayView {
    let theme = &app.theme;
    let muted = Style::new().fg(theme.muted);
    let (list_w, body_w, rows) = layout(area.w, area.h + 1);
    let session = app
        .slots
        .iter()
        .find(|s| s.key == view.key)
        .and_then(|s| s.session.as_ref());
    let files = view.files(session);
    let file = files.get(view.file);

    // Which source, as tabs.
    let tab = |label: &str, on: bool| {
        if on {
            Span::new(
                format!(" {label} "),
                Style {
                    reversed: true,
                    ..Style::new().fg(theme.accent).bold()
                },
            )
        } else {
            Span::new(format!(" {label} "), muted)
        }
    };
    let tabs = Line::new(vec![
        tab("git", view.source == DiffSource::Git),
        Span::raw(" "),
        tab("session", view.source == DiffSource::Session),
    ]);

    // The diff's own rows: a header, a rule, then the window of the body.
    let mut right: Vec<Line> = Vec::new();
    let tabs_w = tabs.width() + 2;
    let head_w = if list_w == 0 {
        body_w.saturating_sub(tabs_w)
    } else {
        body_w
    };
    let head = match file {
        Some(f) => {
            let mut l = Line::new(vec![kind_glyph(f.kind, theme)]);
            if let Some(old) = &f.old_path {
                l.push(Span::new(format!("{} → ", rel(old, &view.cwd)), muted));
            }
            l.push(Span::new(rel(&f.path, &view.cwd), Style::new().bold()));
            let mut meta = counts(f.added, f.removed, theme);
            meta.push(Span::new(format!("  {}/{}", view.file + 1, files.len()), muted));
            spread(l, meta, head_w)
        }
        None => Line::default(),
    };
    let head = if list_w == 0 {
        // No list: the tabs go in front of the file.
        let mut l = tabs.clone();
        l.push(Span::raw("  "));
        l.spans.extend(head.spans);
        clip(l, body_w)
    } else {
        head
    };
    right.push(head);
    right.push(Line::styled("─".repeat(body_w), muted));
    let empty = |msg: &str, hint: &str| {
        vec![
            Line::default(),
            Line::styled(format!("  {msg}"), muted),
            Line::styled(format!("  {hint}"), muted),
        ]
    };
    match (view.source, &view.git, file) {
        (DiffSource::Git, GitState::Loading, _) => {
            right.extend(empty("reading git diff…", ""));
        }
        (DiffSource::Git, GitState::Failed(e), _) => {
            right.extend(empty(&format!("git: {e}"), "s  this session's edits instead"));
        }
        (DiffSource::Git, GitState::Ready(_), None) => {
            right.extend(empty("working tree clean", "s  this session's edits · r  reload"));
        }
        (DiffSource::Session, _, None) => {
            right.extend(empty(
                "no edits in this session yet",
                "s  the repository's changes",
            ));
        }
        (_, _, Some(f)) => {
            let b = body(f, theme, body_w);
            let max = b.lines.len().saturating_sub(rows);
            right.extend(b.lines.into_iter().skip(view.scroll.min(max)).take(rows));
        }
    }

    // The file list beside it.
    let mut lines = Vec::new();
    if list_w > 0 {
        let (a, r) = files
            .iter()
            .fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
        let mut left = vec![
            spread(tabs, counts(a, r, theme), list_w),
            Line::styled("─".repeat(list_w), muted),
        ];
        let start = view.file.saturating_sub(rows.saturating_sub(1));
        for (i, f) in files.iter().enumerate().skip(start).take(rows) {
            let chosen = i == view.file;
            let meta = counts(f.added, f.removed, theme);
            let meta_w: usize = meta.iter().map(|s| width(&s.text)).sum();
            let room = list_w.saturating_sub(meta_w + 5);
            let path = truncate_path(&rel(&f.path, &view.cwd), room);
            let name = if chosen {
                Style::new().fg(theme.accent).bold()
            } else {
                Style::new()
            };
            let l = Line::new(vec![
                Span::new(
                    if chosen { "❯" } else { " " },
                    Style::new().fg(theme.accent).bold(),
                ),
                kind_glyph(f.kind, theme),
                Span::new(path, name),
            ]);
            left.push(spread(l, meta, list_w));
        }
        for (i, r) in right.into_iter().enumerate() {
            let l = left.get(i).cloned().unwrap_or_default();
            let mut row = Line::default();
            let pad = list_w.saturating_sub(l.width());
            row.spans.extend(l.spans);
            row.push(Span::raw(" ".repeat(pad)));
            row.push(Span::new(if i == 1 { "─┼─" } else { " │ " }, muted));
            row.spans.extend(r.spans);
            lines.push(row);
        }
        for l in left.into_iter().skip(lines.len()) {
            let mut row = l.clone();
            row.push(Span::raw(" ".repeat(list_w.saturating_sub(l.width()))));
            row.push(Span::new(" │", muted));
            lines.push(row);
        }
    } else {
        lines = right;
    }
    OverlayView {
        rect: area,
        title: "diff".into(),
        lines,
        footer: Some(Line::styled(
            "s git/session · n/p file · ]/[ hunk · j/k scroll · r reload · esc",
            muted,
        )),
        cursor: None,
        tone: None,
    }
}
