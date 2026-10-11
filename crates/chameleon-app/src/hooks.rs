//! Hook payloads: the JSON and environment a hook command receives.

use acc_core::session::Session;
use serde_json::{Value, json};

use crate::event::HostId;

/// Events hooks can subscribe to (`on = [...]` in config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Permission,
    Question,
    Error,
    Done,
    Idle,
    Exited,
    Turn,
    Notice,
}

impl HookEvent {
    pub fn name(self) -> &'static str {
        match self {
            HookEvent::Permission => "permission",
            HookEvent::Question => "question",
            HookEvent::Error => "error",
            HookEvent::Done => "done",
            HookEvent::Idle => "idle",
            HookEvent::Exited => "exited",
            HookEvent::Turn => "turn",
            HookEvent::Notice => "notice",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "permission" => HookEvent::Permission,
            "question" => HookEvent::Question,
            "error" => HookEvent::Error,
            "done" => HookEvent::Done,
            "idle" => HookEvent::Idle,
            "exited" => HookEvent::Exited,
            "turn" => HookEvent::Turn,
            "notice" => HookEvent::Notice,
            _ => return None,
        })
    }
}

/// The JSON a hook reads on stdin.
pub fn payload(
    host: &HostId,
    session: &Session,
    event: Option<HookEvent>,
    selected_item: Option<&str>,
) -> Value {
    let spec = session.spec();
    let state = session.state();
    json!({
        "host": host,
        "event": event.map(HookEvent::name),
        "spec": serde_json::to_value(spec).unwrap_or(Value::Null),
        "state": state.map(|s| serde_json::to_value(s).unwrap_or(Value::Null)),
        "usage": serde_json::to_value(session.usage()).unwrap_or(Value::Null),
        "quota": session.quota().map(|q| serde_json::to_value(q).unwrap_or(Value::Null)),
        "permissions": serde_json::to_value(session.permissions()).unwrap_or(Value::Null),
        "selected_item": selected_item
            .and_then(|id| session.transcript().get(id))
            .map(|i| serde_json::to_value(i).unwrap_or(Value::Null)),
    })
}

/// `CHAM_*` variables for the hook's environment.
pub fn env(host: &HostId, session: &Session, event: Option<HookEvent>) -> Vec<(String, String)> {
    let spec = session.spec();
    let state = session
        .state()
        .map(|s| {
            serde_json::to_value(&s.state)
                .ok()
                .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let mut env = vec![
        ("CHAM_HOST".to_owned(), host.clone()),
        ("CHAM_SESSION_ID".to_owned(), spec.id.clone()),
        ("CHAM_HARNESS".to_owned(), spec.harness.id().to_owned()),
        (
            "CHAM_HARNESS_NAME".to_owned(),
            spec.harness.display_name().to_owned(),
        ),
        ("CHAM_CWD".to_owned(), spec.cwd.clone()),
        ("CHAM_STATE".to_owned(), state),
        ("CHAM_TITLE".to_owned(), spec.title.clone().unwrap_or_default()),
        (
            "CHAM_NATIVE_ID".to_owned(),
            spec.native_id.clone().unwrap_or_default(),
        ),
        ("CHAM_MODEL".to_owned(), spec.model.clone().unwrap_or_default()),
    ];
    if let Some(e) = event {
        env.push(("CHAM_EVENT".to_owned(), e.name().to_owned()));
    }
    if let Some(p) = session.permissions().first() {
        env.push(("CHAM_REQUEST".to_owned(), p.title.clone()));
    }
    env
}
