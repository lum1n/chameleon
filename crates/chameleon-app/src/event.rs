//! What flows into the app and what it asks the runtime to do.
//!
//! The runtime owns processes and the terminal; the app owns every
//! `acc_core::session::Session`. They meet only through these two enums.

use acc_core::Exec;
use acc_core::detached::FeedEntry;
use serde_json::Value;

use crate::keymap::Key;

pub type HostId = String;

/// Why a script was run, so its output goes to the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecTag {
    /// The launch script of a session; stdout is the harness PID.
    Launch(String),
    /// The status script of a session being attached.
    Status(String),
    /// A re-check of an attached session's status (after the exit file).
    Recheck(String),
    List,
    Probe,
    /// Resolving the working directory typed into the launch wizard.
    ResolveDir,
    Input(String),
    Kill(String),
    Remove(String),
    SpecWrite(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecOutput {
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(Key),
    /// Text pasted into the terminal.
    Paste(String),
    Resize {
        width: u16,
        height: u16,
    },
    Mouse {
        x: u16,
        y: u16,
        kind: MouseKind,
    },
    Tick,
    ExecDone {
        host: HostId,
        tag: ExecTag,
        result: Result<ExecOutput, String>,
    },
    /// One line of a followed file, from the host's fan-in.
    FeedLine {
        host: HostId,
        key: String,
        bytes: Vec<u8>,
    },
    FeedUp {
        host: HostId,
    },
    FeedDown {
        host: HostId,
        error: String,
    },
    HookDone {
        name: String,
        status: Option<i32>,
        stderr: String,
    },
    /// Show a toast (hooks and scripts talking back).
    Notice {
        text: String,
        error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Click,
    ScrollUp,
    ScrollDown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Run a short script on a host and report its output.
    Run {
        host: HostId,
        exec: Exec,
        tag: ExecTag,
        timeout_ms: u64,
    },
    /// Write to a session's harness; writes for one session are applied in order.
    Input {
        host: HostId,
        session: String,
        exec: Exec,
    },
    Follow {
        host: HostId,
        entry: FeedEntry,
    },
    Unfollow {
        host: HostId,
        key: String,
    },
    /// Run a hook command locally with the payload on stdin.
    Hook {
        name: String,
        command: String,
        payload: Value,
        env: Vec<(String, String)>,
        suspend_tui: bool,
    },
    Quit,
}
