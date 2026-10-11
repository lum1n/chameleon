//! The app: one state machine fed by [`AppEvent`]s, answering with
//! [`Effect`]s. It owns every `Session`; nothing here does I/O.

use std::collections::VecDeque;

use acc_core::command::Command;
use acc_core::harness::Availability;

use crate::composer::Composer;
use crate::config::{Config, HostConfig, Theme};
use crate::event::{AppEvent, Effect, ExecTag, HostId, MouseKind};
use crate::keymap::{Action, Key, KeyCode, Keymap};
use crate::overlay::Overlay;
use crate::slot::{Phase, SessionKey, Slot};
use crate::style::{Line, Style};
use crate::view::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedStatus {
    Unknown,
    Up,
    Down(String),
}

#[derive(Debug)]
pub struct HostState {
    pub cfg: HostConfig,
    pub feed: FeedStatus,
    pub harnesses: Vec<Availability>,
    pub probed: bool,
    pub listed_at_ms: Option<u64>,
    pub listing: bool,
    pub error: Option<String>,
}

impl HostState {
    pub fn id(&self) -> &str {
        &self.cfg.name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    Transcript,
    Composer,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub until_ms: u64,
    pub line: Line,
}

type Entropy = Box<dyn FnMut() -> [u8; 15] + Send>;

pub struct App {
    pub(crate) cfg: Config,
    pub(crate) theme: Theme,
    pub(crate) keymap: Keymap,
    pub(crate) hosts: Vec<HostState>,
    pub(crate) slots: Vec<Slot>,
    pub(crate) selected: Option<SessionKey>,
    pub(crate) focus: Pane,
    pub(crate) composer: Composer,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) toasts: VecDeque<Toast>,
    pub(crate) size: (u16, u16),
    pub(crate) client_tag: String,
    pub(crate) entropy: Entropy,
    pub(crate) dirty: bool,
    pub(crate) now_ms: u64,
    pub(crate) problems: Vec<String>,
    /// Where the last frame put things, for mouse clicks.
    pub(crate) layout: Layout,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub sidebar: Option<Rect>,
    pub sidebar_hits: Vec<Option<SessionKey>>,
    pub transcript: Rect,
    pub composer: Rect,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("hosts", &self.hosts.len())
            .field("slots", &self.slots.len())
            .field("selected", &self.selected)
            .field("focus", &self.focus)
            .finish_non_exhaustive()
    }
}

fn urandom() -> [u8; 15] {
    use std::io::Read as _;
    let mut buf = [0u8; 15];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(&mut buf).is_ok()
    {
        return buf;
    }
    // Last resort: time and address entropy, still unique enough for an id.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::process::id() as u128;
    let mix = t ^ (p << 64) ^ (&buf as *const _ as u128);
    buf.copy_from_slice(&mix.to_le_bytes()[..15]);
    buf
}

impl App {
    pub fn new(cfg: Config, client_tag: impl Into<String>) -> Self {
        let mut problems = Vec::new();
        let theme = Theme::from_config(&cfg.theme, &mut problems);
        let mut keymap = Keymap::default();
        let overrides = cfg.keys.iter().map(|(k, v)| (k.clone(), v.to_vec())).collect();
        problems.extend(keymap.apply(&overrides));
        for h in &cfg.hooks {
            for ev in &h.on {
                if crate::hooks::HookEvent::from_name(ev).is_none() {
                    problems.push(format!("hook {:?}: unknown event {ev:?}", h.name));
                }
            }
            if let Some(k) = &h.key
                && Key::parse(k).is_none()
            {
                problems.push(format!("hook {:?}: unknown key {k:?}", h.name));
            }
        }
        let hosts = cfg
            .hosts()
            .into_iter()
            .map(|cfg| HostState {
                cfg,
                feed: FeedStatus::Unknown,
                harnesses: Vec::new(),
                probed: false,
                listed_at_ms: None,
                listing: false,
                error: None,
            })
            .collect();
        Self {
            cfg,
            theme,
            keymap,
            hosts,
            slots: Vec::new(),
            selected: None,
            focus: Pane::Sidebar,
            composer: Composer::default(),
            overlay: None,
            toasts: VecDeque::new(),
            size: (80, 24),
            client_tag: client_tag.into(),
            entropy: Box::new(urandom),
            dirty: true,
            now_ms: 0,
            problems,
            layout: Layout::default(),
        }
    }

    /// Replaces the source of session-id entropy (tests).
    pub fn set_entropy(&mut self, f: impl FnMut() -> [u8; 15] + Send + 'static) {
        self.entropy = Box::new(f);
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn hosts(&self) -> &[HostState] {
        &self.hosts
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    pub fn selected(&self) -> Option<&Slot> {
        let key = self.selected.as_ref()?;
        self.slots.iter().find(|s| &s.key == key)
    }

    pub fn focus(&self) -> Pane {
        self.focus
    }

    pub fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// First effects: list and probe every host.
    pub fn start(&mut self, now_ms: u64) -> Vec<Effect> {
        self.now_ms = now_ms;
        let mut out = Vec::new();
        for i in 0..self.hosts.len() {
            out.extend(self.refresh_host(i));
            out.push(self.run(
                &self.hosts[i].cfg.name.clone(),
                acc_core::Exec::new(acc_core::harness::probe_script()),
                ExecTag::Probe,
            ));
        }
        out
    }

    pub fn handle(&mut self, event: AppEvent, now_ms: u64) -> Vec<Effect> {
        self.now_ms = now_ms;
        self.dirty = true;
        match event {
            AppEvent::Key(key) => self.on_key(key),
            AppEvent::Paste(text) => {
                if let Some(o) = &mut self.overlay {
                    o.paste(&text);
                } else {
                    self.composer.insert(&text);
                    self.focus = Pane::Composer;
                }
                Vec::new()
            }
            AppEvent::Resize { width, height } => {
                self.size = (width, height);
                for s in &mut self.slots {
                    s.wrap.clear();
                }
                Vec::new()
            }
            AppEvent::Mouse { x, y, kind } => self.on_mouse(x, y, kind),
            AppEvent::Tick => self.on_tick(),
            AppEvent::ExecDone { host, tag, result } => self.on_exec_done(&host, tag, result),
            AppEvent::FeedLine { host, key, bytes } => self.on_feed_line(&host, &key, &bytes),
            AppEvent::FeedUp { host } => {
                if let Some(h) = self.host_mut(&host) {
                    h.feed = FeedStatus::Up;
                }
                Vec::new()
            }
            AppEvent::FeedDown { host, error } => {
                if let Some(h) = self.host_mut(&host) {
                    h.feed = FeedStatus::Down(error);
                }
                Vec::new()
            }
            AppEvent::Notice { text, error } => {
                self.toast(text, error);
                Vec::new()
            }
            AppEvent::HookDone { name, status, stderr } => {
                if status != Some(0) {
                    let detail = if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(": {stderr}")
                    };
                    self.toast(format!("hook {name} failed{detail}"), true);
                }
                Vec::new()
            }
        }
    }

    // ---- helpers -------------------------------------------------------------

    pub(crate) fn host(&self, id: &str) -> Option<&HostState> {
        self.hosts.iter().find(|h| h.cfg.name == id)
    }

    pub(crate) fn host_mut(&mut self, id: &str) -> Option<&mut HostState> {
        self.hosts.iter_mut().find(|h| h.cfg.name == id)
    }

    pub(crate) fn slot_index(&self, key: &SessionKey) -> Option<usize> {
        self.slots.iter().position(|s| &s.key == key)
    }

    pub(crate) fn slot_mut(&mut self, key: &SessionKey) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| &s.key == key)
    }

    pub(crate) fn run(&self, host: &HostId, exec: acc_core::Exec, tag: ExecTag) -> Effect {
        Effect::Run {
            host: host.clone(),
            exec,
            tag,
            timeout_ms: self.cfg.general.exec_timeout_s.max(1) * 1000,
        }
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>, error: bool) {
        let style = if error {
            Style::new().fg(self.theme.error).bold()
        } else {
            Style::new().fg(self.theme.accent)
        };
        self.toasts.push_back(Toast {
            until_ms: self.now_ms + if error { 8_000 } else { 4_000 },
            line: Line::styled(text.into(), style),
        });
        while self.toasts.len() > 3 {
            self.toasts.pop_front();
        }
        self.dirty = true;
    }

    /// Re-lists sessions on a host.
    pub(crate) fn refresh_host(&mut self, i: usize) -> Vec<Effect> {
        let host = self.hosts[i].cfg.name.clone();
        self.hosts[i].listing = true;
        vec![self.run(
            &host,
            acc_core::Exec::new(acc_core::detached::list_script()),
            ExecTag::List,
        )]
    }

    fn on_tick(&mut self) -> Vec<Effect> {
        let now = self.now_ms;
        let before = self.toasts.len();
        self.toasts.retain(|t| t.until_ms > now);
        let mut out = Vec::new();
        let refresh_ms = self.cfg.general.list_refresh_s.max(2) * 1000;
        for i in 0..self.hosts.len() {
            let due = self.hosts[i]
                .listed_at_ms
                .is_none_or(|t| now.saturating_sub(t) >= refresh_ms);
            if due && !self.hosts[i].listing {
                out.extend(self.refresh_host(i));
            }
            if !self.hosts[i].probed
                && self.hosts[i]
                    .listed_at_ms
                    .is_some_and(|t| now.saturating_sub(t) > 30_000)
            {
                // The probe never came back (slow host); ask again.
                let host = self.hosts[i].cfg.name.clone();
                self.hosts[i].listed_at_ms = Some(now);
                out.push(self.run(
                    &host,
                    acc_core::Exec::new(acc_core::harness::probe_script()),
                    ExecTag::Probe,
                ));
            }
        }
        for s in &mut self.slots {
            if let Some(at) = s.unfollow_at_ms
                && now >= at
            {
                s.unfollow_at_ms = None;
                out.push(Effect::Unfollow {
                    host: s.key.host.clone(),
                    key: s.key.log_key(),
                });
                out.push(Effect::Unfollow {
                    host: s.key.host.clone(),
                    key: s.key.exit_key(),
                });
            }
        }
        // Only repaint when something time-based changed.
        self.dirty = before != self.toasts.len()
            || self
                .slots
                .iter()
                .any(|s| s.phase != Phase::Listed && s.is_alive());
        out
    }

    fn on_mouse(&mut self, x: u16, y: u16, kind: MouseKind) -> Vec<Effect> {
        if self.overlay.is_some() {
            return Vec::new();
        }
        let inside = |r: Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if let Some(sb) = self.layout.sidebar
            && inside(sb)
        {
            match kind {
                MouseKind::Click => {
                    let row = (y.saturating_sub(sb.y + 1)) as usize;
                    if let Some(Some(key)) = self.layout.sidebar_hits.get(row).cloned() {
                        self.select(key);
                    }
                    self.focus = Pane::Sidebar;
                }
                MouseKind::ScrollUp => self.select_move(false),
                MouseKind::ScrollDown => self.select_move(true),
            }
            return Vec::new();
        }
        if inside(self.layout.transcript) {
            match kind {
                MouseKind::Click => self.focus = Pane::Transcript,
                MouseKind::ScrollUp => self.scroll_lines(-3),
                MouseKind::ScrollDown => self.scroll_lines(3),
            }
        } else if inside(self.layout.composer) && kind == MouseKind::Click {
            self.focus = Pane::Composer;
        }
        Vec::new()
    }

    // ---- keys ----------------------------------------------------------------

    fn on_key(&mut self, key: Key) -> Vec<Effect> {
        if self.overlay.is_some() {
            return self.overlay_key(key);
        }
        // Hook keys win over the defaults, like any other override.
        if let Some(hook) = self.hook_for_key(&key) {
            return self.run_hook(&hook, None);
        }
        if let Some(action) = self.keymap.global(&key) {
            return self.action(action);
        }
        if self.focus == Pane::Composer {
            if key.code == KeyCode::Enter && !key.alt && !key.shift {
                return self.send_prompt();
            }
            if key.code == KeyCode::Enter && key.ctrl {
                return self.send_prompt();
            }
            if self.composer.is_empty() && key.plain_char() == Some('/') {
                self.overlay = Some(Overlay::palette());
                return Vec::new();
            }
            if self.composer.key(&key) {
                return Vec::new();
            }
            // Keys the editor doesn't use fall through to navigation.
            if let Some(action) = self.keymap.nav(&key)
                && matches!(
                    action,
                    Action::PageUp | Action::PageDown | Action::FollowTail | Action::Jump(_)
                )
                && key.plain_char().is_none()
            {
                return self.action(action);
            }
            return Vec::new();
        }
        if let Some(action) = self.keymap.nav(&key) {
            return self.action(action);
        }
        // Typing in the transcript or sidebar starts a prompt.
        if let Some(c) = key.plain_char()
            && !c.is_control()
            && self.selected().is_some_and(|s| s.session.is_some())
        {
            self.focus = Pane::Composer;
            self.composer.insert(&c.to_string());
        }
        Vec::new()
    }

    fn hook_for_key(&self, key: &Key) -> Option<String> {
        self.cfg
            .hooks
            .iter()
            .find(|h| h.key.as_deref().and_then(Key::parse).as_ref() == Some(key))
            .map(|h| h.name.clone())
    }

    pub(crate) fn action(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Quit => vec![Effect::Quit],
            Action::Help => {
                self.overlay = Some(Overlay::Help { scroll: 0 });
                Vec::new()
            }
            Action::SelectUp | Action::SelectDown => {
                let down = action == Action::SelectDown;
                match self.focus {
                    Pane::Transcript => self.cursor_move(down),
                    _ => self.select_move(down),
                }
                Vec::new()
            }
            Action::PageUp | Action::PageDown => {
                let page = (self.size.1 as i64 - 6).max(1);
                self.scroll_lines(if action == Action::PageDown { page } else { -page });
                Vec::new()
            }
            Action::ScrollTop => {
                if let Some(s) = self.selected_slot_mut() {
                    s.scroll.follow = false;
                    s.scroll.from_bottom = usize::MAX / 2;
                }
                Vec::new()
            }
            Action::ScrollBottom | Action::FollowTail => {
                if let Some(s) = self.selected_slot_mut() {
                    s.scroll = Default::default();
                    s.cursor = None;
                }
                Vec::new()
            }
            Action::FocusNext | Action::FocusPrev => {
                let order = [Pane::Sidebar, Pane::Transcript, Pane::Composer];
                let i = order.iter().position(|p| *p == self.focus).unwrap_or(0);
                let n = if action == Action::FocusNext { i + 1 } else { i + 2 };
                self.focus = order[n % 3];
                Vec::new()
            }
            Action::FocusSidebar => {
                self.focus = Pane::Sidebar;
                Vec::new()
            }
            Action::FocusTranscript => {
                self.focus = Pane::Transcript;
                Vec::new()
            }
            Action::FocusComposer => {
                self.focus = Pane::Composer;
                Vec::new()
            }
            Action::Open => self.open_selected(),
            Action::Back => {
                self.focus = match self.focus {
                    Pane::Composer if self.composer.is_empty() => Pane::Transcript,
                    Pane::Composer => Pane::Composer,
                    Pane::Transcript => Pane::Sidebar,
                    Pane::Sidebar => Pane::Sidebar,
                };
                Vec::new()
            }
            Action::Palette => {
                self.overlay = Some(Overlay::palette());
                Vec::new()
            }
            Action::NewSession => {
                self.overlay = Some(Overlay::launch(self));
                Vec::new()
            }
            Action::Refresh => {
                let mut out = Vec::new();
                for i in 0..self.hosts.len() {
                    out.extend(self.refresh_host(i));
                    if !self.hosts[i].probed {
                        let host = self.hosts[i].cfg.name.clone();
                        out.push(self.run(
                            &host,
                            acc_core::Exec::new(acc_core::harness::probe_script()),
                            ExecTag::Probe,
                        ));
                    }
                }
                out
            }
            Action::Kill => {
                if let Some(s) = self.selected() {
                    if s.is_alive() {
                        self.overlay = Some(Overlay::confirm_kill(s.key.clone(), s.title()));
                    } else {
                        self.toast("the session isn't running", false);
                    }
                }
                Vec::new()
            }
            Action::Remove => {
                if let Some(s) = self.selected() {
                    self.overlay = Some(Overlay::confirm_remove(s.key.clone(), s.title()));
                }
                Vec::new()
            }
            Action::Interrupt => self.send_selected(Command::Interrupt),
            Action::Compact => self.send_selected(Command::Compact { hint: None }),
            Action::Fork => self.send_selected(Command::Fork),
            Action::Options => {
                if let Some(s) = self.selected()
                    && let Some(session) = &s.session
                {
                    if session.options().is_empty() {
                        self.toast("the harness hasn't reported options yet", false);
                    } else {
                        self.overlay = Some(Overlay::options(s.key.clone()));
                    }
                }
                Vec::new()
            }
            Action::Diff => {
                if let Some(o) = Overlay::diff_for(self) {
                    self.overlay = Some(o);
                } else {
                    self.toast("no file changes in this session", false);
                }
                Vec::new()
            }
            Action::Approve | Action::Deny => self.quick_answer(action == Action::Approve),
            Action::ToggleExpand => {
                if let Some(s) = self.selected_slot_mut()
                    && let Some(session) = &s.session
                    && let Some(cursor) = s.cursor
                    && let Some(item) = session.transcript().items().get(cursor)
                {
                    let id = item.id.clone();
                    if !s.expanded.remove(&id) {
                        s.expanded.insert(id.clone());
                    }
                    s.bump(&id);
                }
                Vec::new()
            }
            Action::Jump(n) => {
                let rows = self.sidebar_keys();
                let idx = if n == 0 { 9 } else { n as usize - 1 };
                if let Some(k) = rows.get(idx) {
                    self.select(k.clone());
                }
                Vec::new()
            }
        }
    }

    /// Session keys in sidebar order.
    pub(crate) fn sidebar_keys(&self) -> Vec<SessionKey> {
        let mut keys = Vec::new();
        for h in &self.hosts {
            let mut slots: Vec<&Slot> = self.slots.iter().filter(|s| s.key.host == h.cfg.name).collect();
            slots.sort_by(|a, b| {
                a.urgency()
                    .cmp(&b.urgency())
                    .then_with(|| b.last_live_ms.cmp(&a.last_live_ms))
                    .then_with(|| b.modified.cmp(&a.modified))
                    .then_with(|| b.spec.created_ms.cmp(&a.spec.created_ms))
            });
            keys.extend(slots.into_iter().map(|s| s.key.clone()));
        }
        keys
    }

    fn select_move(&mut self, down: bool) {
        let keys = self.sidebar_keys();
        if keys.is_empty() {
            return;
        }
        let cur = self
            .selected
            .as_ref()
            .and_then(|k| keys.iter().position(|x| x == k));
        let next = match (cur, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(keys.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.select(keys[next].clone());
    }

    pub(crate) fn select(&mut self, key: SessionKey) {
        self.selected = Some(key.clone());
        if let Some(s) = self.slot_mut(&key) {
            s.attention = false;
        }
    }

    pub(crate) fn selected_slot_mut(&mut self) -> Option<&mut Slot> {
        let key = self.selected.clone()?;
        self.slot_mut(&key)
    }

    /// Moves the transcript's item cursor.
    fn cursor_move(&mut self, down: bool) {
        if let Some(s) = self.selected_slot_mut()
            && let Some(session) = &s.session
        {
            let n = session.transcript().len();
            if n == 0 {
                return;
            }
            s.cursor = Some(match (s.cursor, down) {
                (None, _) => n - 1,
                (Some(c), true) => (c + 1).min(n - 1),
                (Some(c), false) => c.saturating_sub(1),
            });
            s.scroll.follow = false;
            s.scroll.to_cursor = true;
        }
    }

    fn scroll_lines(&mut self, delta: i64) {
        if let Some(s) = self.selected_slot_mut() {
            if delta > 0 {
                s.scroll.from_bottom = s.scroll.from_bottom.saturating_sub(delta as usize);
                if s.scroll.from_bottom == 0 {
                    s.scroll.follow = true;
                }
            } else {
                s.scroll.follow = false;
                s.scroll.from_bottom = s.scroll.from_bottom.saturating_add((-delta) as usize);
            }
        }
    }

    fn open_selected(&mut self) -> Vec<Effect> {
        let Some(key) = self.selected.clone() else {
            return Vec::new();
        };
        let Some(i) = self.slot_index(&key) else {
            return Vec::new();
        };
        match self.slots[i].phase {
            Phase::Listed => self.attach(i),
            Phase::Live => {
                if self.focus == Pane::Sidebar {
                    self.focus = Pane::Composer;
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn send_prompt(&mut self) -> Vec<Effect> {
        if self.composer.is_empty() {
            return Vec::new();
        }
        let Some(key) = self.selected.clone() else {
            self.toast("select a session first", false);
            return Vec::new();
        };
        let alive = self
            .slot_mut(&key)
            .is_some_and(|s| s.session.is_some() && s.is_alive());
        if !alive {
            self.toast("the session isn't running; start a new one with ctrl-n", true);
            return Vec::new();
        }
        let text = self.composer.take();
        if let Some(s) = self.slot_mut(&key) {
            s.scroll = Default::default();
        }
        self.send(&key, Command::Prompt { text })
    }

    fn send_selected(&mut self, cmd: Command) -> Vec<Effect> {
        match self.selected.clone() {
            Some(key) => self.send(&key, cmd),
            None => Vec::new(),
        }
    }

    fn quick_answer(&mut self, approve: bool) -> Vec<Effect> {
        let Some(key) = self.selected.clone() else {
            return Vec::new();
        };
        let Some(s) = self.slot_mut(&key) else {
            return Vec::new();
        };
        let Some(p) = s.session.as_ref().and_then(|s| s.permissions().first().cloned()) else {
            self.toast("nothing to answer", false);
            return Vec::new();
        };
        if p.kind == acc_core::BlockKind::Question || !approve {
            // Questions and denials deserve a look at the options.
            self.overlay = Some(Overlay::permission(key, &p, !approve));
            return Vec::new();
        }
        let Some(opt) = p.options.first() else {
            return Vec::new();
        };
        self.send(
            &key,
            Command::AnswerPermission {
                request_id: p.request_id.clone(),
                option_id: opt.id.clone(),
            },
        )
    }
}
