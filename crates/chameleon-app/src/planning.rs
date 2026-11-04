//! Plan First in the app: starting a session in plan mode, opening its
//! plan for review, and building it (see [`crate::plan_flow`]).

use acc_core::Harness;
use acc_core::command::{Command, OptionId};
use acc_core::session::{LaunchRequest, new_session_id};

use crate::event::{Effect, ExecOutput, ExecTag, HostId};
use crate::model::App;
use crate::overlay::{BuilderForm, Overlay, allow_option, deny_option};
use crate::plan_flow::{self, Review, Saving, Source, Then};
use crate::slot::SessionKey;

impl App {
    /// The plan waiting for review in a session.
    pub(crate) fn plan_review(&self, key: &SessionKey) -> Option<Review> {
        self.slots
            .iter()
            .find(|s| &s.key == key)
            .and_then(plan_flow::review)
    }

    /// The "build with…" form for the session's plan, on its own harness.
    pub(crate) fn plan_builder(&self, key: &SessionKey) -> Option<Overlay> {
        let slot = self.slots.iter().find(|s| &s.key == key)?;
        let host = self
            .hosts
            .iter()
            .position(|h| h.cfg.name == key.host)
            .unwrap_or(0);
        let harness = slot.harness();
        Some(Overlay::PlanBuilder {
            key: key.clone(),
            form: BuilderForm {
                row: BuilderForm::AGENT,
                host,
                harness,
                model: String::new(),
                effort: None,
                permission: BuilderForm::default_permission(harness),
            },
        })
    }

    /// Writes the session's plan into its repository, then builds it as
    /// `then` says.
    pub(crate) fn save_plan(&mut self, key: &SessionKey, then: Then) -> Vec<Effect> {
        let Some(slot) = self.slots.iter().find(|s| &s.key == key) else {
            return Vec::new();
        };
        let Some(review) = plan_flow::review(slot) else {
            self.toast("the plan was already answered", false);
            return Vec::new();
        };
        let path = plan_flow::plan_file(slot)
            .map(str::to_owned)
            .unwrap_or_else(|| plan_flow::plan_path(&review.title(), self.now_ms, self.utc_offset_min));
        let mut text = review.text.clone();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let exec = acc_core::Exec::with_stdin(plan_flow::save_plan_script(&slot.spec.cwd, &path), text);
        let tag = ExecTag::SavePlan(Box::new(Saving {
            id: key.id.clone(),
            path,
            source: review.source,
            then,
        }));
        self.toast("saving the plan to the repo", false);
        vec![self.run(&key.host, exec, tag)]
    }

    /// The plan is saved (or isn't): tell the planner to build it, or start
    /// the builder and stop the planner.
    pub(crate) fn on_plan_saved(
        &mut self,
        host: &HostId,
        saving: Saving,
        result: Result<ExecOutput, String>,
    ) -> Vec<Effect> {
        let failed = match &result {
            Ok(o) if o.ok() => None,
            Ok(o) => Some(
                o.stderr_text()
                    .lines()
                    .last()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "exit status".into()),
            ),
            Err(e) => Some(e.clone()),
        };
        if let Some(e) = failed {
            self.toast(format!("can't save the plan: {e}"), true);
            return Vec::new();
        }
        let key = SessionKey::new(host, &saving.id);
        match saving.then {
            Then::Build => self.build_here(&key, &saving.path, &saving.source),
            Then::BuildWith(b) => self.build_with(&key, &saving.path, b),
        }
    }

    /// The planner builds its plan: Claude's request to leave plan mode is
    /// approved, others leave theirs and are told to build.
    fn build_here(&mut self, key: &SessionKey, path: &str, source: &Source) -> Vec<Effect> {
        let Some(slot) = self.slots.iter().find(|s| &s.key == key) else {
            return Vec::new();
        };
        let Some(session) = &slot.session else {
            return Vec::new();
        };
        let harness = slot.harness();
        let mode = plan_flow::plan_option(session.options())
            .map(|p| (p.id, p.build))
            .or_else(|| (harness == Harness::Claude).then(|| (OptionId::Permission, "acceptEdits".into())));
        let allow = match source {
            Source::ExitPlanMode { request_id } => session
                .permissions()
                .iter()
                .find(|p| &p.request_id == request_id)
                .and_then(|p| allow_option(&p.options))
                .map(|o| o.id.clone()),
            Source::Message { .. } => None,
        };
        self.toast(format!("telling {} to build", harness.display_name()), false);
        let mut out = Vec::new();
        match source {
            Source::ExitPlanMode { request_id } => {
                let Some(option_id) = allow else {
                    self.toast("the plan was already answered", false);
                    return Vec::new();
                };
                out.extend(self.send(
                    key,
                    Command::AnswerPermission {
                        request_id: request_id.clone(),
                        option_id,
                    },
                ));
                if let Some((id, value)) = mode {
                    out.extend(self.send(key, Command::SetOption { id, value }));
                }
            }
            Source::Message { .. } => {
                if let Some((id, value)) = mode {
                    out.extend(self.send(key, Command::SetOption { id, value }));
                }
                out.extend(self.send(
                    key,
                    Command::Prompt {
                        text: plan_flow::build_prompt(path),
                        attachments: Vec::new(),
                    },
                ));
            }
        }
        out
    }

    /// A fresh agent builds the plan in the planner's directory, from the
    /// saved plan alone; the planner stops.
    fn build_with(&mut self, key: &SessionKey, path: &str, b: plan_flow::Builder) -> Vec<Effect> {
        let Some(spec) = self.slots.iter().find(|s| &s.key == key).map(|s| s.spec.clone()) else {
            return Vec::new();
        };
        let claude = b.harness == Harness::Claude;
        let req = LaunchRequest {
            harness: Some(b.harness),
            cwd: spec.cwd.clone(),
            model: b.model.clone(),
            // Claude takes its mode at spawn; the others once they run.
            permission: if claude { b.permission.clone() } else { None },
            prompt: Some(plan_flow::build_prompt(path)),
            parent: Some(key.id.clone()),
            worktree: spec.worktree.clone(),
            sandbox: spec.sandbox.clone(),
            ..LaunchRequest::default()
        };
        self.toast(format!("starting {} to build", b.harness.display_name()), false);
        let id = new_session_id(b.harness, &(self.entropy)());
        let mut out = self.launch_as(&key.host, req, id.clone());
        let new = SessionKey::new(&key.host, &id);
        let Some(slot) = self.slot_mut(&new) else {
            return out;
        };
        if let Some(e) = b.effort {
            slot.pending_options.push((OptionId::Effort, e));
        }
        if !claude && let Some(p) = b.permission {
            slot.pending_options.push((OptionId::Permission, p));
        }
        out.extend(self.kill(key));
        out
    }

    /// Feedback typed after "revise" goes back to the planner: Claude's
    /// request to leave plan mode is denied first.
    pub(crate) fn revise_plan(&mut self, key: &SessionKey) -> Vec<Effect> {
        let Some(slot) = self.slot_mut(key) else {
            return Vec::new();
        };
        slot.revising = false;
        let deny = slot.session.as_ref().and_then(|session| {
            session
                .permissions()
                .iter()
                .find(|p| plan_flow::is_exit_plan_mode(p))
                .and_then(|p| deny_option(&p.options).map(|o| (p.request_id.clone(), o.id.clone())))
        });
        match deny {
            Some((request_id, option_id)) => self.send(
                key,
                Command::AnswerPermission {
                    request_id,
                    option_id,
                },
            ),
            None => Vec::new(),
        }
    }

    /// Once a session reports what it can do: the options waiting for it,
    /// then a Plan First session's planning turn (in the harness's plan
    /// mode, or asked in words). `timed_out` asks in words without options.
    pub(crate) fn start_pending(&mut self, i: usize, timed_out: bool) -> Vec<Effect> {
        let slot = &mut self.slots[i];
        if slot.plan_start.is_none() && slot.pending_options.is_empty() {
            return Vec::new();
        }
        let Some(session) = &slot.session else {
            return Vec::new();
        };
        if !session.is_alive() || session.is_replaying() {
            return Vec::new();
        }
        let options = session.options().to_vec();
        // The first turn waits until the harness can take it.
        let ready = !options.is_empty()
            && matches!(
                session.state().map(|s| &s.state),
                Some(acc_core::state::RunState::Idle)
            );
        let pending = if options.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut slot.pending_options)
        };
        let start = if ready || timed_out {
            slot.plan_start.take()
        } else {
            None
        };
        let key = slot.key.clone();
        let mut cmds = Vec::new();
        for (id, value) in pending {
            // A harness without permissions may call it a mode.
            let id = if id == OptionId::Permission && !options.iter().any(|o| o.id == id) {
                OptionId::Mode
            } else {
                id
            };
            if options.iter().any(|o| o.id == id) {
                cmds.push(Command::SetOption { id, value });
            }
        }
        if let Some((description, _)) = start {
            let text = match plan_flow::plan_option(&options) {
                Some(p) => {
                    cmds.push(Command::SetOption {
                        id: p.id,
                        value: p.plan,
                    });
                    description
                }
                None => plan_flow::planning_prompt(&description),
            };
            cmds.push(Command::Prompt {
                text,
                attachments: Vec::new(),
            });
        }
        let mut out = Vec::new();
        for c in cmds {
            out.extend(self.send(&key, c));
        }
        out
    }

    /// Plan First sessions that waited too long for their options.
    pub(crate) fn plan_tick(&mut self) -> Vec<Effect> {
        let now = self.now_ms;
        let due: Vec<usize> = (0..self.slots.len())
            .filter(|&i| {
                self.slots[i]
                    .plan_start
                    .as_ref()
                    .is_some_and(|(_, at)| now >= *at)
            })
            .collect();
        let mut out = Vec::new();
        for i in due {
            out.extend(self.start_pending(i, true));
        }
        out
    }

    /// Opens a plan for review the first time it shows up in the selected
    /// session; another session's says so in a toast.
    pub(crate) fn notice_plan(&mut self, i: usize) {
        let slot = &self.slots[i];
        if slot.session.as_ref().is_none_or(|s| s.is_replaying()) {
            return;
        }
        let Some(review) = plan_flow::review(slot) else {
            return;
        };
        if slot.plan_seen.as_deref() == Some(review.id()) {
            return;
        }
        let key = slot.key.clone();
        let title = slot.title();
        self.slots[i].plan_seen = Some(review.id().to_owned());
        let selected = self.selected.as_ref() == Some(&key);
        // The plan card stands in for Claude's approval card.
        let free = match &self.overlay {
            None => true,
            Some(Overlay::Permission { key: k, .. }) => *k == key,
            Some(_) => false,
        };
        if selected && free {
            self.overlay = Some(Overlay::PlanReview { key, scroll: 0 });
        } else {
            self.toast(format!("plan ready: {title}"), false);
        }
    }
}
