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
    /// Checking whether the host can sandbox a session.
    SandboxProbe,
    /// Resolving the working directory typed into the launch wizard.
    ResolveDir,
    /// Making the git worktree a wizard launch starts in, named after the
    /// session's id; stdout is the directory.
    AddWorktree(String),
    /// Listing a directory's subdirectories for the wizard's completions.
    ListDirs(String),
    Input(String),
    Kill(String),
    Remove(String),
    SpecWrite(String),
    /// The repository's changes for the diff viewer, by session.
    GitDiff(String),
    /// Starting a session's watcher on its host.
    Watch(String),
    /// Merging a session's worktree into its repository.
    Merge(String),
    /// Deleting a session's worktree.
    Discard(String),
    /// Skills and MCP servers on the host.
    Loadout(crate::loadout::LoadoutTag),
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
    /// Images read for the composer: the files a paste named (`pasted`
    /// is that text, for an overlay, which takes it as text), or the
    /// clipboard's image (`pasted` empty).
    Images {
        pasted: String,
        images: Vec<Result<crate::image::Image, String>>,
    },
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
        /// The file offset just past this line.
        offset: u64,
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
    /// The left button went down.
    Click,
    /// Moved with the left button down.
    Drag,
    /// The left button came up.
    Release,
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
    /// Restart a host's follower from what was already read; it has stalled.
    Refeed {
        host: HostId,
    },
    /// Run a hook command locally with the payload on stdin.
    Hook {
        name: String,
        command: String,
        payload: Value,
        env: Vec<(String, String)>,
        suspend_tui: bool,
    },
    /// Hand the terminal to a local program until it exits (a harness's
    /// own TUI, for commands only it can run).
    Terminal {
        argv: Vec<String>,
    },
    /// Remember the picked theme for the next start (`None`: the
    /// terminal's colours).
    SaveTheme {
        name: Option<String>,
    },
    /// Keep what sessions spent for the usage overview of the next start:
    /// `acc_core::usage::UsageRecord`s as a JSON array, for
    /// [`crate::App::load_usage`].
    SaveUsage {
        json: String,
    },
    /// Open a URL with the local machine's opener (`open`, `xdg-open`).
    Open {
        url: String,
    },
    /// Put text on the clipboard (OSC 52, which works over SSH too).
    Copy {
        text: String,
    },
    /// Read the image on the local clipboard, answered with
    /// [`AppEvent::Images`].
    PasteImage,
    Quit,
}
