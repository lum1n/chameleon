//! The diff viewer: what changed in a session's repository (`git diff`
//! against HEAD, untracked files included, run on the session's host; for
//! a session in a worktree of its own, everything it changed there since
//! the worktree was made), everything the agent edited in this session,
//! merged per file, or, where the harness diffs its turns (Codex), what the
//! latest turn changed.

use acc_core::event::ItemKind;
use acc_core::item::{FileChangeKind, FileDiff, ItemDetail};
use acc_core::session::Session;
use acc_core::transcript::ItemStatus;

use crate::event::{Effect, ExecOutput, ExecTag};
use crate::keymap::{Key, KeyCode};
use crate::model::App;
use crate::overlay::Overlay;
use crate::slot::{SessionKey, Slot};

/// Bytes of `git diff` read at most; a diff this big isn't read in a TUI.
const MAX_BYTES: usize = 4_000_000;
/// Untracked files shown at most.
const MAX_UNTRACKED: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    /// The working tree against HEAD.
    Git,
    /// The agent's edits in this session.
    Session,
    /// The latest turn's diff, as the harness reported it.
    Turn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitState {
    Loading,
    Ready(Vec<FileDiff>),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    pub key: SessionKey,
    pub source: DiffSource,
    pub git: GitState,
    /// The session's directory, to show paths relative to it.
    pub cwd: String,
    /// Index of the file shown.
    pub file: usize,
    /// First line of the file's diff on screen.
    pub scroll: usize,
    /// A path to select once the files are known (opened from an edit).
    pub want: Option<String>,
    /// The source was picked by hand; don't switch it on a git failure.
    pub chosen: bool,
    /// The session has a worktree of its own: git shows what it changed there.
    pub worktree: bool,
}

impl DiffView {
    /// The files of the source shown.
    pub fn files(&self, slot: Option<&Slot>) -> Vec<FileDiff> {
        match self.source {
            DiffSource::Git => match &self.git {
                GitState::Ready(files) => files.clone(),
                _ => Vec::new(),
            },
            DiffSource::Session => slot
                .and_then(|s| s.session.as_ref())
                .map(session_files)
                .unwrap_or_default(),
            DiffSource::Turn => slot.and_then(|s| s.turn_diff.clone()).unwrap_or_default(),
        }
    }

    fn select_wanted(&mut self, files: &[FileDiff]) {
        let Some(want) = &self.want else {
            return;
        };
        // Git paths are relative to the repository; edits usually absolute.
        let hit = files.iter().position(|f| {
            f.path == *want
                || want.ends_with(&format!("/{}", f.path))
                || f.path.ends_with(&format!("/{want}"))
        });
        if let Some(i) = hit {
            self.file = i;
            self.scroll = 0;
            self.want = None;
        }
    }
}

/// Every file the agent changed in the session (subagents included), in
/// the order first touched, each edit's hunks in the order made.
pub fn session_files(session: &Session) -> Vec<FileDiff> {
    let mut out: Vec<FileDiff> = Vec::new();
    for item in session.transcript().items() {
        if item.kind != ItemKind::FileChange || item.status == ItemStatus::Failed {
            continue;
        }
        let ItemDetail::FileChange { files, .. } = item.detail.clone() else {
            continue;
        };
        for f in files {
            match out.iter_mut().find(|o| o.path == f.path) {
                Some(o) => {
                    o.added += f.added;
                    o.removed += f.removed;
                    o.hunks.extend(f.hunks);
                    o.kind = match (o.kind, f.kind) {
                        (FileChangeKind::Add, FileChangeKind::Delete) => FileChangeKind::Delete,
                        (FileChangeKind::Add, _) => FileChangeKind::Add,
                        (_, k) => k,
                    };
                }
                None => out.push(f),
            }
        }
    }
    out
}

/// The repository's uncommitted changes as one unified diff: tracked files
/// against HEAD (or the empty tree before the first commit), renames found,
/// then untracked files as additions. Paths are relative to the top level.
pub fn git_script(cwd: &str) -> String {
    format!(
        "cd {dir} 2>/dev/null || {{ echo \"can't enter the session's directory\" >&2; exit 3; }}\n\
         top=$(git rev-parse --show-toplevel 2>/dev/null) || {{ echo 'not a git repository' >&2; exit 4; }}\n\
         cd \"$top\" || exit 3\n\
         base=HEAD\n\
         git rev-parse -q --verify HEAD >/dev/null || base=$(git hash-object -t tree /dev/null)\n\
         {{\n\
         git -c core.quotepath=off diff --no-color --no-ext-diff -M \"$base\"\n\
         git -c core.quotepath=off ls-files --others --exclude-standard | head -n {MAX_UNTRACKED} | while IFS= read -r f; do\n\
         git -c core.quotepath=off diff --no-color --no-ext-diff --no-index -- /dev/null \"$f\"\n\
         done\n\
         }} | head -c {MAX_BYTES}\n",
        dir = acc_core::shell::quote(cwd),
    )
}

/// The script reading a session's git changes: its worktree's since it
/// was made, else the repository's against HEAD.
fn git_exec(session: &Session) -> acc_core::Exec {
    session
        .worktree_diff_exec()
        .unwrap_or_else(|_| acc_core::Exec::new(git_script(&session.spec().cwd)))
}

impl App {
    /// Opens the diff viewer for the selected session: on the edit under
    /// the cursor when there is one, else on the repository's changes.
    pub(crate) fn open_diff(&mut self) -> Vec<Effect> {
        let Some(slot) = self.selected() else {
            self.toast("select a session first", false);
            return Vec::new();
        };
        let Some(session) = &slot.session else {
            self.toast("the session isn't attached", false);
            return Vec::new();
        };
        let want = slot
            .cursor
            .and_then(|c| session.transcript().items().get(c))
            .and_then(|it| match it.detail.clone() {
                ItemDetail::FileChange { files, .. } => files.first().map(|f| f.path.clone()),
                _ => None,
            });
        let cwd = session.spec().cwd.clone();
        let worktree = session.spec().worktree.is_some();
        let exec = git_exec(session);
        let key = slot.key.clone();
        let mut view = DiffView {
            key: key.clone(),
            source: if want.is_some() {
                DiffSource::Session
            } else {
                DiffSource::Git
            },
            git: GitState::Loading,
            cwd: cwd.clone(),
            file: 0,
            scroll: 0,
            want,
            chosen: false,
            worktree,
        };
        if view.source == DiffSource::Session {
            let files = view.files(Some(slot));
            view.select_wanted(&files);
        }
        self.overlay = Some(Overlay::Diff(Box::new(view)));
        vec![self.run(&key.host, exec, ExecTag::GitDiff(key.id))]
    }

    pub(crate) fn on_git_diff(&mut self, key: &SessionKey, outcome: Result<ExecOutput, String>) {
        let has_edits = self
            .slots
            .iter()
            .find(|s| &s.key == key)
            .and_then(|s| s.session.as_ref())
            .is_some_and(|s| !session_files(s).is_empty());
        let Some(Overlay::Diff(view)) = self.overlay.as_mut() else {
            return;
        };
        if view.key != *key {
            return;
        }
        view.git = match outcome {
            Ok(o) => GitState::Ready(acc_core::item::parse_unified(&o.stdout_text(), "")),
            Err(e) => GitState::Failed(e.trim().to_owned()),
        };
        match &view.git {
            GitState::Failed(_) if !view.chosen && has_edits => view.source = DiffSource::Session,
            GitState::Ready(files) if view.source == DiffSource::Git => {
                let files = files.clone();
                view.file = view.file.min(files.len().saturating_sub(1));
                view.select_wanted(&files);
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Keys of the diff viewer; false closes it.
    pub(crate) fn diff_key(&mut self, view: &mut DiffView, key: Key) -> (bool, Vec<Effect>) {
        let slot = self.slots.iter().find(|s| s.key == view.key);
        let session = slot.and_then(|s| s.session.as_ref());
        let has_turn = slot.is_some_and(|s| s.turn_diff.is_some());
        let files = view.files(slot);
        let n = files.len();
        let (_, body_w, rows) = crate::view::diff::layout(self.size.0, self.size.1);
        let body = files
            .get(view.file)
            .map(|f| crate::view::diff::body(f, &self.theme, body_w))
            .unwrap_or_default();
        let max = body.lines.len().saturating_sub(rows);
        let page = rows.saturating_sub(2).max(1);
        let file = view.file;
        let go_file = |view: &mut DiffView, i: usize| {
            if i != view.file {
                view.file = i;
                view.scroll = 0;
            }
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('d') => return (false, Vec::new()),
            KeyCode::Char('j') | KeyCode::Down => view.scroll = (view.scroll + 1).min(max),
            KeyCode::Char('k') | KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::Char(' ') | KeyCode::PageDown => view.scroll = (view.scroll + page).min(max),
            KeyCode::Char('b') | KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(page),
            KeyCode::Char('u') if key.ctrl => view.scroll = view.scroll.saturating_sub(page / 2),
            KeyCode::Char('e') if key.ctrl => view.scroll = (view.scroll + page / 2).min(max),
            KeyCode::Char('g') | KeyCode::Home => view.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => view.scroll = max,
            KeyCode::Char('n') | KeyCode::Char('J') | KeyCode::Right | KeyCode::Char('l') if n > 0 => {
                go_file(view, (file + 1) % n)
            }
            KeyCode::Char('p') | KeyCode::Char('K') | KeyCode::Left | KeyCode::Char('h') if n > 0 => {
                go_file(view, (file + n - 1) % n)
            }
            KeyCode::Char(']') => match body.hunks.iter().find(|&&h| h > view.scroll) {
                Some(&h) => view.scroll = h.min(max),
                None if file + 1 < n => go_file(view, file + 1),
                None => {}
            },
            KeyCode::Char('[') => match body.hunks.iter().rev().find(|&&h| h < view.scroll) {
                Some(&h) => view.scroll = h,
                None if file > 0 => {
                    // The previous file's last hunk.
                    let prev = crate::view::diff::body(&files[file - 1], &self.theme, body_w);
                    go_file(view, file - 1);
                    let max = prev.lines.len().saturating_sub(rows);
                    view.scroll = prev.hunks.last().copied().unwrap_or(0).min(max);
                }
                None => {}
            },
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('s') => {
                let back = key.code == KeyCode::BackTab;
                view.source = match (view.source, back) {
                    (DiffSource::Git, false) | (DiffSource::Turn, true) => DiffSource::Session,
                    (DiffSource::Session, false) if has_turn => DiffSource::Turn,
                    (DiffSource::Git, true) if has_turn => DiffSource::Turn,
                    (DiffSource::Git, true) => DiffSource::Session,
                    _ => DiffSource::Git,
                };
                view.chosen = true;
                view.file = 0;
                view.scroll = 0;
            }
            KeyCode::Char('r') => {
                let Some(exec) = session.map(git_exec) else {
                    return (true, Vec::new());
                };
                view.git = GitState::Loading;
                let effect = self.run(&view.key.host, exec, ExecTag::GitDiff(view.key.id.clone()));
                return (true, vec![effect]);
            }
            // The viewer is the selected session's; these ask to confirm.
            KeyCode::Char('M') if view.worktree => {
                return (false, self.action(crate::keymap::Action::Merge));
            }
            KeyCode::Char('X') if view.worktree => {
                return (false, self.action(crate::keymap::Action::Discard));
            }
            _ => {}
        }
        (true, Vec::new())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;

    fn sh(dir: &Path, script: &str) -> std::process::Output {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir)
            .output()
            .unwrap()
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cham-diff-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let init = "git init -q && git config user.email t@t && git config user.name t";
        assert!(sh(&dir, init).status.success());
        dir
    }

    fn files(dir: &Path) -> Vec<FileDiff> {
        // Run from a subdirectory: paths still come out repository-relative.
        let out = sh(dir, &git_script(&dir.join("sub").display().to_string()));
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        acc_core::item::parse_unified(&String::from_utf8_lossy(&out.stdout), "")
    }

    #[test]
    fn git_diff_covers_tracked_untracked_deleted_and_renamed_files() {
        let dir = repo("all");
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(dir.join("old name.txt"), "same\nsame\nsame\nsame\n").unwrap();
        assert!(sh(&dir, "git add -A && git commit -qm init").status.success());
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        assert!(sh(&dir, "git mv 'old name.txt' 'new name.txt'").status.success());
        std::fs::write(dir.join("sub/new.rs"), "fn main() {}\n").unwrap();

        let got = files(&dir);
        let by = |p: &str| {
            got.iter()
                .find(|f| f.path == p)
                .unwrap_or_else(|| panic!("{p} in {got:?}"))
        };
        let a = by("a.txt");
        assert_eq!((a.kind, a.added, a.removed), (FileChangeKind::Update, 1, 1));
        assert_eq!(a.hunks[0].old_start, 1);
        assert_eq!(by("gone.txt").kind, FileChangeKind::Delete);
        let r = by("new name.txt");
        assert_eq!(r.kind, FileChangeKind::Rename);
        assert_eq!(r.old_path.as_deref(), Some("old name.txt"));
        let n = by("sub/new.rs");
        assert_eq!((n.kind, n.added), (FileChangeKind::Add, 1));
        assert_eq!(got.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_diff_works_before_the_first_commit_and_says_when_not_a_repo() {
        let dir = repo("fresh");
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        assert!(sh(&dir, "git add staged.txt").status.success());
        std::fs::write(dir.join("loose.txt"), "l\n").unwrap();
        let mut got: Vec<String> = files(&dir).into_iter().map(|f| f.path).collect();
        got.sort();
        assert_eq!(got, ["loose.txt", "staged.txt"]);

        let plain = std::env::temp_dir().join(format!("cham-diff-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        let out = sh(
            &plain,
            &format!(
                "GIT_CEILING_DIRECTORIES={} ; export GIT_CEILING_DIRECTORIES; {}",
                std::env::temp_dir().display(),
                git_script(&plain.display().to_string())
            ),
        );
        assert!(!out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr).trim(),
            "not a git repository"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }
}
