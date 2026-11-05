//! Plan first, then build, ported from Sessh's `PlanFlow`: the agent reads
//! the code and writes a plan in its harness's plan mode (Claude's `plan`
//! permission, Codex's and ACP's `plan` mode), you approve or revise it,
//! and the approved plan is saved into the repository before an agent
//! builds it. Pure: no I/O, only what the app decides from a slot.

use acc_core::Harness;
use acc_core::command::{OptionDef, OptionId};
use acc_core::event::ItemKind;
use acc_core::item::ItemDetail;
use acc_core::session::Permission;
use acc_core::state::RunState;
use serde_json::Value;

use crate::slot::Slot;

/// What the launch wizard starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A session, as it always was.
    Blank,
    /// A change to the code in the directory.
    Feature,
    /// A new folder, made a git repository, to start something in.
    Project,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Blank, Kind::Feature, Kind::Project];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Blank => "blank",
            Kind::Feature => "new feature",
            Kind::Project => "new project",
        }
    }
}

/// The session label that marks a session as Plan First; its value is
/// where the plan is saved, relative to the session's directory.
pub const LABEL: &str = "chameleon.plan_file";

/// Claude's tool for presenting a plan and asking to leave plan mode.
pub const EXIT_PLAN_MODE: &str = "ExitPlanMode";

/// The exit status of [`create_project_script`] when the folder is taken.
pub const EXIT_TAKEN: i32 = 9;

/// Modes to build in, most fitting first: Claude takes edits without
/// asking once the plan is approved, as its own TUI offers.
const BUILD_MODES: [&str; 5] = ["acceptedits", "default", "build", "agent", "code"];

/// The option that puts a harness in plan mode, with the value that does
/// and the one to build with afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanOption {
    pub id: OptionId,
    pub plan: String,
    pub build: String,
}

/// Where a plan comes from: Claude asks to leave plan mode with it
/// (`ExitPlanMode`); the others end their planning turn with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    ExitPlanMode { request_id: String },
    Message { item_id: String },
}

/// A plan waiting for your go-ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub text: String,
    pub source: Source,
}

impl Review {
    /// The request or item the plan came from, to tell one plan from the next.
    pub fn id(&self) -> &str {
        match &self.source {
            Source::ExitPlanMode { request_id } => request_id,
            Source::Message { item_id } => item_id,
        }
    }

    /// The plan's first heading, else its first line.
    pub fn title(&self) -> String {
        let lines: Vec<&str> = self.text.lines().map(str::trim).collect();
        let line = lines
            .iter()
            .find(|l| l.starts_with('#'))
            .or_else(|| lines.iter().find(|l| !l.is_empty()))
            .copied()
            .unwrap_or("");
        line.trim_start_matches('#').trim().to_owned()
    }

    /// The plan without the heading its title came from, to show under it.
    pub fn body(&self) -> String {
        let title = self.title();
        let mut lines = self.text.lines();
        let mut before = Vec::new();
        for l in lines.by_ref() {
            if l.trim().starts_with('#') {
                if l.trim().trim_start_matches('#').trim() == title {
                    let rest: Vec<&str> = before.into_iter().chain(lines).collect();
                    return rest.join("\n").trim().to_owned();
                }
                break;
            }
            before.push(l);
        }
        self.text.trim().to_owned()
    }
}

/// What to do once the plan is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// The planner builds it.
    Build,
    /// A fresh agent builds it, and the planner stops.
    BuildWith(Builder),
}

/// A fresh agent to build an approved plan: any harness, the planner's
/// too, with none of the planning conversation. Unset values keep the
/// harness's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Builder {
    pub harness: Harness,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Option<String>,
}

/// A plan being written into the repository (the tag of that script).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saving {
    /// The planner's session id.
    pub id: String,
    pub path: String,
    pub source: Source,
    pub then: Then,
}

/// A mode value's name: the fragment of a URL-style id, lowercased.
fn mode_name(value: &str) -> String {
    value.rsplit('#').next().unwrap_or(value).to_ascii_lowercase()
}

/// The value to build in among `values`: the first of [`BUILD_MODES`]
/// there, compared case-insensitively.
pub fn build_value<S: AsRef<str>>(values: &[S]) -> Option<&str> {
    BUILD_MODES
        .iter()
        .find_map(|mode| values.iter().map(AsRef::as_ref).find(|v| mode_name(v) == *mode))
}

/// The harness's plan mode: a permission or mode value named `plan` (ACP
/// modes can be URLs ending in `#plan`).
pub fn plan_option(options: &[OptionDef]) -> Option<PlanOption> {
    [OptionId::Permission, OptionId::Mode].into_iter().find_map(|id| {
        let option = options.iter().find(|o| o.id == id)?;
        let plan = option.values.iter().find(|v| mode_name(v) == "plan")?;
        let others: Vec<&String> = option.values.iter().filter(|v| *v != plan).collect();
        let build = build_value(&others).or(others.first().map(|s| s.as_str()))?;
        Some(PlanOption {
            id,
            plan: plan.clone(),
            build: build.to_owned(),
        })
    })
}

/// Where the session's plan is saved, when it was started Plan First.
pub fn plan_file(slot: &Slot) -> Option<&str> {
    slot.spec.labels.get(LABEL).map(String::as_str)
}

/// The plan was approved: the build prompt naming its file was sent.
pub fn is_approved(slot: &Slot) -> bool {
    let (Some(path), Some(session)) = (plan_file(slot), &slot.session) else {
        return false;
    };
    session
        .transcript()
        .items()
        .iter()
        .any(|it| it.kind == ItemKind::UserMessage && it.text.contains(path))
}

/// The session is still working out a plan: in its harness's plan mode,
/// or, without one, started Plan First and not approved yet.
pub fn is_planning(slot: &Slot) -> bool {
    let Some(session) = &slot.session else {
        return false;
    };
    match plan_option(session.options()) {
        Some(p) => session
            .options()
            .iter()
            .find(|o| o.id == p.id)
            .is_some_and(|o| o.current.as_deref() == Some(p.plan.as_str())),
        None => plan_file(slot).is_some() && !is_approved(slot),
    }
}

pub fn is_exit_plan_mode(p: &Permission) -> bool {
    match &p.detail {
        Some(ItemDetail::Tool { tool, .. }) => tool == EXIT_PLAN_MODE,
        _ => p.title.contains(EXIT_PLAN_MODE),
    }
}

/// The plan an `ExitPlanMode` request carries in its input.
fn exit_plan_text(p: &Permission) -> Option<String> {
    let from = |v: &Value| v.get("plan").and_then(Value::as_str).map(str::to_owned);
    let text = match &p.detail {
        Some(ItemDetail::Tool { input, .. }) => from(input),
        _ => None,
    }
    .or_else(|| from(&p.native))?;
    (!text.trim().is_empty()).then_some(text)
}

/// The plan waiting for review in the session, if there is one.
pub fn review(slot: &Slot) -> Option<Review> {
    let session = slot.session.as_ref()?;
    if let Some((p, text)) = session
        .permissions()
        .iter()
        .filter(|p| is_exit_plan_mode(p))
        .find_map(|p| exit_plan_text(p).map(|t| (p, t)))
    {
        return Some(Review {
            text,
            source: Source::ExitPlanMode {
                request_id: p.request_id.clone(),
            },
        });
    }
    // Claude always asks with ExitPlanMode; a message at rest is a question.
    if slot.harness() == Harness::Claude
        || plan_file(slot).is_none()
        || !session.is_alive()
        || !session.permissions().is_empty()
        || !is_planning(slot)
        || is_approved(slot)
    {
        return None;
    }
    if !matches!(
        session.state().map(|s| &s.state),
        Some(RunState::Idle | RunState::Done | RunState::Waiting)
    ) {
        return None;
    }
    let last = session
        .transcript()
        .items()
        .iter()
        .rev()
        .find(|it| it.parent.is_none())?;
    (last.kind == ItemKind::AgentMessage && !last.text.trim().is_empty()).then(|| Review {
        text: last.text.clone(),
        source: Source::Message {
            item_id: last.id.clone(),
        },
    })
}

/// Where an approved plan is saved, relative to the session's directory:
/// `docs/plans/<date>-<slug>.md`, the slug from the description's first
/// six words. The date is the local one at `now_ms`.
pub fn plan_path(description: &str, now_ms: u64, utc_offset_min: i32) -> String {
    let lower = description.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(6)
        .collect();
    let slug = if words.is_empty() {
        "plan".to_owned()
    } else {
        words.join("-")
    };
    let local_ms = now_ms as i64 + i64::from(utc_offset_min) * 60_000;
    let (y, m, d) = civil(local_ms.div_euclid(86_400_000));
    format!("docs/plans/{y:04}-{m:02}-{d:02}-{slug}.md")
}

/// Year, month and day of a count of days since 1970-01-01.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// The first message of a Plan First session on a harness without a plan
/// mode of its own.
pub fn planning_prompt(description: &str) -> String {
    format!(
        "Don't change any files yet. Read the code this needs, then reply with a plan as markdown: \
         a heading naming the change, the approach, the files to change, and how to verify it. \
         I'll review it before you build.\n\n{description}"
    )
}

/// The message that starts the build once the plan is saved.
pub fn build_prompt(path: &str) -> String {
    format!("The plan is approved and saved at {path}. Implement it.")
}

/// A directory typed into the wizard, for a script: `~` is the home
/// directory, the rest is quoted.
fn shell_dir(dir: &str) -> String {
    let dir = dir.trim();
    match dir.strip_prefix('~') {
        Some("") | Some("/") => "\"$HOME\"".to_owned(),
        Some(rest) if rest.starts_with('/') => {
            format!(
                "\"$HOME\"/{}",
                acc_core::shell::quote(rest.trim_start_matches('/'))
            )
        }
        _ if dir.is_empty() => ".".to_owned(),
        _ => acc_core::shell::quote(dir),
    }
}

/// Makes folder `name` in `parent` a new git repository and prints its
/// resolved path; exits [`EXIT_TAKEN`] when the folder exists and isn't empty.
pub fn create_project_script(parent: &str, name: &str) -> String {
    // "/" trims to nothing, which would mean the current directory.
    let parent = match parent.trim().trim_end_matches('/') {
        "" if parent.trim().starts_with('/') => String::new(),
        trimmed => shell_dir(trimmed),
    };
    let name = acc_core::shell::quote(name.trim());
    format!(
        "p={parent}/{name}; [ -e \"$p\" ] && [ -n \"$(ls -A \"$p\")\" ] && exit {EXIT_TAKEN}; \
         mkdir -p \"$p\" && cd \"$p\" && git init -q && pwd -P\n"
    )
}

/// Writes the plan on stdin to `path`, relative to `cwd`.
pub fn save_plan_script(cwd: &str, path: &str) -> String {
    let dir = path.rsplit_once('/').map_or(".", |(d, _)| d);
    format!(
        "cd {} && mkdir -p {} && cat > {}\n",
        acc_core::shell::quote(cwd),
        acc_core::shell::quote(dir),
        acc_core::shell::quote(path)
    )
}

/// A project name the wizard can make: one folder, not `.` or `..`.
pub fn valid_project_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n != "." && n != ".." && !n.contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use acc_core::command::Apply;

    fn opt(id: OptionId, values: &[&str]) -> OptionDef {
        OptionDef {
            id,
            values: values.iter().map(|v| (*v).to_owned()).collect(),
            labels: Vec::new(),
            current: None,
            apply: Apply::Live,
        }
    }

    #[test]
    fn plan_paths_take_the_first_six_words_and_the_local_date() {
        // 2026-11-04 00:30 UTC; an hour behind it is still the 3rd.
        let ms = 1_793_752_200_000;
        assert_eq!(
            plan_path("Chameleon should have the same New Feature, really: now!", ms, 0),
            "docs/plans/2026-11-04-chameleon-should-have-the-same-new.md"
        );
        assert_eq!(plan_path("¿¡ !!", ms, -60), "docs/plans/2026-11-03-plan.md");
        assert_eq!(
            plan_path("Café au lait", ms, 0),
            "docs/plans/2026-11-04-caf-au-lait.md"
        );
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn titles_and_bodies_split_at_the_first_heading() {
        let r = |text: &str| Review {
            text: text.to_owned(),
            source: Source::Message { item_id: "i".into() },
        };
        let plan = r("Here it is.\n\n# Add a thing\n\n## Steps\n- one\n");
        assert_eq!(plan.title(), "Add a thing");
        assert_eq!(plan.body(), "Here it is.\n\n\n## Steps\n- one");
        let plain = r("\n  First line\nsecond\n");
        assert_eq!(plain.title(), "First line");
        assert_eq!(plain.body(), "First line\nsecond");
    }

    #[test]
    fn plan_options_find_plan_mode_and_what_to_build_in() {
        let claude = opt(
            OptionId::Permission,
            &["default", "acceptEdits", "plan", "auto", "dontAsk"],
        );
        assert_eq!(
            plan_option(std::slice::from_ref(&claude)),
            Some(PlanOption {
                id: OptionId::Permission,
                plan: "plan".into(),
                build: "acceptEdits".into(),
            })
        );
        let acp = opt(
            OptionId::Mode,
            &[
                "https://x/modes#Ask",
                "https://x/modes#Plan",
                "https://x/modes#Agent",
            ],
        );
        let p = plan_option(&[opt(OptionId::Model, &["a"]), acp]).unwrap();
        assert_eq!(
            (p.plan.as_str(), p.build.as_str()),
            ("https://x/modes#Plan", "https://x/modes#Agent")
        );
        let odd = opt(OptionId::Mode, &["plan", "yolo"]);
        assert_eq!(plan_option(&[odd]).unwrap().build, "yolo");
        assert_eq!(plan_option(&[opt(OptionId::Mode, &["plan"])]), None);
        assert_eq!(plan_option(&[opt(OptionId::Mode, &["ask", "code"])]), None);
        assert_eq!(
            build_value(&["plan", "Default", "acceptEdits"]),
            Some("acceptEdits")
        );
        assert_eq!(build_value::<&str>(&[]), None);
    }

    #[test]
    fn scripts_quote_what_they_are_given() {
        assert_eq!(
            create_project_script("~/code/", "my app"),
            "p=\"$HOME\"/code/'my app'; [ -e \"$p\" ] && [ -n \"$(ls -A \"$p\")\" ] && exit 9; \
             mkdir -p \"$p\" && cd \"$p\" && git init -q && pwd -P\n"
        );
        assert!(create_project_script("~", "x").starts_with("p=\"$HOME\"/x;"));
        assert!(create_project_script("/", "x").starts_with("p=/x;"));
        assert!(create_project_script("/srv/it's", "x").starts_with("p='/srv/it'\\''s'/x;"));
        assert_eq!(
            save_plan_script("/w/a b", "docs/plans/2026-11-04-x.md"),
            "cd '/w/a b' && mkdir -p docs/plans && cat > docs/plans/2026-11-04-x.md\n"
        );
        assert!(valid_project_name(" app "));
        assert!(!valid_project_name("a/b") && !valid_project_name("..") && !valid_project_name(" "));
    }

    #[test]
    fn prompts_say_what_sessh_says() {
        assert!(planning_prompt("add x").starts_with("Don't change any files yet."));
        assert!(planning_prompt("add x").ends_with("I'll review it before you build.\n\nadd x"));
        assert_eq!(
            build_prompt("docs/plans/p.md"),
            "The plan is approved and saved at docs/plans/p.md. Implement it."
        );
    }
}
