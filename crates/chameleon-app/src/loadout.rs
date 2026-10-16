//! Skills and MCP servers on a host, for every harness there
//! (`acc_core::loadout`): one list of the skill store with each harness's
//! link to it, and one list of MCP servers with which harnesses' config
//! files have each. Scripts run on the host like every other; the config
//! files are edited here, one entry at a time, and written back whole.

use acc_core::loadout::mcp::{self, McpServer, McpTransport};
use acc_core::loadout::skills::{self, LinkState, Listing};
use acc_core::loadout::{LoadoutError, Scope};
use acc_core::{Exec, Harness};

use crate::event::{Effect, ExecOutput, ExecTag, HostId};
use crate::keymap::{Key, KeyCode};
use crate::model::App;
use crate::overlay::Overlay;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Skills,
    Mcp,
}

/// Why a loadout script ran, and for which scope, so a late answer for
/// another scope is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadoutTag {
    /// The store and every harness's skills directory.
    Skills(Scope),
    /// A link, unlink, adopt or delete; the listing is read again after.
    SkillsChanged(Scope),
    /// A harness's MCP config file.
    McpRead(Harness, Scope),
    /// A harness's MCP config file written back.
    McpWrite(Harness, Scope),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

/// One harness's MCP config file as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpFile {
    pub text: String,
    pub servers: Vec<McpServer>,
}

/// A row of the skills list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRow {
    /// Index into the listing's skills.
    Store(usize),
    /// A skill only in a harness's own directory, to adopt into the store.
    Unmanaged { harness: Harness, name: String },
}

/// How one harness has an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpState {
    Enabled,
    Disabled,
    Absent,
    /// The file isn't read (yet, or it can't be).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadoutView {
    pub host: HostId,
    pub scope: Scope,
    pub tab: Tab,
    /// The harnesses on the host: the columns.
    pub harnesses: Vec<Harness>,
    pub skills: Load<Listing>,
    /// Every column's MCP config file; absent for one with none at the scope.
    pub mcp: Vec<(Harness, Load<McpFile>)>,
    pub row: usize,
    /// Index into `harnesses`.
    pub col: usize,
    /// The new MCP server being typed: `name command args…` or `name url`.
    pub input: Option<String>,
    /// A delete waiting for its key to be pressed again: the row's name.
    pub armed: Option<String>,
    /// The project directory `p` switches to (the selected session's).
    pub project: Option<String>,
}

impl LoadoutView {
    pub fn skill_rows(&self) -> Vec<SkillRow> {
        let Load::Ready(listing) = &self.skills else {
            return Vec::new();
        };
        let mut rows: Vec<SkillRow> = (0..listing.skills.len()).map(SkillRow::Store).collect();
        for place in &listing.places {
            if !self.harnesses.contains(&place.harness) {
                continue;
            }
            for name in &place.unmanaged {
                rows.push(SkillRow::Unmanaged {
                    harness: place.harness,
                    name: name.clone(),
                });
            }
        }
        rows
    }

    /// Every server name any harness has, sorted.
    pub fn mcp_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .mcp
            .iter()
            .filter_map(|(_, f)| match f {
                Load::Ready(f) => Some(f.servers.iter().map(|s| s.name.clone())),
                _ => None,
            })
            .flatten()
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn rows(&self) -> usize {
        match self.tab {
            Tab::Skills => self.skill_rows().len(),
            Tab::Mcp => self.mcp_names().len(),
        }
    }

    pub fn link_state(&self, skill: usize, harness: Harness) -> Option<LinkState> {
        let Load::Ready(listing) = &self.skills else {
            return None;
        };
        listing
            .skills
            .get(skill)?
            .placements
            .iter()
            .find(|p| p.harness == harness)
            .map(|p| p.state.clone())
    }

    pub fn file(&self, harness: Harness) -> Option<&McpFile> {
        self.mcp
            .iter()
            .find(|(h, _)| *h == harness)
            .and_then(|(_, f)| match f {
                Load::Ready(f) => Some(f),
                _ => None,
            })
    }

    pub fn mcp_state(&self, name: &str, harness: Harness) -> McpState {
        match self.file(harness) {
            None => McpState::Unknown,
            Some(f) => match f.servers.iter().find(|s| s.name == name) {
                Some(s) if s.enabled => McpState::Enabled,
                Some(_) => McpState::Disabled,
                None => McpState::Absent,
            },
        }
    }

    /// The server `name` as the column's harness has it, else as the first
    /// harness that does.
    pub fn server(&self, name: &str) -> Option<(Harness, &McpServer)> {
        let col = self.harnesses.get(self.col).copied();
        let mut found = self
            .mcp
            .iter()
            .filter_map(|(h, _)| Some((*h, self.file(*h)?.servers.iter().find(|s| s.name == name)?)));
        let first = found.next();
        col.and_then(|c| {
            self.file(c)
                .and_then(|f| f.servers.iter().find(|s| s.name == name))
                .map(|s| (c, s))
        })
        .or(first)
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.rows().saturating_sub(1));
        self.col = self.col.min(self.harnesses.len().saturating_sub(1));
    }
}

/// One line of a stdio command or a URL, for the details and the list.
pub fn transport_summary(t: &McpTransport) -> String {
    match t {
        McpTransport::Stdio { command, args, .. } => {
            acc_core::shell::join(std::iter::once(command.as_str()).chain(args.iter().map(String::as_str)))
        }
        McpTransport::Http { url, .. } => url.clone(),
        McpTransport::Sse { url, .. } => format!("{url} (sse)"),
        McpTransport::Unknown { what } => what.clone(),
    }
}

/// A server from `name command args…` or `name url`.
fn parse_server(line: &str) -> Result<McpServer, String> {
    let mut words = line.split_whitespace();
    let (Some(name), Some(first)) = (words.next(), words.next()) else {
        return Err("type a name, then a command or a URL".into());
    };
    let transport = if first.starts_with("http://") || first.starts_with("https://") {
        McpTransport::Http {
            url: first.to_owned(),
            headers: Vec::new(),
        }
    } else {
        McpTransport::Stdio {
            command: first.to_owned(),
            args: words.map(str::to_owned).collect(),
            env: Vec::new(),
        }
    };
    Ok(McpServer {
        name: name.to_owned(),
        transport,
        enabled: true,
        extras: acc_core::loadout::json::Json::Object(vec![]),
    })
}

/// The scope as a title: `user`, or the project's directory.
pub fn scope_name(scope: &Scope) -> String {
    match scope {
        Scope::User => "user".into(),
        Scope::Project { dir } => dir.clone(),
    }
}

impl App {
    /// Opens skills and MCP servers for the selected session's host (or the
    /// first host), at user scope.
    pub(crate) fn open_loadout(&mut self) -> Vec<Effect> {
        let selected = self.selected().map(|s| (s.key.host.clone(), s.spec.cwd.clone()));
        let Some(host) = selected
            .as_ref()
            .map(|(h, _)| h.clone())
            .or_else(|| self.hosts.first().map(|h| h.cfg.name.clone()))
        else {
            self.toast("no hosts configured", false);
            return Vec::new();
        };
        let index = self.hosts.iter().position(|h| h.cfg.name == host).unwrap_or(0);
        let view = LoadoutView {
            host,
            scope: Scope::User,
            tab: Tab::Skills,
            harnesses: self.wizard_harnesses(index),
            skills: Load::Loading,
            mcp: Vec::new(),
            row: 0,
            col: 0,
            input: None,
            armed: None,
            project: selected.map(|(_, cwd)| cwd),
        };
        self.load_loadout(view)
    }

    /// Reads everything again and shows `view`.
    fn load_loadout(&mut self, mut view: LoadoutView) -> Vec<Effect> {
        view.skills = Load::Loading;
        view.mcp = view
            .harnesses
            .iter()
            .filter(|h| mcp::read_script(**h, &view.scope).is_some())
            .map(|h| (*h, Load::Loading))
            .collect();
        let mut out = vec![self.loadout_run(
            &view,
            Exec::new(skills::list_script(&view.scope)),
            LoadoutTag::Skills(view.scope.clone()),
        )];
        for (h, _) in &view.mcp {
            if let Some(script) = mcp::read_script(*h, &view.scope) {
                out.push(self.loadout_run(
                    &view,
                    Exec::new(script),
                    LoadoutTag::McpRead(*h, view.scope.clone()),
                ));
            }
        }
        self.overlay = Some(Overlay::Loadout(Box::new(view)));
        out
    }

    fn loadout_run(&self, view: &LoadoutView, exec: Exec, tag: LoadoutTag) -> Effect {
        self.run(&view.host, exec, ExecTag::Loadout(tag))
    }

    pub(crate) fn on_loadout_done(
        &mut self,
        host: &HostId,
        tag: LoadoutTag,
        result: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let scope = match &tag {
            LoadoutTag::Skills(s)
            | LoadoutTag::SkillsChanged(s)
            | LoadoutTag::McpRead(_, s)
            | LoadoutTag::McpWrite(_, s) => s.clone(),
        };
        // A change still lands when the view is closed or elsewhere; say so.
        let error = |o: &ExecOutput| {
            let err = o.stderr_text();
            match o.status {
                Some(c) if c == skills::EXIT_NO_SKILL as i32 => "the skill isn't in the store".to_owned(),
                Some(c) if c == skills::EXIT_CONFLICT as i32 && err.is_empty() => {
                    "a skill of that name is already in the store".to_owned()
                }
                _ if !err.is_empty() => err.lines().last().unwrap_or_default().to_owned(),
                s => format!(
                    "exit status {}",
                    s.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                ),
            }
        };
        let failed = match &result {
            Ok(o) if o.ok() => None,
            Ok(o) => Some(error(o)),
            Err(e) => Some(e.trim().to_owned()),
        };
        if matches!(tag, LoadoutTag::SkillsChanged(_) | LoadoutTag::McpWrite(..))
            && let Some(e) = &failed
        {
            self.toast(e.clone(), true);
        }
        let Some(Overlay::Loadout(view)) = self.overlay.as_mut() else {
            return Vec::new();
        };
        if view.host != *host || view.scope != scope {
            return Vec::new();
        }
        self.dirty = true;
        match tag {
            LoadoutTag::Skills(_) => {
                view.skills = match (result, failed) {
                    (Ok(o), None) => Load::Ready(skills::parse_listing(&o.stdout_text(), &view.scope)),
                    (_, e) => Load::Failed(e.unwrap_or_default()),
                };
                view.clamp();
                Vec::new()
            }
            LoadoutTag::SkillsChanged(scope) => {
                let exec = Exec::new(skills::list_script(&scope));
                let view = view.as_ref().clone();
                vec![self.loadout_run(&view, exec, LoadoutTag::Skills(scope))]
            }
            LoadoutTag::McpRead(h, _) => {
                let file = match (result, failed) {
                    (Ok(o), None) => {
                        let text = o.stdout_text();
                        match mcp::servers(h, &text) {
                            Ok(servers) => Load::Ready(McpFile { text, servers }),
                            Err(e) => Load::Failed(e.to_string()),
                        }
                    }
                    (_, e) => Load::Failed(e.unwrap_or_default()),
                };
                if let Some(slot) = view.mcp.iter_mut().find(|(x, _)| *x == h) {
                    slot.1 = file;
                }
                view.clamp();
                Vec::new()
            }
            LoadoutTag::McpWrite(h, scope) => {
                // Read back what landed, written or not.
                let Some(script) = mcp::read_script(h, &scope) else {
                    return Vec::new();
                };
                let view = view.as_ref().clone();
                vec![self.loadout_run(&view, Exec::new(script), LoadoutTag::McpRead(h, scope))]
            }
        }
    }

    /// Keys of the loadout view; false closes it.
    pub(crate) fn loadout_key(&mut self, view: &mut LoadoutView, key: Key) -> (bool, Vec<Effect>) {
        if let Some(input) = &mut view.input {
            match key.code {
                KeyCode::Esc => view.input = None,
                KeyCode::Enter => {
                    let line = std::mem::take(input);
                    view.input = None;
                    return (true, self.add_server(view, &line));
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                _ => {
                    if let Some(c) = key.plain_char() {
                        input.push(c);
                    }
                }
            }
            return (true, Vec::new());
        }
        let armed = view.armed.take();
        let n = view.rows();
        let cols = view.harnesses.len().max(1);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('L') => return (false, Vec::new()),
            KeyCode::Tab | KeyCode::BackTab => {
                view.tab = match view.tab {
                    Tab::Skills => Tab::Mcp,
                    Tab::Mcp => Tab::Skills,
                };
                view.row = 0;
            }
            KeyCode::Char('j') | KeyCode::Down if n > 0 => view.row = (view.row + 1).min(n - 1),
            KeyCode::Char('k') | KeyCode::Up => view.row = view.row.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => view.row = 0,
            KeyCode::Char('G') | KeyCode::End => view.row = n.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => view.col = (view.col + 1) % cols,
            KeyCode::Char('h') | KeyCode::Left => view.col = (view.col + cols - 1) % cols,
            KeyCode::Char('r') => {
                let v = view.clone();
                return (false, self.load_loadout(v));
            }
            KeyCode::Char('p') => {
                let Some(dir) = view.project.clone() else {
                    self.toast("select a session to see its project's", false);
                    return (true, Vec::new());
                };
                let mut v = view.clone();
                v.scope = match v.scope {
                    Scope::User => Scope::Project { dir },
                    Scope::Project { .. } => Scope::User,
                };
                v.row = 0;
                return (false, self.load_loadout(v));
            }
            _ => {
                let fx = match view.tab {
                    Tab::Skills => self.skills_key(view, key, armed),
                    Tab::Mcp => self.mcp_key(view, key, armed),
                };
                return (true, fx);
            }
        }
        (true, Vec::new())
    }

    fn skills_key(&mut self, view: &mut LoadoutView, key: Key, armed: Option<String>) -> Vec<Effect> {
        let Load::Ready(listing) = &view.skills else {
            return Vec::new();
        };
        let Some(row) = view.skill_rows().get(view.row).cloned() else {
            return Vec::new();
        };
        let harness = view.harnesses.get(view.col).copied();
        let scope = view.scope.clone();
        let script = match (row, key.code) {
            (SkillRow::Store(i), code) => {
                let name = listing.skills[i].name.clone();
                match code {
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        let Some(h) = harness else {
                            return Vec::new();
                        };
                        match view.link_state(i, h) {
                            Some(LinkState::Linked) => skills::unlink_script(&scope, &name, &[h]),
                            Some(LinkState::Missing) => skills::link_script(&scope, &name, &[h], false),
                            Some(LinkState::Native) => {
                                self.toast(format!("{} reads the store itself", h.display_name()), false);
                                return Vec::new();
                            }
                            Some(LinkState::Foreign { what }) => {
                                self.toast(
                                    format!("{}: {what} is in the way; F replaces it", h.display_name()),
                                    false,
                                );
                                return Vec::new();
                            }
                            None => {
                                self.toast(
                                    format!("{} has no skills directory here", h.display_name()),
                                    false,
                                );
                                return Vec::new();
                            }
                        }
                    }
                    KeyCode::Char('F') => {
                        let Some(h) = harness else {
                            return Vec::new();
                        };
                        skills::link_script(&scope, &name, &[h], true)
                    }
                    KeyCode::Char('a') => skills::link_script(&scope, &name, &view.harnesses, false),
                    KeyCode::Char('u') => skills::unlink_script(&scope, &name, &view.harnesses),
                    KeyCode::Char('X') if armed.as_deref() == Some(name.as_str()) => {
                        skills::remove_script(&scope, &name)
                    }
                    KeyCode::Char('X') => {
                        view.armed = Some(name);
                        return Vec::new();
                    }
                    _ => return Vec::new(),
                }
            }
            (SkillRow::Unmanaged { harness, name }, KeyCode::Char('A') | KeyCode::Enter) => {
                skills::adopt_script(&scope, harness, &name)
            }
            _ => return Vec::new(),
        };
        match script {
            Ok(s) => vec![self.loadout_run(view, Exec::new(s), LoadoutTag::SkillsChanged(scope))],
            Err(e) => {
                self.toast(e.to_string(), true);
                Vec::new()
            }
        }
    }

    fn mcp_key(&mut self, view: &mut LoadoutView, key: Key, armed: Option<String>) -> Vec<Effect> {
        if key.code == KeyCode::Char('n') {
            view.input = Some(String::new());
            return Vec::new();
        }
        let Some(name) = view.mcp_names().get(view.row).cloned() else {
            return Vec::new();
        };
        let Some(h) = view.harnesses.get(view.col).copied() else {
            return Vec::new();
        };
        let edits: Vec<(Harness, Result<String, LoadoutError>)> = match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => match view.mcp_state(&name, h) {
                McpState::Unknown => {
                    self.toast(format!("{} has no MCP config here", h.display_name()), false);
                    return Vec::new();
                }
                McpState::Absent => {
                    let Some((_, server)) = view.server(&name) else {
                        return Vec::new();
                    };
                    let server = server.portable();
                    vec![(h, self.edit(view, h, |t| mcp::upsert(h, t, &server)))]
                }
                _ => vec![(h, self.edit(view, h, |t| mcp::remove(h, t, &name)))],
            },
            KeyCode::Char('e') => {
                let enabled = match view.mcp_state(&name, h) {
                    McpState::Enabled => false,
                    McpState::Disabled => true,
                    _ => return Vec::new(),
                };
                if !mcp::supports_disable(h) {
                    self.toast(
                        format!(
                            "{} can't keep a server switched off; space removes it",
                            h.display_name()
                        ),
                        false,
                    );
                    return Vec::new();
                }
                vec![(h, self.edit(view, h, |t| mcp::set_enabled(h, t, &name, enabled)))]
            }
            KeyCode::Char('a') => {
                let Some((_, server)) = view.server(&name) else {
                    return Vec::new();
                };
                let server = server.portable();
                view.harnesses
                    .clone()
                    .into_iter()
                    .filter(|x| view.mcp_state(&name, *x) == McpState::Absent)
                    .map(|x| (x, self.edit(view, x, |t| mcp::upsert(x, t, &server))))
                    .collect()
            }
            KeyCode::Char('X') if armed.as_deref() == Some(name.as_str()) => view
                .harnesses
                .clone()
                .into_iter()
                .filter(|x| matches!(view.mcp_state(&name, *x), McpState::Enabled | McpState::Disabled))
                .map(|x| (x, self.edit(view, x, |t| mcp::remove(x, t, &name))))
                .collect(),
            KeyCode::Char('X') => {
                view.armed = Some(name);
                return Vec::new();
            }
            _ => return Vec::new(),
        };
        self.write_files(view, edits)
    }

    /// Adds the server typed after `n` to the column's harness.
    fn add_server(&mut self, view: &mut LoadoutView, line: &str) -> Vec<Effect> {
        let server = match parse_server(line) {
            Ok(s) => s,
            Err(e) => {
                self.toast(e, false);
                return Vec::new();
            }
        };
        let Some(h) = view.harnesses.get(view.col).copied() else {
            return Vec::new();
        };
        if view.file(h).is_none() {
            self.toast(format!("{} has no MCP config here", h.display_name()), false);
            return Vec::new();
        }
        let edit = self.edit(view, h, |t| mcp::upsert(h, t, &server));
        let fx = self.write_files(view, vec![(h, edit)]);
        if let Some(i) = view.mcp_names().iter().position(|n| *n == server.name) {
            view.row = i;
        }
        fx
    }

    /// `f` applied to `harness`'s config as read.
    fn edit(
        &self,
        view: &LoadoutView,
        harness: Harness,
        f: impl FnOnce(&str) -> Result<String, LoadoutError>,
    ) -> Result<String, LoadoutError> {
        let file = view.file(harness).ok_or_else(|| {
            LoadoutError::Unsupported(format!("{} has no MCP config here", harness.display_name()))
        })?;
        f(&file.text)
    }

    /// Writes each edited config back and shows it at once, so the next
    /// edit builds on this one before the host answers.
    fn write_files(
        &mut self,
        view: &mut LoadoutView,
        edits: Vec<(Harness, Result<String, LoadoutError>)>,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        for (h, edit) in edits {
            let text = match edit {
                Ok(t) => t,
                Err(e) => {
                    self.toast(format!("{}: {e}", h.display_name()), true);
                    continue;
                }
            };
            let Some(script) = mcp::write_script(h, &view.scope) else {
                continue;
            };
            if let Ok(servers) = mcp::servers(h, &text)
                && let Some(slot) = view.mcp.iter_mut().find(|(x, _)| *x == h)
            {
                slot.1 = Load::Ready(McpFile {
                    text: text.clone(),
                    servers,
                });
            }
            out.push(self.loadout_run(
                view,
                Exec::with_stdin(script, text),
                LoadoutTag::McpWrite(h, view.scope.clone()),
            ));
        }
        view.clamp();
        out
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_server_is_a_command_or_a_url() {
        let s = parse_server("fs npx -y @mcp/fs /tmp").unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(s.name, "fs");
        assert!(
            matches!(s.transport, McpTransport::Stdio { ref command, ref args, .. }
            if command == "npx" && args.len() == 3)
        );
        let s = parse_server("docs https://example.com/mcp").unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(s.transport, McpTransport::Http { .. }));
        assert!(parse_server("lonely").is_err());
    }
}
