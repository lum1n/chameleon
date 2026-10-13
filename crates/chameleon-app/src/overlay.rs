//! Modal overlays: permission cards, the launch wizard, the palette,
//! option pickers, harness panels, confirmations, diffs and help.

use acc_core::Harness;
use acc_core::command::{Choice, Command, CommandEntry, CommandInput, Exec, OptionId};
use acc_core::event::{ItemKind, Panel, PanelAction};
use acc_core::item::ItemDetail;
use acc_core::session::{LaunchRequest, Origin, Permission};
use acc_core::state::BlockKind;

use crate::event::{Effect, ExecOutput, ExecTag, HostId};
use crate::keymap::{Action, Key, KeyCode};
use crate::model::App;
use crate::slot::SessionKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    Kill,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wizard {
    pub host: usize,
    pub harness: Harness,
    /// cwd, model, permission, prompt
    pub fields: [String; 4],
    /// 0 host, 1 harness, 2.. text fields
    pub field: usize,
    pub busy: bool,
    pub error: Option<String>,
    pub resume: Option<String>,
}

impl Wizard {
    pub const FIELDS: usize = 6;

    pub fn text_field(&self) -> Option<usize> {
        self.field.checked_sub(2).filter(|i| *i < 4)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteKind {
    Command(String),
    Action(Action),
    Hook(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub label: String,
    pub hint: String,
    pub kind: PaletteKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help {
        scroll: usize,
    },
    Permission {
        key: SessionKey,
        request_id: String,
        selected: usize,
        text: String,
        /// Answers to the questions already stepped through (see [`steps`]).
        answers: Vec<Vec<String>>,
        /// Options ticked on the current multiple-choice question.
        marked: Vec<usize>,
    },
    Launch(Wizard),
    Palette {
        query: String,
        selected: usize,
    },
    Options {
        key: SessionKey,
        row: usize,
        pending: Vec<usize>,
    },
    Confirm {
        key: SessionKey,
        action: ConfirmAction,
        title: String,
    },
    Diff {
        key: SessionKey,
        item: String,
        scroll: usize,
    },
    /// Picks the value for a palette entry that takes one (`/model`).
    Pick {
        key: SessionKey,
        command: String,
        choices: Vec<Choice>,
        current: Option<String>,
        selected: usize,
    },
    /// A harness dialog (`/mcp`, `/status`); its content lives in the slot
    /// and is replaced whenever the harness answers again.
    Panel {
        key: SessionKey,
        id: String,
        /// Index into [`panel_actions`].
        selected: usize,
        /// Text typed for an action that asks for it.
        input: Option<String>,
    },
    /// Main and the session's subagent runs, to pick which one to view.
    Agents {
        key: SessionKey,
        /// Index into [`agent_rows`].
        selected: usize,
    },
    /// The theme picker; moving previews, esc puts `original` back.
    Themes {
        /// Index into [`App::theme_choices`], after the terminal entry (0).
        selected: usize,
        original: Option<String>,
    },
}

impl Overlay {
    pub fn palette() -> Self {
        Overlay::Palette {
            query: String::new(),
            selected: 0,
        }
    }

    pub fn confirm_kill(key: SessionKey, title: String) -> Self {
        Overlay::Confirm {
            key,
            action: ConfirmAction::Kill,
            title,
        }
    }

    pub fn confirm_remove(key: SessionKey, title: String) -> Self {
        Overlay::Confirm {
            key,
            action: ConfirmAction::Remove,
            title,
        }
    }

    /// `deny` preselects the denying option (or the last one).
    pub fn permission(key: SessionKey, p: &Permission, deny: bool) -> Self {
        let selected = if deny {
            p.options
                .iter()
                .position(|o| is_denial(&o.id, &o.label))
                .unwrap_or(p.options.len().saturating_sub(1))
        } else {
            0
        };
        Overlay::Permission {
            key,
            request_id: p.request_id.clone(),
            selected,
            text: String::new(),
            answers: Vec::new(),
            marked: Vec::new(),
        }
    }

    pub fn themes(app: &App) -> Self {
        let original = app.theme_name().map(str::to_owned);
        let choices = app.theme_choices();
        let selected = original
            .as_deref()
            .and_then(|n| {
                let builtin = crate::themes::builtin();
                let id = crate::themes::find(n, &app.cfg.custom_themes, &builtin)?
                    .id
                    .clone();
                choices.iter().position(|t| t.id == id)
            })
            .map_or(0, |i| i + 1);
        Overlay::Themes { selected, original }
    }

    /// The agents picker on the conversation shown; `None` without subagents.
    pub fn agents(slot: &crate::slot::Slot) -> Option<Self> {
        let session = slot.session.as_ref()?;
        let rows = agent_rows(session);
        if rows.len() < 2 {
            return None;
        }
        let scope = slot.scope();
        let selected = rows.iter().position(|r| r.id.as_deref() == scope).unwrap_or(0);
        Some(Overlay::Agents {
            key: slot.key.clone(),
            selected,
        })
    }

    pub fn options(key: SessionKey) -> Self {
        Overlay::Options {
            key,
            row: 0,
            pending: Vec::new(),
        }
    }

    pub fn launch(app: &App) -> Self {
        let host = app
            .selected()
            .and_then(|s| app.hosts.iter().position(|h| h.cfg.name == s.key.host))
            .unwrap_or(0);
        let harness = app
            .cfg
            .general
            .default_harness
            .as_deref()
            .and_then(parse_harness)
            .or_else(|| app.selected().map(|s| s.harness()))
            .or_else(|| {
                app.hosts
                    .get(host)
                    .and_then(|h| h.harnesses.first().map(|a| a.harness))
            })
            .unwrap_or(Harness::Claude);
        let cwd = app
            .selected()
            .filter(|s| {
                s.key.host
                    == app
                        .hosts
                        .get(host)
                        .map(|h| h.cfg.name.clone())
                        .unwrap_or_default()
            })
            .map(|s| s.spec.cwd.clone())
            .unwrap_or_else(|| {
                if app.hosts.get(host).is_some_and(|h| h.cfg.is_local()) {
                    std::env::current_dir()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "~".into())
                } else {
                    "~".into()
                }
            });
        Overlay::Launch(Wizard {
            host,
            harness,
            fields: [cwd, String::new(), String::new(), String::new()],
            field: 2,
            busy: false,
            error: None,
            resume: None,
        })
    }

    /// A diff overlay for the cursor item, or the latest file change.
    pub fn diff_for(app: &App) -> Option<Self> {
        let slot = app.selected()?;
        let session = slot.session.as_ref()?;
        let items = session.transcript().items();
        let pick = slot
            .cursor
            .and_then(|c| items.get(c))
            .filter(|i| i.kind == ItemKind::FileChange)
            .or_else(|| items.iter().rev().find(|i| i.kind == ItemKind::FileChange))?;
        Some(Overlay::Diff {
            key: slot.key.clone(),
            item: pick.id.clone(),
            scroll: 0,
        })
    }

    pub fn paste(&mut self, text: &str) {
        match self {
            Overlay::Launch(w) => {
                if let Some(i) = w.text_field() {
                    w.fields[i].push_str(text);
                }
            }
            Overlay::Palette { query, .. } => query.push_str(text),
            Overlay::Permission { text: t, .. } => t.push_str(text),
            Overlay::Panel { input: Some(t), .. } => t.push_str(text),
            _ => {}
        }
    }
}

/// The questions a request is answered through one at a time: several
/// questions, or one taking several picks. Empty when its options suffice.
pub fn steps(p: &Permission) -> Vec<acc_core::question::Question> {
    if p.kind != BlockKind::Question {
        return Vec::new();
    }
    let qs = acc_core::question::parse(&p.input);
    if qs.len() > 1 || qs.iter().any(|q| q.multi_select) {
        qs
    } else {
        Vec::new()
    }
}

/// Permission modes the launch wizard offers as choices. Claude's mirror
/// acc's own list (`claude::PERMISSION_MODES`); other harnesses take a
/// free-form value (a Codex profile, say).
pub fn permission_modes(h: Harness) -> &'static [&'static str] {
    match h {
        Harness::Claude => &["default", "acceptEdits", "plan", "auto", "dontAsk"],
        _ => &[],
    }
}

pub fn parse_harness(s: &str) -> Option<Harness> {
    let s = s.trim().to_ascii_lowercase();
    Harness::ALL
        .into_iter()
        .find(|h| h.id() == s || h.program() == s || h.display_name().to_ascii_lowercase() == s)
}

/// A panel's buttons in display order: each row's, then the section's.
pub fn panel_actions(panel: &Panel) -> Vec<&PanelAction> {
    panel
        .sections
        .iter()
        .flat_map(|s| s.rows.iter().flat_map(|r| &r.actions).chain(&s.actions))
        .collect()
}

pub(crate) fn is_denial(id: &str, label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    let i = id.to_ascii_lowercase();
    ["deny", "no", "reject", "cancel", "decline", "block"]
        .iter()
        .any(|w| i == *w || l == *w || l.starts_with(&format!("{w} ")))
}

impl App {
    /// Palette entries for the selected session, unfiltered.
    pub(crate) fn palette_items(&self) -> Vec<PaletteItem> {
        let mut items = Vec::new();
        if let Some(s) = self.selected()
            && let Some(session) = &s.session
        {
            for c in session.commands() {
                let mut hint = c.description.clone();
                if let Some(a) = &c.arg_hint {
                    hint = format!("{hint} ⟨{a}⟩");
                }
                if c.exec == Exec::Terminal {
                    hint = format!("{hint} · opens {}", s.harness().display_name());
                }
                items.push(PaletteItem {
                    label: format!("/{}", c.name.trim_start_matches('/')),
                    hint,
                    kind: PaletteKind::Command(c.name.clone()),
                });
            }
        }
        for a in Action::ALL {
            if matches!(
                a,
                Action::Jump(_) | Action::SelectUp | Action::SelectDown | Action::Back
            ) {
                continue;
            }
            let keys: Vec<String> = self.keymap.keys_for(a).iter().map(ToString::to_string).collect();
            items.push(PaletteItem {
                label: a.describe().to_owned(),
                hint: keys.join(" "),
                kind: PaletteKind::Action(a),
            });
        }
        for h in &self.cfg.hooks {
            items.push(PaletteItem {
                label: format!("hook: {}", h.name),
                hint: h.key.clone().unwrap_or_default(),
                kind: PaletteKind::Hook(h.name.clone()),
            });
        }
        items
    }

    /// Entries matching the palette query (its first word).
    pub(crate) fn palette_matches(&self, query: &str) -> Vec<PaletteItem> {
        let word = query
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches('/')
            .to_ascii_lowercase();
        let all = self.palette_items();
        if word.is_empty() {
            return all;
        }
        let mut scored: Vec<(usize, PaletteItem)> = all
            .into_iter()
            .filter_map(|it| {
                let l = it.label.trim_start_matches('/').to_ascii_lowercase();
                if l == word {
                    Some((0, it))
                } else if l.starts_with(&word) {
                    Some((1, it))
                } else if l.contains(&word) {
                    Some((2, it))
                } else if subsequence(&word, &l) {
                    Some((3, it))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by_key(|(s, _)| *s);
        scored.into_iter().map(|(_, it)| it).collect()
    }

    pub(crate) fn overlay_key(&mut self, key: Key) -> Vec<Effect> {
        let Some(mut overlay) = self.overlay.take() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut keep = true;
        match &mut overlay {
            Overlay::Help { scroll } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter | KeyCode::F(1) => {
                    keep = false
                }
                KeyCode::Char('j') | KeyCode::Down => *scroll += 1,
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                _ => {}
            },
            Overlay::Confirm {
                key: skey, action, ..
            } => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    keep = false;
                    let skey = skey.clone();
                    out = match action {
                        ConfirmAction::Kill => self.kill(&skey),
                        ConfirmAction::Remove => self.remove(&skey),
                    };
                }
                _ => keep = false,
            },
            Overlay::Permission {
                key: skey,
                request_id,
                selected,
                text,
                answers,
                marked,
            } => {
                let perm = self
                    .slot_mut(skey)
                    .and_then(|s| s.session.as_ref())
                    .and_then(|s| {
                        s.permissions()
                            .iter()
                            .find(|p| &p.request_id == request_id)
                            .cloned()
                    });
                let Some(perm) = perm else {
                    self.toast("the request was already answered", false);
                    return Vec::new();
                };
                let steps = steps(&perm);
                if let Some(q) = steps.get(answers.len()) {
                    let labels: Vec<String> = q.options.iter().map(|o| o.label.clone()).collect();
                    let n = labels.len().max(1);
                    let mut done = None;
                    match key.code {
                        KeyCode::Esc => keep = false,
                        KeyCode::Down | KeyCode::Tab => *selected = (*selected + 1) % n,
                        KeyCode::Up | KeyCode::BackTab => *selected = (*selected + n - 1) % n,
                        KeyCode::Char('j') if text.is_empty() => *selected = (*selected + 1) % n,
                        KeyCode::Char('k') if text.is_empty() => *selected = (*selected + n - 1) % n,
                        KeyCode::Char(' ') if text.is_empty() && q.multi_select => toggle(marked, *selected),
                        KeyCode::Char(c) if c.is_ascii_digit() && text.is_empty() && !key.ctrl => {
                            let i = c.to_digit(10).unwrap_or(0) as usize;
                            if i >= 1 && i <= labels.len() {
                                *selected = i - 1;
                                if q.multi_select {
                                    toggle(marked, i - 1);
                                } else {
                                    done = Some(vec![labels[i - 1].clone()]);
                                }
                            }
                        }
                        KeyCode::Enter => {
                            done = if !text.trim().is_empty() {
                                Some(vec![text.trim().to_owned()])
                            } else if q.multi_select && !marked.is_empty() {
                                marked.sort_unstable();
                                Some(marked.iter().filter_map(|&i| labels.get(i).cloned()).collect())
                            } else {
                                labels.get(*selected).map(|l| vec![l.clone()])
                            };
                        }
                        // Back to the previous question.
                        KeyCode::Backspace if text.is_empty() => {
                            answers.pop();
                            marked.clear();
                            *selected = 0;
                        }
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char('u') if key.ctrl => text.clear(),
                        KeyCode::Char(c) if !key.ctrl && !key.alt => text.push(c),
                        _ => {}
                    }
                    if let Some(a) = done {
                        answers.push(a);
                        text.clear();
                        marked.clear();
                        *selected = 0;
                        if answers.len() == steps.len() {
                            keep = false;
                            let encoded = acc_core::question::encode(answers);
                            out = self.answer(skey, request_id, &encoded);
                        }
                    }
                    if keep && self.overlay.is_none() {
                        self.overlay = Some(overlay);
                    }
                    return out;
                }
                let n = perm.options.len();
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Down | KeyCode::Tab => *selected = (*selected + 1) % n.max(1),
                    KeyCode::Up | KeyCode::BackTab => *selected = (*selected + n.max(1) - 1) % n.max(1),
                    KeyCode::Char('j') if text.is_empty() => *selected = (*selected + 1) % n.max(1),
                    KeyCode::Char('k') if text.is_empty() => {
                        *selected = (*selected + n.max(1) - 1) % n.max(1)
                    }
                    KeyCode::Char('y') if text.is_empty() && perm.kind != BlockKind::Question => {
                        keep = false;
                        if let Some(o) = perm.options.first() {
                            out = self.answer(skey, request_id, &o.id);
                        }
                    }
                    KeyCode::Char('n') if text.is_empty() && perm.kind != BlockKind::Question => {
                        keep = false;
                        if let Some(o) = perm
                            .options
                            .iter()
                            .find(|o| is_denial(&o.id, &o.label))
                            .or(perm.options.last())
                        {
                            out = self.answer(skey, request_id, &o.id);
                        }
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() && text.is_empty() && !key.ctrl => {
                        let i = c.to_digit(10).unwrap_or(0) as usize;
                        if i >= 1 && i <= n {
                            keep = false;
                            out = self.answer(skey, request_id, &perm.options[i - 1].id.clone());
                        }
                    }
                    KeyCode::Enter => {
                        keep = false;
                        let answer = if !text.trim().is_empty() {
                            text.trim().to_owned()
                        } else {
                            perm.options
                                .get(*selected)
                                .map(|o| o.id.clone())
                                .unwrap_or_default()
                        };
                        if answer.is_empty() {
                            keep = true;
                        } else {
                            out = self.answer(skey, request_id, &answer);
                        }
                    }
                    KeyCode::Backspace => {
                        text.pop();
                    }
                    KeyCode::Char('u') if key.ctrl => text.clear(),
                    KeyCode::Char(c) if perm.kind == BlockKind::Question && !key.ctrl && !key.alt => {
                        text.push(c)
                    }
                    _ => {}
                }
            }
            Overlay::Launch(w) => {
                if w.busy {
                    if key.code == KeyCode::Esc {
                        keep = false;
                    }
                } else {
                    match key.code {
                        KeyCode::Esc => keep = false,
                        KeyCode::Tab | KeyCode::Down => w.field = (w.field + 1) % Wizard::FIELDS,
                        KeyCode::BackTab | KeyCode::Up => {
                            w.field = (w.field + Wizard::FIELDS - 1) % Wizard::FIELDS
                        }
                        KeyCode::Enter if key.ctrl || w.field == Wizard::FIELDS - 1 => {
                            if key.alt || key.shift {
                                w.fields[3].push('\n');
                            } else {
                                out = self.wizard_submit(w);
                            }
                        }
                        KeyCode::Enter => w.field = (w.field + 1) % Wizard::FIELDS,
                        KeyCode::Left | KeyCode::Right if w.field == 0 => {
                            let n = self.hosts.len().max(1);
                            w.host = if key.code == KeyCode::Right {
                                (w.host + 1) % n
                            } else {
                                (w.host + n - 1) % n
                            };
                        }
                        KeyCode::Left | KeyCode::Right if w.field == 1 => {
                            let list = self.wizard_harnesses(w.host);
                            let i = list.iter().position(|h| *h == w.harness).unwrap_or(0);
                            let n = list.len().max(1);
                            let j = if key.code == KeyCode::Right {
                                (i + 1) % n
                            } else {
                                (i + n - 1) % n
                            };
                            if let Some(h) = list.get(j) {
                                w.harness = *h;
                            }
                        }
                        KeyCode::Left | KeyCode::Right
                            if w.field == 4 && !permission_modes(w.harness).is_empty() =>
                        {
                            let modes = permission_modes(w.harness);
                            let n = modes.len();
                            let next = match modes.iter().position(|m| *m == w.fields[2]) {
                                None if key.code == KeyCode::Right => 0,
                                None => n - 1,
                                Some(i) if key.code == KeyCode::Right => (i + 1) % n,
                                Some(i) => (i + n - 1) % n,
                            };
                            w.fields[2] = modes[next].to_owned();
                        }
                        KeyCode::Char(' ') if w.field == 1 => {
                            let list = self.wizard_harnesses(w.host);
                            let i = list.iter().position(|h| *h == w.harness).unwrap_or(0);
                            if let Some(h) = list.get((i + 1) % list.len().max(1)) {
                                w.harness = *h;
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(i) = w.text_field() {
                                if key.ctrl || key.alt {
                                    let t = w.fields[i].trim_end().to_owned();
                                    let cut = t.rfind(' ').map(|p| p + 1).unwrap_or(0);
                                    w.fields[i].truncate(cut);
                                } else {
                                    w.fields[i].pop();
                                }
                            }
                        }
                        KeyCode::Char('u') if key.ctrl => {
                            if let Some(i) = w.text_field() {
                                w.fields[i].clear();
                            }
                        }
                        KeyCode::Char(c) if !key.ctrl && !key.alt => {
                            if let Some(i) = w.text_field() {
                                w.fields[i].push(c);
                            } else if w.field == 1
                                && let Some(h) = self.wizard_harnesses(w.host).into_iter().find(|h| {
                                    h.display_name()
                                        .to_ascii_lowercase()
                                        .starts_with(c.to_ascii_lowercase())
                                })
                            {
                                w.harness = h;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Overlay::Palette { query, selected } => {
                let matches = self.palette_matches(query);
                match key.code {
                    KeyCode::Esc => keep = false,
                    KeyCode::Down | KeyCode::Tab => *selected = (*selected + 1) % matches.len().max(1),
                    KeyCode::Up | KeyCode::BackTab => {
                        *selected = (*selected + matches.len().max(1) - 1) % matches.len().max(1)
                    }
                    KeyCode::Char('n') if key.ctrl => *selected = (*selected + 1) % matches.len().max(1),
                    KeyCode::Char('p') if key.ctrl => {
                        *selected = (*selected + matches.len().max(1) - 1) % matches.len().max(1)
                    }
                    KeyCode::Enter => {
                        keep = false;
                        if let Some(it) = matches.get(*selected).cloned() {
                            let args = query
                                .split_once(char::is_whitespace)
                                .map(|(_, a)| a.trim().to_owned());
                            out = self.palette_run(it, args.unwrap_or_default());
                        }
                    }
                    KeyCode::Backspace => {
                        if query.pop().is_none() {
                            keep = false;
                        }
                        *selected = 0;
                    }
                    KeyCode::Char('u') if key.ctrl => {
                        query.clear();
                        *selected = 0;
                    }
                    KeyCode::Char(c) if !key.ctrl && !key.alt => {
                        query.push(c);
                        *selected = 0;
                    }
                    _ => {}
                }
            }
            Overlay::Options {
                key: skey,
                row,
                pending,
            } => {
                let defs = self
                    .slot_mut(skey)
                    .and_then(|s| s.session.as_ref())
                    .map(|s| s.options().to_vec())
                    .unwrap_or_default();
                if pending.len() != defs.len() {
                    *pending = defs
                        .iter()
                        .map(|d| {
                            d.current
                                .as_ref()
                                .and_then(|c| d.values.iter().position(|v| v == c))
                                .unwrap_or(0)
                        })
                        .collect();
                }
                let n = defs.len().max(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => keep = false,
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => *row = (*row + 1) % n,
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => *row = (*row + n - 1) % n,
                    KeyCode::Left | KeyCode::Char('h') => {
                        if let (Some(d), Some(p)) = (defs.get(*row), pending.get_mut(*row)) {
                            let m = d.values.len().max(1);
                            *p = (*p + m - 1) % m;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
                        if let (Some(d), Some(p)) = (defs.get(*row), pending.get_mut(*row)) {
                            let m = d.values.len().max(1);
                            *p = (*p + 1) % m;
                        }
                    }
                    KeyCode::Enter => {
                        keep = false;
                        let skey = skey.clone();
                        for (i, d) in defs.iter().enumerate() {
                            let Some(v) = pending.get(i).and_then(|p| d.values.get(*p)) else {
                                continue;
                            };
                            if d.current.as_ref() != Some(v) {
                                out.extend(self.send(
                                    &skey,
                                    Command::SetOption {
                                        id: d.id,
                                        value: v.clone(),
                                    },
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Overlay::Pick {
                key: skey,
                command,
                choices,
                selected,
                ..
            } => {
                let n = choices.len().max(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => keep = false,
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => *selected = (*selected + 1) % n,
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                        *selected = (*selected + n - 1) % n
                    }
                    KeyCode::Enter => {
                        keep = false;
                        if let Some(c) = choices.get(*selected) {
                            let (skey, name, args) = (skey.clone(), command.clone(), c.value.clone());
                            out = self.send(&skey, Command::Invoke { name, args });
                        }
                    }
                    _ => {}
                }
            }
            Overlay::Panel {
                key: skey,
                id,
                selected,
                input,
            } => {
                let actions: Vec<PanelAction> = self
                    .slot_mut(skey)
                    .and_then(|s| s.panels.get(id.as_str()))
                    .map(|p| panel_actions(p).into_iter().cloned().collect())
                    .unwrap_or_default();
                let n = actions.len().max(1);
                if let Some(text) = input {
                    match key.code {
                        KeyCode::Esc => *input = None,
                        KeyCode::Enter => {
                            if let Some(a) = actions.get(*selected) {
                                let args = format!("{} {}", a.args, text.trim()).trim().to_owned();
                                let (skey, name) = (skey.clone(), a.command.clone());
                                *input = None;
                                out = self.send(&skey, Command::Invoke { name, args });
                            }
                        }
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char('u') if key.ctrl => text.clear(),
                        KeyCode::Char(c) if !key.ctrl && !key.alt => text.push(c),
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => keep = false,
                        KeyCode::Down
                        | KeyCode::Char('j')
                        | KeyCode::Tab
                        | KeyCode::Right
                        | KeyCode::Char('l') => *selected = (*selected + 1) % n,
                        KeyCode::Up
                        | KeyCode::Char('k')
                        | KeyCode::BackTab
                        | KeyCode::Left
                        | KeyCode::Char('h') => *selected = (*selected + n - 1) % n,
                        KeyCode::Enter => {
                            if let Some(a) = actions.get(*selected) {
                                if a.prompt.is_some() {
                                    *input = Some(String::new());
                                } else {
                                    // The panel stays open; the harness answers with a fresh one.
                                    let (skey, name, args) =
                                        (skey.clone(), a.command.clone(), a.args.clone());
                                    out = self.send(&skey, Command::Invoke { name, args });
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Overlay::Themes { selected, original } => {
                let n = self.theme_choices().len() + 1;
                let before = *selected;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        keep = false;
                        let original = original.clone();
                        self.apply_theme(original.as_deref());
                    }
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => *selected = (*selected + 1) % n,
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                        *selected = (*selected + n - 1) % n
                    }
                    KeyCode::PageDown => *selected = (*selected + 10).min(n - 1),
                    KeyCode::PageUp => *selected = selected.saturating_sub(10),
                    KeyCode::Home | KeyCode::Char('g') => *selected = 0,
                    KeyCode::End | KeyCode::Char('G') => *selected = n - 1,
                    KeyCode::Enter => {
                        keep = false;
                        let name = self.theme_name().map(str::to_owned);
                        self.toast(format!("theme: {}", name.as_deref().unwrap_or("terminal")), false);
                        out.push(Effect::SaveTheme { name });
                    }
                    _ => {}
                }
                if keep && *selected != before {
                    let name = selected
                        .checked_sub(1)
                        .and_then(|i| self.theme_choices().get(i).map(|t| t.id.clone()));
                    self.apply_theme(name.as_deref());
                }
            }
            Overlay::Agents { key: skey, selected } => {
                let rows = self
                    .slot_mut(skey)
                    .and_then(|s| s.session.as_ref().map(agent_rows))
                    .unwrap_or_default();
                let n = rows.len().max(1);
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('a') => keep = false,
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => *selected = (*selected + 1) % n,
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                        *selected = (*selected + n - 1) % n
                    }
                    KeyCode::Home | KeyCode::Char('g') => *selected = 0,
                    KeyCode::End | KeyCode::Char('G') => *selected = n - 1,
                    KeyCode::Enter => {
                        keep = false;
                        let id = rows.get(*selected).and_then(|r| r.id.clone());
                        let skey = skey.clone();
                        if let Some(s) = self.slot_mut(&skey) {
                            s.go_to_agent(id.as_deref());
                        }
                        self.focus = crate::model::Pane::Transcript;
                    }
                    _ => {}
                }
            }
            Overlay::Diff { scroll, .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('d') => keep = false,
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll += 20,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
                KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                _ => {}
            },
        }
        if keep && self.overlay.is_none() {
            self.overlay = Some(overlay);
        }
        out
    }

    fn answer(&mut self, key: &SessionKey, request_id: &str, option_id: &str) -> Vec<Effect> {
        self.send(
            key,
            Command::AnswerPermission {
                request_id: request_id.to_owned(),
                option_id: option_id.to_owned(),
            },
        )
    }

    pub(crate) fn wizard_harnesses(&self, host: usize) -> Vec<Harness> {
        let probed: Vec<Harness> = self
            .hosts
            .get(host)
            .map(|h| h.harnesses.iter().map(|a| a.harness).collect())
            .unwrap_or_default();
        if probed.is_empty() {
            Harness::ALL.to_vec()
        } else {
            probed
        }
    }

    fn wizard_submit(&mut self, w: &mut Wizard) -> Vec<Effect> {
        let Some(host) = self.hosts.get(w.host).map(|h| h.cfg.name.clone()) else {
            return Vec::new();
        };
        let cwd = w.fields[0].trim();
        let cwd = if cwd.is_empty() { "~" } else { cwd };
        w.busy = true;
        w.error = None;
        vec![self.run(
            &host,
            acc_core::Exec::new(acc_core::session::resolve_dir_script(cwd)),
            ExecTag::ResolveDir,
        )]
    }

    pub(crate) fn on_resolve_dir(
        &mut self,
        host: &HostId,
        outcome: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let Some(Overlay::Launch(w)) = self.overlay.take() else {
            return Vec::new();
        };
        let mut w = w;
        w.busy = false;
        match outcome {
            Ok(o) => {
                let cwd = o.stdout_text().trim().to_owned();
                if cwd.is_empty() {
                    w.error = Some("the directory doesn't exist".into());
                    self.overlay = Some(Overlay::Launch(w));
                    return Vec::new();
                }
                let opt = |s: &String| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
                let req = LaunchRequest {
                    harness: Some(w.harness),
                    cwd,
                    model: opt(&w.fields[1]),
                    permission: opt(&w.fields[2]),
                    resume: w.resume.clone(),
                    fork: false,
                    title: None,
                    prompt: opt(&w.fields[3]),
                    attachments: Vec::new(),
                    parent: None,
                    replaces_parent: false,
                    origin: Origin::Started,
                    launcher: None,
                };
                self.focus = crate::model::Pane::Composer;
                self.launch(host, req)
            }
            Err(e) => {
                w.error = Some(format!("no such directory: {e}"));
                self.overlay = Some(Overlay::Launch(w));
                Vec::new()
            }
        }
    }

    fn palette_run(&mut self, item: PaletteItem, args: String) -> Vec<Effect> {
        match item.kind {
            PaletteKind::Command(name) => match self.selected.clone() {
                Some(key) => self.run_command(&key, name, args),
                None => Vec::new(),
            },
            PaletteKind::Action(a) => self.action(a),
            PaletteKind::Hook(name) => self.run_hook(&name, None),
        }
    }
}

impl App {
    fn command_entry(&self, key: &SessionKey, name: &str) -> Option<CommandEntry> {
        let slot = self.slots.iter().find(|s| &s.key == key)?;
        slot.session
            .as_ref()?
            .commands()
            .iter()
            .find(|c| c.name == name)
            .cloned()
    }

    /// Runs a palette entry: entries that take a value open a picker when
    /// none was typed, terminal-only ones open the harness's own TUI.
    pub(crate) fn run_command(&mut self, key: &SessionKey, name: String, args: String) -> Vec<Effect> {
        let entry = self.command_entry(key, &name);
        if entry.as_ref().is_some_and(|e| e.exec == Exec::Terminal) {
            return self.open_terminal(key);
        }
        if args.trim().is_empty()
            && let Some(picker) = self.picker(key, &name)
        {
            self.overlay = Some(picker);
            return Vec::new();
        }
        self.send(key, Command::Invoke { name, args })
    }

    /// A picker for palette entry `name`, when it takes a known set of values.
    pub(crate) fn picker(&self, key: &SessionKey, name: &str) -> Option<Overlay> {
        let entry = self.command_entry(key, name)?;
        let (choices, current) = match &entry.input {
            CommandInput::None => return None,
            CommandInput::Option { id } => {
                let session = self.slots.iter().find(|s| &s.key == key)?.session.as_ref()?;
                let def = session.options().iter().find(|o| o.id == *id)?;
                (
                    def.values.iter().map(Choice::new).collect::<Vec<_>>(),
                    def.current.clone(),
                )
            }
            CommandInput::Choice { choices, required } => {
                let mut list = choices.clone();
                if !required {
                    list.insert(
                        0,
                        Choice {
                            value: String::new(),
                            label: "(none)".into(),
                            description: Some("run it without a value".into()),
                        },
                    );
                }
                (list, None)
            }
        };
        if choices.is_empty() {
            return None;
        }
        let selected = current
            .as_ref()
            .and_then(|c| choices.iter().position(|v| &v.value == c))
            .unwrap_or(0);
        Some(Overlay::Pick {
            key: key.clone(),
            command: name.to_owned(),
            choices,
            current,
            selected,
        })
    }

    /// Hands the terminal to the harness's own TUI in the session's directory.
    pub(crate) fn open_terminal(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(slot) = self.slots.iter().find(|s| &s.key == key) else {
            return Vec::new();
        };
        let harness = slot.harness();
        let program = match acc_core::multiplexer::tui::launch(harness, None) {
            Ok(p) => p,
            Err(e) => {
                self.toast(e.to_string(), true);
                return Vec::new();
            }
        };
        let argv = program.argv();
        let line = format!(
            "cd {} && exec {}",
            acc_core::shell::quote(&slot.spec.cwd),
            acc_core::shell::join(argv.iter().map(String::as_str))
        );
        let argv = match self
            .host(&key.host)
            .and_then(|h| h.cfg.ssh.clone().map(|d| (d, h.cfg.ssh_args.clone())))
        {
            Some((dest, ssh_args)) => {
                let mut v = vec!["ssh".to_owned(), "-t".to_owned()];
                v.extend(ssh_args);
                v.extend(["--".to_owned(), dest, line]);
                v
            }
            None => vec!["sh".to_owned(), "-c".to_owned(), line],
        };
        vec![Effect::Terminal { argv }]
    }

    /// Shows a panel the harness answered with, unless another dialog is open.
    pub(crate) fn show_panel(&mut self, key: &SessionKey, id: &str) {
        if self.selected.as_ref() != Some(key) {
            return;
        }
        let actions = self
            .slots
            .iter()
            .find(|s| &s.key == key)
            .and_then(|s| s.panels.get(id))
            .map_or(0, |p| panel_actions(p).len());
        match &mut self.overlay {
            None => {
                self.overlay = Some(Overlay::Panel {
                    key: key.clone(),
                    id: id.to_owned(),
                    selected: 0,
                    input: None,
                })
            }
            Some(Overlay::Panel {
                key: k,
                id: open,
                selected,
                ..
            }) if k == key => {
                if open != id {
                    *open = id.to_owned();
                    *selected = 0;
                }
                // Keep the cursor inside a panel that lost actions.
                *selected = (*selected).min(actions.saturating_sub(1));
            }
            Some(_) => {}
        }
    }
}

fn subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// Human label for an option id.
pub fn option_label(id: OptionId) -> &'static str {
    match id {
        OptionId::Model => "model",
        OptionId::Effort => "effort",
        OptionId::Mode => "mode",
        OptionId::Permission => "permission",
    }
}

/// The file changes of an item, for the diff overlay.
/// One line of the agents picker: main (`id: None`) or a subagent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub id: Option<String>,
    /// Subagents a run sits inside of.
    pub depth: usize,
    pub name: String,
    pub description: String,
    pub status: Option<acc_core::transcript::ItemStatus>,
    /// Items in its conversation.
    pub items: usize,
}

/// Every subagent run in the session, in the order they started.
pub fn agent_ids(session: &acc_core::session::Session) -> Vec<String> {
    session.transcript().subagents().map(|it| it.id.clone()).collect()
}

/// Main, then every subagent run, nested under the one it ran in.
pub fn agent_rows(session: &acc_core::session::Session) -> Vec<AgentRow> {
    let t = session.transcript();
    let harness = session.spec().harness;
    let count = |id: Option<&str>| t.items().iter().filter(|it| it.parent.as_deref() == id).count();
    let mut rows = vec![AgentRow {
        id: None,
        depth: 0,
        name: "main".into(),
        description: session.spec().title.clone().unwrap_or_default(),
        status: None,
        items: count(None),
    }];
    for item in t.subagents() {
        let mut depth = 1;
        let mut at = item.parent.as_deref();
        while let Some(p) = at.and_then(|p| t.get(p)) {
            depth += 1;
            at = p.parent.as_deref();
        }
        let description = match item.detail(harness) {
            ItemDetail::Subagent { description, .. } => description,
            _ => String::new(),
        };
        rows.push(AgentRow {
            id: Some(item.id.clone()),
            depth,
            name: crate::view::transcript::agent_name(item, harness),
            description,
            status: Some(item.status),
            items: count(Some(&item.id)),
        });
    }
    rows
}

pub fn file_changes(detail: &ItemDetail) -> Option<&[acc_core::item::FileDiff]> {
    match detail {
        ItemDetail::FileChange { files } => Some(files),
        _ => None,
    }
}

/// Ticks or unticks option `i`.
fn toggle(marked: &mut Vec<usize>, i: usize) {
    if let Some(at) = marked.iter().position(|&m| m == i) {
        marked.remove(at);
    } else {
        marked.push(i);
    }
}
