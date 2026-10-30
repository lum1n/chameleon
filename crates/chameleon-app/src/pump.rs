//! The byte pump between the runtime and `acc_core::session::Session`:
//! launch and attach, feed log lines, flush what the harness must receive,
//! and fold updates into the slot (attention, hooks, revisions).

use acc_core::command::Command;
use acc_core::detached::{FeedEntry, FeedPath, Status, parse_list};
use acc_core::driver::DriverError;
use acc_core::event::Event;
use acc_core::policy::Policy;
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
        let id = new_session_id(harness, &(self.entropy)());
        self.launch_as(host, req, id)
    }

    /// Starts a new session with id `id` (one a worktree was made for).
    pub(crate) fn launch_as(&mut self, host: &HostId, req: LaunchRequest, id: String) -> Vec<Effect> {
        let Some(harness) = req.harness else {
            self.toast("pick a harness first", true);
            return Vec::new();
        };
        let replaces = req.replaces_parent.then(|| req.parent.clone()).flatten();
        let (mut session, exec) = match Session::launch(req, &id, &self.client_tag, self.now_ms) {
            Ok(x) => x,
            Err(e) => {
                self.toast(format!("can't start {}: {e}", harness.display_name()), true);
                return Vec::new();
            }
        };
        session.set_policy(self.policy());
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

    /// Starts the host's watcher for the session in slot `i`, when the
    /// config asks for one. Only for sessions started here: running it
    /// again replaces the hook another client may have set.
    fn watch(&self, i: usize) -> Vec<Effect> {
        let Some(Ok(hook)) = self.cfg.watch.as_ref().map(crate::config::WatchConfig::hook) else {
            return Vec::new();
        };
        let slot = &self.slots[i];
        match &slot.session {
            Some(s) => vec![self.run(
                &slot.key.host,
                s.watch_exec(hook.as_ref()),
                ExecTag::Watch(slot.key.id.clone()),
            )],
            None => Vec::new(),
        }
    }

    /// The config's rules, for sessions to answer approvals by. Set when a
    /// session is made, before anything waits, so nothing is answered then.
    fn policy(&self) -> Option<Policy> {
        (!self.cfg.policy.is_empty()).then(|| Policy {
            rules: self.cfg.policy.clone(),
        })
    }

    /// Joins the session in slot `i` from its spec.
    pub(crate) fn attach(&mut self, i: usize) -> Vec<Effect> {
        let policy = self.policy();
        let slot = &mut self.slots[i];
        if slot.session.is_some() {
            return Vec::new();
        }
        match Session::attach(slot.spec.clone(), &self.client_tag) {
            Ok(mut s) => {
                s.set_policy(policy);
                let exec = s.status_exec();
                slot.session = Some(s);
                slot.plan = None;
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
        self.try_send(key, cmd).unwrap_or_else(|out| out)
    }

    /// [`Self::send`], telling a command that didn't go (`Err`, after a
    /// toast saying why) from one that did.
    pub(crate) fn try_send(&mut self, key: &SessionKey, cmd: Command) -> Result<Vec<Effect>, Vec<Effect>> {
        let now = self.now_ms;
        let Some(slot) = self.slot_mut(key) else {
            return Err(Vec::new());
        };
        let Some(session) = slot.session.as_mut() else {
            self.toast("the session isn't attached", false);
            return Err(Vec::new());
        };
        let invoked = match &cmd {
            Command::Invoke { name, .. } => Some(name.clone()),
            _ => None,
        };
        let handoff = matches!(cmd, Command::Handoff { .. });
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
                Ok(out)
            }
            Ok(Outbound::Relaunch(req)) => {
                let host = key.host.clone();
                let what = if handoff {
                    "Handing over to"
                } else if req.fork {
                    "Forking"
                } else {
                    "Restarting"
                };
                self.toast(
                    format!("{what} {}…", req.harness.map(Harness::display_name).unwrap_or("")),
                    false,
                );
                Ok(self.launch(&host, req))
            }
            Err(SessionError::Driver(DriverError::MissingArgument(what))) => {
                match invoked.and_then(|name| self.picker(key, &name)) {
                    Some(picker) => self.overlay = Some(picker),
                    None => self.toast(format!("this command needs {what}"), true),
                }
                Err(Vec::new())
            }
            Err(e) => {
                self.toast(e.to_string(), true);
                Err(Vec::new())
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
        let tag = match tag {
            ExecTag::Merge(id) => return self.on_merge_done(&SessionKey::new(host, &id), result),
            ExecTag::Loadout(t) => return self.on_loadout_done(host, t, result),
            tag => tag,
        };
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
            ExecTag::SandboxProbe => {
                // Only the wizard's note needs it; a failure leaves it unknown.
                let support = outcome
                    .ok()
                    .and_then(|o| acc_core::sandbox::parse_probe(&o.stdout_text()));
                if let Some(h) = self.host_mut(host) {
                    h.sandbox = support;
                }
                Vec::new()
            }
            ExecTag::ResolveDir => self.on_resolve_dir(host, outcome),
            ExecTag::AddWorktree(id) => self.on_add_worktree(host, &id, outcome),
            ExecTag::ListDirs(parent) => {
                self.on_list_dirs(host, &parent, outcome);
                Vec::new()
            }
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
            ExecTag::SpecWrite(_) | ExecTag::Merge(_) | ExecTag::Loadout(_) => Vec::new(),
            ExecTag::Watch(_) => {
                if let Err(e) = outcome {
                    self.toast(format!("can't watch the session on {host}: {e}"), true);
                }
                Vec::new()
            }
            ExecTag::Discard(id) => {
                let key = SessionKey::new(host, &id);
                match outcome {
                    Ok(_) => {
                        self.toast("worktree deleted", false);
                        if matches!(&self.overlay, Some(crate::overlay::Overlay::Diff(v)) if v.key == key) {
                            self.overlay = None;
                        }
                        match self
                            .slot_mut(&key)
                            .and_then(|s| s.session.as_ref().map(|x| x.status_exec()))
                        {
                            Some(exec) => vec![self.run(host, exec, ExecTag::Recheck(id))],
                            None => Vec::new(),
                        }
                    }
                    Err(e) => {
                        self.toast(format!("can't delete the worktree: {e}"), true);
                        Vec::new()
                    }
                }
            }
            ExecTag::GitDiff(id) => {
                self.on_git_diff(&SessionKey::new(host, &id), outcome);
                Vec::new()
            }
            ExecTag::Branch(id) => {
                if let Some(s) = self.slot_mut(&SessionKey::new(host, &id)) {
                    // Outside a repository, or the host unreachable: no branch.
                    s.branch = outcome
                        .ok()
                        .map(|o| o.stdout_text().trim().to_owned())
                        .filter(|b| !b.is_empty());
                }
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
                // The PID, then for a sandboxed session what the host applied.
                let stdout = o.stdout_text();
                let pid = stdout.lines().next().unwrap_or("").trim().to_owned();
                self.slots[i].phase = Phase::Live;
                self.slots[i].status = Some(Status::Alive {
                    pid: pid.parse().unwrap_or(0),
                    size: 0,
                });
                let updates = match self.slots[i].session.as_mut() {
                    Some(s) => s.on_launched(&stdout),
                    None => Vec::new(),
                };
                let mut out = Self::follow_effects(&key, 0);
                out.extend(self.process_updates(i, updates));
                out.extend(self.flush(i));
                out.extend(self.watch(i));
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
        slot.status = status;
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
                slot.status = Some(l.status);
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

    pub(crate) fn on_feed_line(
        &mut self,
        host: &HostId,
        key: &str,
        bytes: &[u8],
        offset: u64,
    ) -> Vec<Effect> {
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
            // A restarted feed, or one still draining after a re-follow, can
            // hand out lines the session already read; acc appends a fed
            // delta to its item, so it would show twice.
            match unread(bytes, offset, session.offset()) {
                Some(b) => session.feed(b, now),
                None => return Vec::new(),
            }
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
        let mut spent = false;
        {
            let slot = &mut self.slots[i];
            for u in &updates {
                if let Some(change) = &u.item {
                    slot.bump(change.id());
                }
                // Replayed too: the log is the only place the plan is kept.
                match &u.event {
                    Event::Plan(plan) => slot.plan = Some(plan.clone()),
                    Event::TurnDiff { diff, .. } => {
                        slot.turn_diff = Some(acc_core::item::parse_unified(diff, ""));
                    }
                    _ => {}
                }
                if u.replay {
                    continue;
                }
                // Replayed usage arrives with the first live report, whole.
                spent |= matches!(u.event, Event::Usage(_) | Event::Quota(_));
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
        if spent && let Some(session) = self.slots[i].session.take() {
            self.record_usage(&session);
            self.slots[i].session = Some(session);
        }
        for n in notices {
            self.toast(n, false);
        }
        let key = self.slots[i].key.clone();
        for id in panels {
            self.show_panel(&key, &id);
        }
        let mut out = self.auto_answer(i);
        let waiting = self.slots[i]
            .session
            .as_ref()
            .is_some_and(|s| s.permissions().iter().any(|p| p.kind == BlockKind::Permission));
        if !waiting {
            // Answered already: nothing for a hook to tell you about.
            events.retain(|e| *e != HookEvent::Permission);
        }
        events.dedup();
        for e in events {
            out.extend(self.hook_effects(i, e));
        }
        out
    }

    /// Turns auto-approval on or off; on, it answers what is waiting now.
    pub(crate) fn toggle_auto_approve(&mut self) -> Vec<Effect> {
        self.auto_approve = !self.auto_approve;
        if !self.auto_approve {
            self.toast("auto-approve off", false);
            return Vec::new();
        }
        self.toast("auto-approve on: dangerous requests still ask", false);
        for slot in &mut self.slots {
            // Requests left to you before get another look.
            slot.auto_seen.clear();
        }
        (0..self.slots.len()).flat_map(|i| self.auto_answer(i)).collect()
    }

    /// Allows the session's waiting approvals that don't look dangerous,
    /// while auto-approval is on. Each request is looked at once; one left
    /// to you says why in a toast. Questions are always left to you.
    fn auto_answer(&mut self, i: usize) -> Vec<Effect> {
        if !self.auto_approve {
            return Vec::new();
        }
        let slot = &mut self.slots[i];
        let Some(session) = &slot.session else {
            return Vec::new();
        };
        if session.is_replaying() || !session.is_alive() {
            return Vec::new();
        }
        let mut answers = Vec::new();
        let mut left = Vec::new();
        for p in session.permissions() {
            if p.kind != BlockKind::Permission || !slot.auto_seen.insert(p.request_id.clone()) {
                continue;
            }
            let item = p.item_id.as_deref().and_then(|id| session.transcript().get(id));
            let request = acc_core::policy::Request::new(item, p.detail.as_ref());
            match crate::auto::concern(&request, &session.spec().cwd, &self.cfg.auto_approve) {
                Some(why) => left.push(format!("auto-approve left to you: {why}")),
                // Only an option marked allow-once; never "always", never a guess.
                None => match acc_core::policy::option_for(acc_core::policy::Decision::Allow, &p.options) {
                    Some(opt) => answers.push((p.request_id.clone(), opt.id.clone(), p.title.clone())),
                    None => left.push(format!("auto-approve can't allow {}", p.title)),
                },
            }
        }
        let key = slot.key.clone();
        for why in left {
            self.toast(why, false);
        }
        let mut out = Vec::new();
        for (request_id, option_id, title) in answers {
            self.toast(format!("auto-approved: {title}"), false);
            out.extend(self.send(
                &key,
                Command::AnswerPermission {
                    request_id,
                    option_id,
                },
            ));
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

    /// Commits what the session changed in its worktree and squashes it
    /// into the branch checked out in the repository.
    pub(crate) fn merge_worktree(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(slot) = self.slot_mut(key) else {
            return Vec::new();
        };
        // From the spec, so a session that isn't attached merges too.
        let Some(name) = slot.spec.worktree.clone() else {
            return Vec::new();
        };
        let message = slot
            .spec
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "Changes from {} session {}",
                    slot.harness().display_name(),
                    key.id
                )
            });
        let exec = Exec::new(acc_core::worktree::merge_script(&name, &message, true));
        vec![self.run(&key.host, exec, ExecTag::Merge(key.id.clone()))]
    }

    fn on_merge_done(&mut self, key: &SessionKey, result: Result<ExecOutput, String>) -> Vec<Effect> {
        match result {
            Ok(o) if o.ok() => {
                let commit = o.stdout_text();
                let commit = commit.trim();
                self.toast(format!("merged as {}", commit.get(..10).unwrap_or(commit)), false);
                // Later diffs count from the merge; show that.
                if let Some(crate::overlay::Overlay::Diff(v)) = &mut self.overlay
                    && v.key == *key
                {
                    v.git = crate::diff::GitState::Loading;
                    if let Some(exec) = self
                        .slots
                        .iter()
                        .find(|s| s.key == *key)
                        .and_then(|s| s.spec.worktree.as_deref())
                        .map(|name| Exec::new(acc_core::worktree::diff_script(name)))
                    {
                        return vec![self.run(&key.host, exec, ExecTag::GitDiff(key.id.clone()))];
                    }
                }
            }
            Ok(o) if o.status == Some(acc_core::worktree::EXIT_CONFLICT as i32) => self.toast(
                "the changes conflict with the repository's branch; nothing was merged",
                true,
            ),
            Ok(o) => {
                let err = o.stderr_text();
                self.toast(
                    format!("merge failed: {}", err.lines().last().unwrap_or("exit status")),
                    true,
                );
            }
            Err(e) => self.toast(format!("merge failed: {e}"), true),
        }
        Vec::new()
    }

    /// Stops the harness and deletes the session's worktree and branch.
    pub(crate) fn discard_worktree(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(name) = self.slot_mut(key).and_then(|s| s.spec.worktree.clone()) else {
            return Vec::new();
        };
        let Some(dir) = acc_core::detached::SessionDir::new(&key.id) else {
            return Vec::new();
        };
        // What `Session::worktree_discard_exec` runs, for attached or not.
        let exec = Exec::new(format!(
            "{}{}",
            dir.kill_script(2_000),
            acc_core::worktree::discard_script(&name)
        ));
        vec![self.run(&key.host, exec, ExecTag::Discard(key.id.clone()))]
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

/// What of a line ending at file offset `end` lies past `read`, or `None`
/// when it was all read.
fn unread(bytes: &[u8], end: u64, read: u64) -> Option<&[u8]> {
    if end <= read {
        return None;
    }
    let new = usize::try_from(end - read).unwrap_or(usize::MAX);
    Some(&bytes[bytes.len().saturating_sub(new)..])
}

#[cfg(test)]
mod tests {
    use super::unread;

    #[test]
    fn unread_skips_and_trims_what_was_read() {
        assert_eq!(unread(b"abc\n", 4, 4), None);
        assert_eq!(unread(b"abc\n", 4, 9), None);
        assert_eq!(unread(b"abc\n", 4, 0), Some(&b"abc\n"[..]));
        assert_eq!(unread(b"abc\n", 4, 2), Some(&b"c\n"[..]));
    }
}
