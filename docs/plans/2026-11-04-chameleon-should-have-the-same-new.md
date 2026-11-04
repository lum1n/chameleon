# Plan First: new feature / new project in chameleon, like sessh

## Context
sessh (the Swift app, `~/repos/sessh-swift`) has a guided start: **New Feature** or **New Project**. With **Plan First** on, the agent reads the code and writes a plan without changing anything. The plan is then shown as a summary card with three choices:
- **Build**: save the plan to `docs/plans/…` and have the same agent build it.
- **Revise**: your next message is feedback, and the agent re-plans.
- **Build With…**: save the plan, start a fresh agent with any harness, model, effort and permission, give it only the saved plan, and stop the planner.

chameleon only has a plain launch wizard (`n`), so this plan ports the same flow. Source of truth: `Sessh/Agents/PlanFlow.swift`, `AgentHub.swift` (`startPlanning`, `approvePlan`, `revisePlan`, `savePlan`, `createProject`) and `Features/Agents/Chat/PlanReviewCard.swift`.

Everything happens in `crates/chameleon-app`. No acc-core change is expected: `LaunchRequest.labels`, `Command::SetOption`, `AnswerPermission` and `Exec::with_stdin` already cover it.

## 1. `plan_flow.rs`, a pure module ported from PlanFlow.swift
New file `crates/chameleon-app/src/plan_flow.rs`, registered in `lib.rs`. It does no I/O and has unit tests.
- **`Kind { Blank, Feature, Project }`.**
- **`LABEL = "chameleon.plan_file"`**: the session label that marks a session as Plan First and holds the plan's path.
- **`plan_path(description, date)`**: `docs/plans/<YYYY-MM-DD>-<first 6 ASCII words joined with ->.md`, or `…-plan.md` if there are no words. The date comes from `now_ms`.
- **Prompts, copied word for word from sessh:**
  - `planning_prompt(desc)`: "Don't change any files yet. Read the code this needs, then reply with a plan as markdown: a heading naming the change, the approach, the files to change, and how to verify it. I'll review it before you build.\n\n<desc>"
  - `build_prompt(path)`: "The plan is approved and saved at <path>. Implement it."
- **`plan_option(options: &[OptionDef])`**: the permission or mode option that has a `plan` value.
- **`build_value(...)`**: the first value found in `["acceptEdits","default","build","agent","code"]`, compared case-insensitively.
- **`Source { ExitPlanMode { request_id }, Message { item_id } }`** and **`Review { text, source }`**, with `title()` (the first `#` heading, else the first line) and `body()`.
- **`review(slot) -> Option<Review>`:**
  - Claude: a pending permission whose tool is ExitPlanMode. The text is `native["plan"]`.
  - Other harnesses: a Plan First session that is still in plan mode, has nothing pending and is idle. The text is the last top-level agent message.
- **`is_planning(slot)`**: drives the header chip.
- **`is_approved(slot)`**: true once a user message contains the plan path.
- **`create_project_script(parent, name)`**:
  - Runs `p=<parent>/<name>; [ -e "$p" ] && [ -n "$(ls -A "$p")" ] && exit 9; mkdir -p "$p" && cd "$p" && git init -q && pwd -P`.
  - Values are shell-quoted the way acc-core's scripts quote them.
- **`save_plan_script(cwd, path)`**: `cd <cwd> && mkdir -p <dir> && cat > <path>`, with the plan sent on stdin.

## 2. The wizard learns about kinds (`overlay.rs` `Wizard`, `view/overlay.rs` `launch_view`)
- **New `Wizard` fields:**
  - `kind: Kind`.
  - `plan_first: bool`, defaulting to `cfg.plan.first`, which is true.
  - A `name` text field for projects. `fields` grows to 5.
- **Rows:** add `KIND` (a cycle row: blank / feature / project), `NAME` (shown only for projects) and `PLAN` (a toggle, hidden for blank).
  - Renumber the row constants and teach `text_field()` and up/down navigation to skip hidden rows. Use a `rows(&self) -> Vec<usize>` helper rather than more special cases.
  - `WORKTREE` is hidden for projects.
- **Labels change with the kind:**
  - The prompt row becomes "What do you want to build?" or "What are you making?".
  - For a project, the directory row becomes "parent" and defaults to `cfg.plan.projects_dir`, falling back to `~`.
  - The footer explains Plan First the way sessh does: "The agent reads the code and writes a plan. Nothing changes until you approve it; the plan is saved in docs/plans."
  - Feature and project need a description. If it's missing, show an error on the row.
- **Submitting:**
  - For a project, `wizard_submit` first runs `create_project_script` with a new `ExecTag::CreateProject`. `on_create_project` treats stdout as the resolved cwd and continues into the existing `on_resolve_dir` path. Exit 9 shows the error "the folder exists and isn't empty".
  - `wizard_request` sets `labels[LABEL] = plan_path(...)` when Plan First is on.
    - Claude: `permission = "plan"`, and the description goes as `prompt`.
    - Other harnesses: `prompt = None`, and the description is kept as a new `Slot.plan_start: Option<(String, deadline_ms)>`.
- **Starting other harnesses in plan mode** (in `pump.rs`, where options and state updates arrive):
  - Once the session reports its options: if `plan_option` exists, send `SetOption(plan)` and then the description. If not, send `planning_prompt(desc)`.
  - After 15 s without options, fall back to `planning_prompt`.
  - Either way, clear `plan_start`.
- **Entry points:**
  - New `Action::NewFeature` and `Action::NewProject`, bound to `F` and `P`. Both keys are free in `keymap.rs`.
  - Both actions also appear in the palette. `n` and `ctrl-n` keep opening a blank session.
  - `Overlay::launch(app)` takes a `Kind`.
- **Config:** add a `[plan]` section with `first = true` and `projects_dir`, in `config.rs`.

## 3. The plan summary: `Overlay::PlanReview`
- **New variant:** `PlanReview { key, scroll }`. The review is derived again on every render with `plan_flow::review`, the same way `permission_view` looks up its request.
- **View** (`view/overlay.rs`, `plan_review_view`):
  - Header: "plan ready", the harness, the session title and the plan path.
  - The plan title in bold, then the body rendered with the existing `markdown.rs` and scrolled with j/k, space/b and g/G.
  - Footer: `b build · r revise · w build with… · esc`, with enter also building.
  - When the review is gone (already answered), show "already answered" the way `permission_view` does.
- **When it opens:**
  - `open_selected` (`model.rs` ~1015) opens `PlanReview` in place of `Overlay::permission` whenever `review()` is Some. This also covers Claude's ExitPlanMode card, which the plan card replaces.
  - It opens on its own the first time a review appears for the selected session. `Slot.plan_seen: Option<String>` holds the request or item id so it doesn't reopen.
  - A toast says "plan ready" for sessions that aren't selected.
- **Header chip** in `view/header.rs`: "planning" or "plan ready" for Plan First sessions.
- **Auto-approve:** `auto_answer` in `pump.rs` must never answer ExitPlanMode on a Plan First session. Leave it for review.

## 4. The three options
- **Build** (`b` or enter):
  1. Run `save_plan_script` with `ExecTag::SavePlan(id, Then::Build)`.
  2. When that succeeds:
     - ExitPlanMode: answer it with the approve option, then `SetOption(permission, build_value)`. That is `acceptEdits` for Claude.
     - Message: `SetOption(build_value)`, then `Prompt(build_prompt(path))`.
  3. While this runs, toasts walk through the stages: "saving the plan to the repo" and then "telling <agent> to build".
- **Revise** (`r`):
  1. Close the overlay, set `Slot.revising = true` and focus the composer. The composer placeholder becomes "say what should change in the plan".
  2. `send_prompt` (`model.rs:1030`) catches the next message. For ExitPlanMode, it first denies the request with the deny or reject option. It then sends the text as a prompt and clears `revising`.
  3. Esc in the composer cancels revising. The next plan opens a new review.
- **Build with…** (`w`): opens a new `Overlay::PlanBuilder { key, form }`.
  - **The form**, modelled on the wizard's rows and helpers:
    - Agent: cycles through `wizard_harnesses(host)`, including the planner's own harness.
    - Model: free text, with choices from `wizard_models`.
    - Effort: a cycle row built from the harness's known `OptionId::Effort` values (the planner's own, or the newest session of that harness on the host). The row is hidden when there are none.
    - Permission: cycles `permission_modes(h)`, defaulting to `build_value`, which is `acceptEdits` for Claude.
    - Footer: "A new session in the same directory, with the saved plan and none of the planning conversation."
  - **Enter:**
    1. Save the plan with `ExecTag::SavePlan(id, Then::BuildWith(builder))`.
    2. Then `launch` a new `LaunchRequest` with:
       - the planner's cwd, worktree and sandbox,
       - `parent: planner id`,
       - `model` and `permission` set at spawn,
       - `prompt: build_prompt(path)`.
    3. Effort is applied after launch through a `Slot.pending_options` sent when options arrive. This reuses the `plan_start` mechanism above.
    4. Kill the planner with the existing kill path and select the new session.

## Files
- **New:** `crates/chameleon-app/src/plan_flow.rs`.
- **Changed:**
  - `overlay.rs`: Wizard, the `PlanReview`/`PlanBuilder` variants and their key handling, `wizard_request`.
  - `view/overlay.rs`: launch_view rows, plus `plan_review_view` and `plan_builder_view`.
  - `pump.rs`: plan_start and pending_options, auto_answer, the SavePlan and CreateProject result handling.
  - `event.rs`: the `ExecTag::CreateProject` and `SavePlan` tags.
  - `slot.rs`: `plan_start`, `plan_seen`, `revising`, `pending_options`.
  - `model.rs`: `open_selected`, `send_prompt`, action dispatch.
  - `keymap.rs`: actions and bindings, the help text.
  - `config.rs`: `[plan]`.
  - `view/header.rs` and `view/composer.rs`: the chip and placeholder.
  - `README.md`: rows in the key table and the `[plan]` config section.

## Verification
- **Unit tests in `plan_flow.rs`:** plan_path slugging, title/body splitting, plan_option and build_value selection, and quoting in the scripts.
- **Integration tests in `crates/chameleon-app/tests/fixtures.rs`**, using `app()`, `press`, `feed_line`, `ran` and `written`:
  - `F` opens the wizard as a feature with Plan First on. Submitting launches Claude with `permission=plan`, the plan label and the description.
  - `P` with a name runs the create-project script first and then launches in the directory it prints. Exit 9 shows the error.
  - A non-Claude Plan First launch sends `SetOption(plan)` and then the description once options arrive, or the planning prompt when there is no plan mode.
  - A fed ExitPlanMode request opens `PlanReview` with the title shown, and auto-approve leaves it alone.
  - `b` runs the save script with the plan on stdin, then answers approve and sets acceptEdits.
  - `r` followed by a typed message denies the request and sends the feedback.
  - `w`, then picking codex, then enter saves the plan, launches codex with the build prompt and parent, and kills the planner.
- **Checks:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` (also with `--features lua`) and `cargo test`. These need `../acc` checked out.
- **Manual:** run chameleon locally, press `F` on this repo with a small description, check the review overlay, then try revise, build and build with… against a second harness.
