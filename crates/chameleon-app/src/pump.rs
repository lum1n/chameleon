//! The byte pump between the runtime and `acc_core::session::Session`:
//! launch and attach, feed log lines, flush what the harness must receive,
//! and fold updates into the slot (attention, hooks, revisions).

use acc_core::command::Command;
use acc_core::detached::{FeedEntry, FeedPath, Status, parse_list};
use acc_core::driver::DriverError;
use acc_core::event::Event;
use acc_core::session::{
    LaunchRequest, Outbound, Session, SessionError, SessionSpec, Update, new_session_id,
};
use acc_core::state::{BlockKind, RunState};
use acc_core::{Exec, Harness};

use crate::event::{Effect, ExecOutput, ExecTag, HostId};
use crate::hooks::{self, HookEvent};
use crate::model::App;
use crate::slot::{Phase, SessionKey, Slot};

/// How long after an exit the tail keeps running, so the last lines land.
const UNFOLLOW_GRACE_MS: u64 = 1_500;

impl App {
    /// Starts a new session on a host.
    pub(crate) fn launch(&mut self, host: &HostId, req: LaunchRequest) -> Vec<Effect> {
        let Some(harness) = req.harness else {
            self.toast("pick a harness first", true);
            return Vec::new();
        };
        let replaces = req.replaces_parent.then(|| req.parent.clone()).flatten();
        let id = new_session_id(harness, &(self.entropy)());
        let (session, exec) = match Session::launch(req, &id, &self.client_tag, self.now_ms) {
            Ok(x) => x,
            Err(e) => {
                self.toast(format!("can't start {}: {e}", harness.display_name()), true);
                return Vec::new();
            }
        };
        let key = SessionKey::new(host, &id);
        let mut slot = Slot::new(key.clone(), session.spec().clone());
        slot.session = Some(session);
        slot.phase = Phase::Launching;
        slot.last_live_ms = self.now_ms;
        slot.kill_after_launch = replaces;
        self.slots.push(slot);
        self.select(key);
        vec![self.run(host, exec, ExecTag::Launch(id))]
    }

    /// Joins the session in slot `i` from its spec.
    pub(crate) fn attach(&mut self, i: usize) -> Vec<Effect> {
        let slot = &mut self.slots[i];
        if slot.session.is_some() {
            return Vec::new();
        }
        match Session::attach(slot.spec.clone(), &self.client_tag) {
            Ok(s) => {
                let exec = s.status_exec();
                slot.session = Some(s);
                slot.phase = Phase::Attaching;
                slot.error = None;
                let (host, id) = (slot.key.host.clone(), slot.key.id.clone());
                vec![self.run(&host, exec, ExecTag::Status(id))]
            }
            Err(e) => {
                slot.error = Some(e.to_string());
                self.toast(format!("can't attach: {e}"), true);
                Vec::new()
            }
        }
    }

    /// Sends a command to a session, or relaunches when the harness needs it.
    pub(crate) fn send(&mut self, key: &SessionKey, cmd: Command) -> Vec<Effect> {
        let now = self.now_ms;
        let Some(slot) = self.slot_mut(key) else {
            return Vec::new();
        };
        let Some(session) = slot.session.as_mut() else {
            self.toast("the session isn't attached", false);
            return Vec::new();
        };
        let invoked = match &cmd {
            Command::Invoke { name, .. } => Some(name.clone()),
            _ => None,
        };
        match session.command(cmd) {
            Ok(Outbound::Send(bytes)) => {
                let mut out = Vec::new();
                if !bytes.is_empty() {
                    out.push(Effect::Input {
                        host: key.host.clone(),
                        session: key.id.clone(),
                        exec: session.input_exec(bytes),
                    });
                }
                let updates = session.take_updates(now);
                let i = self.slot_index(key).unwrap_or(0);
                out.extend(self.process_updates(i, updates));
                out.extend(self.flush(i));
                out
            }
            Ok(Outbound::Relaunch(req)) => {
                let host = key.host.clone();
                let what = if req.fork { "Forking" } else { "Restarting" };
                self.toast(
                    format!("{what} {}…", req.harness.map(Harness::display_name).unwrap_or("")),
                    false,
                );
                self.launch(&host, req)
            }
            Err(SessionError::Driver(DriverError::MissingArgument(what))) => {
                match invoked.and_then(|name| self.picker(key, &name)) {
                    Some(picker) => self.overlay = Some(picker),
                    None => self.toast(format!("this command needs {what}"), true),
                }
                Vec::new()
            }
            Err(e) => {
                self.toast(e.to_string(), true);
                Vec::new()
            }
        }
    }

    /// Whatever the session wants written or rewritten after a feed or command.
    pub(crate) fn flush(&mut self, i: usize) -> Vec<Effect> {
        let slot = &mut self.slots[i];
        let Some(session) = slot.session.as_mut() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let bytes = session.take_transmit();
        if !bytes.is_empty() && session.is_alive() {
            out.push(Effect::Input {
                host: slot.key.host.clone(),
                session: slot.key.id.clone(),
                exec: session.input_exec(bytes),
            });
        }
        if let Some(exec) = session.take_spec_write() {
            let (host, id) = (slot.key.host.clone(), slot.key.id.clone());
            slot.spec = session.spec().clone();
            out.push(self.run(&host, exec, ExecTag::SpecWrite(id)));
        }
        out
    }

    fn follow_effects(key: &SessionKey, from: u64) -> Vec<Effect> {
        vec![
            Effect::Follow {
                host: key.host.clone(),
                entry: FeedEntry {
                    key: key.log_key(),
                    path: FeedPath::SessionLog(key.id.clone()),
                    offset: from,
                },
            },
            Effect::Follow {
                host: key.host.clone(),
                entry: FeedEntry {
                    key: key.exit_key(),
                    path: FeedPath::SessionExit(key.id.clone()),
                    offset: 0,
                },
            },
        ]
    }

    pub(crate) fn on_exec_done(
        &mut self,
        host: &HostId,
        tag: ExecTag,
        result: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let outcome = match result {
            Ok(o) if o.ok() => Ok(o),
            Ok(o) => {
                let err = o.stderr_text();
                Err(if err.is_empty() {
                    format!(
                        "exit status {}",
                        o.status.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                    )
                } else {
                    err
                })
            }
            Err(e) => Err(e),
        };
        match tag {
            ExecTag::Launch(id) => self.on_launch_done(host, &id, outcome),
            ExecTag::Status(id) | ExecTag::Recheck(id) => self.on_status_done(host, &id, outcome),
            ExecTag::List => self.on_list_done(host, outcome),
            ExecTag::Probe => {
                match outcome {
                    Ok(o) => {
                        let found = acc_core::harness::parse_probe(&o.stdout_text());
                        if let Some(h) = self.host_mut(host) {
                            h.harnesses = found;
                            h.probed = true;
                        }
                    }
                    Err(e) => self.host_error(host, Some(e)),
                }
                Vec::new()
            }
            ExecTag::ResolveDir => self.on_resolve_dir(host, outcome),
            ExecTag::Input(id) => {
                if let Err(e) = outcome {
                    let key = SessionKey::new(host, &id);
                    if let Some(s) = self.slot_mut(&key) {
                        s.error = Some(e.clone());
                    }
                    self.toast(format!("input failed: {e}"), true);
                }
                Vec::new()
            }
            ExecTag::Kill(id) => {
                let key = SessionKey::new(host, &id);
                match outcome {
                    Ok(_) => {
                        self.toast("stopped", false);
                        match self
                            .slot_mut(&key)
                            .and_then(|s| s.session.as_ref().map(|x| x.status_exec()))
                        {
                            Some(exec) => vec![self.run(host, exec, ExecTag::Recheck(id))],
                            None => Vec::new(),
                        }
                    }
                    Err(e) => {
                        self.toast(format!("stop failed: {e}"), true);
                        Vec::new()
                    }
                }
            }
            ExecTag::Remove(id) => {
                let key = SessionKey::new(host, &id);
                match outcome {
                    Ok(_) => {
                        self.slots.retain(|s| s.key != key);
                        if self.selected.as_ref() == Some(&key) {
                            self.selected = self.sidebar_keys().first().cloned();
                        }
                        self.toast("removed", false);
                        let mut out = vec![
                            Effect::Unfollow {
                                host: host.clone(),
                                key: key.log_key(),
                            },
                            Effect::Unfollow {
                                host: host.clone(),
                                key: key.exit_key(),
                            },
                        ];
                        if let Some(i) = self.hosts.iter().position(|h| h.cfg.name == *host) {
                            out.extend(self.refresh_host(i));
                        }
                        out
                    }
                    Err(e) => {
                        self.toast(format!("remove failed: {e}"), true);
                        Vec::new()
                    }
                }
            }
            ExecTag::SpecWrite(_) => Vec::new(),
            ExecTag::GitDiff(id) => {
                self.on_git_diff(&SessionKey::new(host, &id), outcome);
                Vec::new()
            }
        }
    }

    fn on_launch_done(
        &mut self,
        host: &HostId,
        id: &str,
        outcome: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let key = SessionKey::new(host, id);
        let Some(i) = self.slot_index(&key) else {
            return Vec::new();
        };
        match outcome {
            Ok(o) => {
                let pid = o.stdout_text().trim().to_owned();
                self.slots[i].phase = Phase::Live;
                self.slots[i].status = Some(Status::Alive {
                    pid: pid.parse().unwrap_or(0),
                    size: 0,
                });
                let mut out = Self::follow_effects(&key, 0);
                out.extend(self.flush(i));
                if let Some(parent) = self.slots[i].kill_after_launch.take() {
                    let pk = SessionKey::new(host, &parent);
                    if let Some(exec) = self
                        .slot_mut(&pk)
                        .and_then(|s| s.session.as_ref().map(|x| x.kill_exec()))
                    {
                        out.push(self.run(host, exec, ExecTag::Kill(parent)));
                    }
                }
                out
            }
            Err(e) => {
                self.slots[i].phase = Phase::Listed;
                self.slots[i].session = None;
                self.slots[i].error = Some(e.clone());
                self.toast(format!("launch failed: {e}"), true);
                Vec::new()
            }
        }
    }

    fn on_status_done(
        &mut self,
        host: &HostId,
        id: &str,
        outcome: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let key = SessionKey::new(host, id);
        let Some(i) = self.slot_index(&key) else {
            return Vec::new();
        };
        let text = match outcome {
            Ok(o) => o.stdout_text(),
            Err(e) => {
                let slot = &mut self.slots[i];
                slot.error = Some(e.clone());
                if slot.phase == Phase::Attaching {
                    slot.phase = Phase::Listed;
                    slot.session = None;
                }
                self.toast(format!("status failed: {e}"), true);
                return Vec::new();
            }
        };
        let status = acc_core::detached::parse_status(&text);
        let was_attaching = self.slots[i].phase == Phase::Attaching;
        let slot = &mut self.slots[i];
        slot.status = status.clone();
        slot.log_size = status.as_ref().map(Status::size);
        let now = self.now_ms;
        let updates = match slot.session.as_mut() {
            Some(s) => s.on_status(&text, now),
            None => Vec::new(),
        };
        let mut out = Vec::new();
        if was_attaching {
            slot.phase = Phase::Live;
            let from = slot.session.as_ref().map(|s| s.offset()).unwrap_or(0);
            let size = slot.log_size.unwrap_or(0);
            if slot.is_alive() || size > from {
                out.extend(Self::follow_effects(&key, from));
            }
        }
        out.extend(self.process_updates(i, updates));
        out.extend(self.flush(i));
        out
    }

    fn on_list_done(&mut self, host: &HostId, outcome: Result<ExecOutput, String>) -> Vec<Effect> {
        let now = self.now_ms;
        let Some(h) = self.host_mut(host) else {
            return Vec::new();
        };
        h.listing = false;
        h.listed_at_ms = Some(now);
        let text = match outcome {
            Ok(o) => {
                self.host_error(host, None);
                o.stdout_text()
            }
            Err(e) => {
                self.host_error(host, Some(e));
                return Vec::new();
            }
        };
        let listings = parse_list(&text);
        let mut seen = Vec::new();
        let mut out = Vec::new();
        let mut refeed = false;
        for l in listings {
            let Some(spec) = l.spec_json.as_deref().and_then(SessionSpec::from_json) else {
                continue;
            };
            let key = SessionKey::new(host, &l.id);
            seen.push(key.clone());
            let i = match self.slot_index(&key) {
                Some(i) => i,
                None => {
                    self.slots.push(Slot::new(key.clone(), spec.clone()));
                    self.slots.len() - 1
                }
            };
            let slot = &mut self.slots[i];
            slot.modified = l.modified;
            if slot.session.is_none() {
                slot.spec = spec;
                slot.status = Some(l.status.clone());
            } else if slot.spec.title.is_none() {
                slot.spec.title = spec.title;
            }
            let alive = matches!(l.status, Status::Alive { .. });
            if alive && slot.phase == Phase::Live {
                refeed |= slot.check_stalled(l.status.size());
            }
            if slot.session.is_some() && !alive && slot.is_alive() && slot.phase == Phase::Live {
                // The listing says it's gone but we never saw the exit file.
                if let Some(exec) = slot.session.as_ref().map(|s| s.status_exec()) {
                    out.push(self.run(host, exec, ExecTag::Recheck(l.id.clone())));
                }
                continue;
            }
            if alive && slot.session.is_none() && self.cfg.general.auto_attach {
                out.extend(self.attach(i));
            }
        }
        if refeed {
            out.push(Effect::Refeed { host: host.clone() });
        }
        // Sessions removed on the host disappear here too, unless still starting.
        self.slots
            .retain(|s| s.key.host != *host || seen.contains(&s.key) || s.phase == Phase::Launching);
        if self
            .selected
            .as_ref()
            .is_some_and(|k| self.slot_index(k).is_none())
        {
            self.selected = None;
        }
        if self.selected.is_none() {
            self.selected = self.sidebar_keys().first().cloned();
        }
        out
    }

    pub(crate) fn on_feed_line(&mut self, host: &HostId, key: &str, bytes: &[u8]) -> Vec<Effect> {
        let Some((id, is_exit)) = SessionKey::parse_feed_key(key) else {
            return Vec::new();
        };
        let skey = SessionKey::new(host, id);
        let Some(i) = self.slot_index(&skey) else {
            return Vec::new();
        };
        let now = self.now_ms;
        let slot = &mut self.slots[i];
        let Some(session) = slot.session.as_mut() else {
            return Vec::new();
        };
        let updates = if is_exit {
            session.on_exit_file(&String::from_utf8_lossy(bytes), now)
        } else {
            session.feed(bytes, now)
        };
        let mut out = self.process_updates(i, updates);
        out.extend(self.flush(i));
        // An exited session whose log is fully read needs no follower.
        let slot = &mut self.slots[i];
        if let Some(s) = &slot.session
            && !s.is_alive()
            && slot.unfollow_at_ms.is_none()
            && slot.log_size.is_some_and(|size| s.offset() >= size)
        {
            slot.unfollow_at_ms = Some(now + UNFOLLOW_GRACE_MS);
        }
        out
    }

    /// Folds updates into the slot and fires hooks for live ones.
    pub(crate) fn process_updates(&mut self, i: usize, updates: Vec<Update>) -> Vec<Effect> {
        if updates.is_empty() {
            return Vec::new();
        }
        let now = self.now_ms;
        let selected = self.selected.as_ref() == Some(&self.slots[i].key);
        let mut events = Vec::new();
        let mut notices = Vec::new();
        let mut panels = Vec::new();
        {
            let slot = &mut self.slots[i];
            for u in &updates {
                if let Some(change) = &u.item {
                    slot.bump(change.id());
                }
                if u.replay {
                    continue;
                }
                slot.last_live_ms = now;
                if !selected {
                    slot.attention = true;
                }
                match &u.event {
                    Event::PermissionRequested { kind, .. } => events.push(match kind {
                        BlockKind::Question => HookEvent::Question,
                        _ => HookEvent::Permission,
                    }),
                    Event::StateChanged(s) => match s.state {
                        RunState::Error => events.push(HookEvent::Error),
                        RunState::Done => events.push(HookEvent::Done),
                        RunState::Idle => events.push(HookEvent::Idle),
                        RunState::Exited => events.push(HookEvent::Exited),
                        _ => {}
                    },
                    Event::TurnCompleted => events.push(HookEvent::Turn),
                    Event::Exited { .. } => {
                        slot.unfollow_at_ms = Some(now + UNFOLLOW_GRACE_MS);
                    }
                    Event::Notice { message } => {
                        events.push(HookEvent::Notice);
                        notices.push(message.clone());
                    }
                    Event::Panel(panel) => {
                        slot.panels.insert(panel.id.clone(), panel.clone());
                        panels.push(panel.id.clone());
                    }
                    _ => {}
                }
            }
        }
        for n in notices {
            self.toast(n, false);
        }
        let key = self.slots[i].key.clone();
        for id in panels {
            self.show_panel(&key, &id);
        }
        let mut out = Vec::new();
        events.dedup();
        for e in events {
            out.extend(self.hook_effects(i, e));
        }
        out
    }

    fn hook_effects(&self, i: usize, event: HookEvent) -> Vec<Effect> {
        let slot = &self.slots[i];
        let Some(session) = &slot.session else {
            return Vec::new();
        };
        self.cfg
            .hooks
            .iter()
            .filter(|h| h.on.iter().any(|n| n == event.name()))
            .map(|h| Effect::Hook {
                name: h.name.clone(),
                command: h.command.clone(),
                payload: hooks::payload(&slot.key.host, session, Some(event), None),
                env: hooks::env(&slot.key.host, session, Some(event)),
                suspend_tui: h.suspend_tui,
            })
            .collect()
    }

    /// Runs a named hook for the selected session (key or palette).
    pub(crate) fn run_hook(&mut self, name: &str, item: Option<&str>) -> Vec<Effect> {
        let Some(h) = self.cfg.hooks.iter().find(|h| h.name == name).cloned() else {
            return Vec::new();
        };
        let Some(slot) = self.selected() else {
            self.toast("select a session first", false);
            return Vec::new();
        };
        let Some(session) = &slot.session else {
            self.toast("the session isn't attached", false);
            return Vec::new();
        };
        vec![Effect::Hook {
            name: h.name.clone(),
            command: h.command.clone(),
            payload: hooks::payload(&slot.key.host, session, None, item),
            env: hooks::env(&slot.key.host, session, None),
            suspend_tui: h.suspend_tui,
        }]
    }

    /// Stops a session's harness.
    pub(crate) fn kill(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(slot) = self.slot_mut(key) else {
            return Vec::new();
        };
        let exec = match &slot.session {
            Some(s) => s.kill_exec(),
            None => acc_core::detached::SessionDir::new(&key.id)
                .map(|d| Exec::new(d.kill_script(3_000)))
                .unwrap_or_else(|| Exec::new(String::new())),
        };
        vec![self.run(&key.host, exec, ExecTag::Kill(key.id.clone()))]
    }

    /// Stops a session and deletes it on the host.
    pub(crate) fn remove(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(slot) = self.slot_mut(key) else {
            return Vec::new();
        };
        let exec = match &slot.session {
            Some(s) => s.remove_exec(),
            None => acc_core::detached::SessionDir::new(&key.id)
                .map(|d| Exec::new(d.remove_script()))
                .unwrap_or_else(|| Exec::new(String::new())),
        };
        vec![self.run(&key.host, exec, ExecTag::Remove(key.id.clone()))]
    }
}
