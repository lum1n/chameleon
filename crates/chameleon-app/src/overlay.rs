//! Modal overlays: permission cards, the launch wizard, the palette,
//! option pickers, confirmations, diffs and help.

use acc_core::Harness;
use acc_core::command::{Command, OptionId};
use acc_core::event::ItemKind;
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
        }
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
            _ => {}
        }
    }
}

pub fn parse_harness(s: &str) -> Option<Harness> {
    let s = s.trim().to_ascii_lowercase();
    Harness::ALL
        .into_iter()
        .find(|h| h.id() == s || h.program() == s || h.display_name().to_ascii_lowercase() == s)
}

fn is_denial(id: &str, label: &str) -> bool {
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
                Some(key) => self.send(&key, Command::Invoke { name, args }),
                None => Vec::new(),
            },
            PaletteKind::Action(a) => self.action(a),
            PaletteKind::Hook(name) => self.run_hook(&name, None),
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
pub fn file_changes(detail: &ItemDetail) -> Option<&[acc_core::item::FileDiff]> {
    match detail {
        ItemDetail::FileChange { files } => Some(files),
        _ => None,
    }
}
