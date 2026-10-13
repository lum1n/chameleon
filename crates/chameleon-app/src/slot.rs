//! One session as the app sees it: a row in the sidebar that may or may
//! not have a live `Session` behind it yet.

use std::collections::{HashMap, HashSet};

use acc_core::detached::Status;
use acc_core::event::{ItemId, Panel};
use acc_core::session::{Session, SessionSpec};
use acc_core::state::{BlockKind, RunState};

use crate::event::HostId;
use crate::wrap::WrapCache;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub host: HostId,
    pub id: String,
}

impl SessionKey {
    pub fn new(host: &str, id: &str) -> Self {
        Self {
            host: host.to_owned(),
            id: id.to_owned(),
        }
    }
    /// Fan-in key of the session's log.
    pub fn log_key(&self) -> String {
        format!("l:{}", self.id)
    }
    /// Fan-in key of the session's exit file.
    pub fn exit_key(&self) -> String {
        format!("x:{}", self.id)
    }
    /// The session id behind a fan-in key, and whether it is the exit file.
    pub fn parse_feed_key(key: &str) -> Option<(&str, bool)> {
        key.strip_prefix("l:")
            .map(|id| (id, false))
            .or_else(|| key.strip_prefix("x:").map(|id| (id, true)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Known from the host's listing only.
    Listed,
    /// The launch script is running.
    Launching,
    /// The status script is running.
    Attaching,
    /// Following the log (replaying history or live).
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scroll {
    /// Stick to the newest line.
    pub follow: bool,
    /// Lines above the bottom when not following.
    pub from_bottom: usize,
    /// Bring the cursor item into view on the next frame.
    pub to_cursor: bool,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            follow: true,
            from_bottom: 0,
            to_cursor: false,
        }
    }
}

/// Urgency for sorting: lower sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Blocked,
    Error,
    Working,
    Idle,
    Done,
    Exited,
    Unknown,
}

#[derive(Debug)]
pub struct Slot {
    pub key: SessionKey,
    pub spec: SessionSpec,
    pub status: Option<Status>,
    /// Last log modification, seconds since the epoch (from the listing).
    pub modified: Option<u64>,
    pub session: Option<Session>,
    pub phase: Phase,
    /// Something happened live while this session wasn't selected.
    pub attention: bool,
    pub last_live_ms: u64,
    pub expanded: HashSet<ItemId>,
    pub scroll: Scroll,
    /// Item under the transcript cursor, when navigating items.
    pub cursor: Option<usize>,
    /// Bumped on every change to an item, for the wrap cache.
    pub revisions: HashMap<ItemId, u64>,
    pub wrap: WrapCache,
    /// Unfollow the log at this time (exited; the tail has caught up).
    pub unfollow_at_ms: Option<u64>,
    /// Our log offset when a listing last found the log longer than that.
    pub behind_at: Option<u64>,
    /// The offset the feed was last restarted at for being stuck there.
    pub refed_at: Option<u64>,
    /// Stop this session once the launch of its replacement succeeds.
    pub kill_after_launch: Option<String>,
    /// A problem with the last script run for this session.
    pub error: Option<String>,
    /// The log size reported by the status script, for exited sessions.
    pub log_size: Option<u64>,
    /// The latest dialog the harness answered with, by palette entry.
    pub panels: HashMap<String, Panel>,
}

impl Slot {
    pub fn new(key: SessionKey, spec: SessionSpec) -> Self {
        Self {
            key,
            spec,
            status: None,
            modified: None,
            session: None,
            phase: Phase::Listed,
            attention: false,
            last_live_ms: 0,
            expanded: HashSet::new(),
            scroll: Scroll::default(),
            cursor: None,
            revisions: HashMap::new(),
            wrap: WrapCache::new(),
            unfollow_at_ms: None,
            behind_at: None,
            refed_at: None,
            kill_after_launch: None,
            error: None,
            log_size: None,
            panels: HashMap::new(),
        }
    }

    pub fn harness(&self) -> acc_core::Harness {
        self.spec.harness
    }

    pub fn title(&self) -> String {
        if let Some(t) = self.spec.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return t.to_owned();
        }
        let cwd = self.spec.cwd.trim_end_matches('/');
        cwd.rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(cwd)
            .to_owned()
    }

    /// Weighs a listing's log size against what the feed has delivered: true
    /// when two listings in a row found us stuck behind at the same offset,
    /// once per offset so a log that ends mid-line can't restart it forever.
    pub fn check_stalled(&mut self, size: u64) -> bool {
        let Some(offset) = self.session.as_ref().map(Session::offset) else {
            return false;
        };
        if offset >= size {
            self.behind_at = None;
            return false;
        }
        let stuck = self.behind_at == Some(offset) && self.refed_at != Some(offset);
        self.behind_at = Some(offset);
        if stuck {
            self.refed_at = Some(offset);
        }
        stuck
    }

    pub fn is_alive(&self) -> bool {
        match &self.session {
            Some(s) => s.is_alive(),
            None => matches!(self.status, Some(Status::Alive { .. })),
        }
    }

    pub fn run_state(&self) -> Option<&RunState> {
        self.session.as_ref().and_then(|s| s.state()).map(|s| &s.state)
    }

    pub fn urgency(&self) -> Urgency {
        if self.session.as_ref().is_some_and(|s| !s.permissions().is_empty()) {
            return Urgency::Blocked;
        }
        match self.run_state() {
            Some(RunState::Blocked { .. }) => Urgency::Blocked,
            Some(RunState::Error) => Urgency::Error,
            Some(RunState::Working { .. }) => Urgency::Working,
            Some(RunState::Idle) => Urgency::Idle,
            Some(RunState::Done) => Urgency::Done,
            Some(RunState::Exited) => Urgency::Exited,
            None => match self.status {
                Some(Status::Alive { .. }) => Urgency::Unknown,
                Some(Status::Exited { .. }) | Some(Status::Missing) => Urgency::Exited,
                None => Urgency::Unknown,
            },
        }
    }

    /// Glyph and a short word for the state.
    pub fn badge(&self) -> (&'static str, &'static str) {
        if let Some(s) = &self.session
            && let Some(p) = s.permissions().first()
        {
            return match p.kind {
                BlockKind::Question => ("?", "question"),
                _ => ("⚠", "approval"),
            };
        }
        match self.run_state() {
            Some(RunState::Blocked { kind }) => match kind {
                BlockKind::Permission => ("⚠", "approval"),
                BlockKind::Question => ("?", "question"),
                BlockKind::Auth => ("⚠", "auth"),
            },
            Some(RunState::Error) => ("✖", "error"),
            Some(RunState::Working { .. }) => {
                let tool_running = self.session.as_ref().is_some_and(|s| {
                    s.transcript().items().iter().rev().take(3).any(|i| {
                        i.status == acc_core::transcript::ItemStatus::Running
                            && matches!(
                                i.kind,
                                acc_core::event::ItemKind::ToolCall
                                    | acc_core::event::ItemKind::CommandExecution
                                    | acc_core::event::ItemKind::FileChange
                                    | acc_core::event::ItemKind::Subagent
                            )
                    })
                });
                if tool_running {
                    ("⚙", "tool")
                } else {
                    ("…", "thinking")
                }
            }
            Some(RunState::Idle) => ("·", "idle"),
            Some(RunState::Done) => ("✓", "done"),
            Some(RunState::Exited) => ("·", "exited"),
            None => match self.phase {
                Phase::Launching => ("…", "starting"),
                Phase::Attaching => ("…", "attaching"),
                Phase::Live => ("…", "replaying"),
                Phase::Listed => match self.status {
                    Some(Status::Alive { .. }) => ("·", "alive"),
                    Some(Status::Exited { .. }) => ("·", "exited"),
                    Some(Status::Missing) => ("·", "missing"),
                    None => ("·", ""),
                },
            },
        }
    }

    pub fn bump(&mut self, id: &str) {
        *self.revisions.entry(id.to_owned()).or_insert(0) += 1;
    }

    pub fn revision(&self, id: &str) -> u64 {
        self.revisions.get(id).copied().unwrap_or(0)
    }
}
