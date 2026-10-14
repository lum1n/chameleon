//! Drives the app model with acc's recorded harness sessions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use acc_core::Harness;
use acc_core::item::ItemDetail;
use chameleon_app::config::Config;
use chameleon_app::keymap::{Key, KeyCode};
use chameleon_app::{App, AppEvent, Effect, ExecOutput, ExecTag, SessionKey};

const CLAUDE_HI: &str =
    include_str!("../../../../acc/crates/acc-core/tests/fixtures/claude/hi-and-controls.ndjson");
const CLAUDE_PERMISSION: &str =
    include_str!("../../../../acc/crates/acc-core/tests/fixtures/claude/permission-deny.ndjson");
const CLAUDE_SUBAGENTS: &str =
    include_str!("../../../../acc/crates/acc-core/tests/fixtures/claude/subagents.ndjson");
const CODEX_HANDSHAKE: &str =
    include_str!("../../../../acc/crates/acc-core/tests/fixtures/codex/handshake.ndjson");
const PI_TURN: &str =
    include_str!("../../../../acc/crates/acc-core/tests/fixtures/pi/rpc-turn-with-dialog.jsonl");

fn ok(stdout: &str) -> Result<ExecOutput, String> {
    Ok(ExecOutput {
        status: Some(0),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    })
}

fn app() -> App {
    let mut a = App::new(Config::default(), "test");
    a.set_entropy(|| [7; 15]);
    a.handle(
        AppEvent::Resize {
            width: 120,
            height: 40,
        },
        0,
    );
    a
}

fn ch(c: char) -> AppEvent {
    AppEvent::Key(Key::char(c))
}

fn press(code: KeyCode) -> AppEvent {
    AppEvent::Key(Key::new(code))
}

fn ctrl(c: char) -> AppEvent {
    AppEvent::Key(Key::ctrl(KeyCode::Char(c)))
}

/// Launches `harness` through the wizard and returns the session key.
fn launch(a: &mut App, harness: Harness, prompt: &str) -> (SessionKey, Vec<Effect>) {
    let mut fx = a.handle(ctrl('n'), 1);
    assert!(a.overlay().is_some(), "wizard opens");
    // Pick the harness: cycle until it matches.
    fx.extend(a.handle(press(KeyCode::Up), 1)); // to the harness field
    for _ in 0..8 {
        if matches!(a.overlay(), Some(chameleon_app::overlay::Overlay::Launch(w)) if w.harness == harness) {
            break;
        }
        a.handle(press(KeyCode::Right), 1);
    }
    a.handle(press(KeyCode::Down), 1); // cwd
    a.handle(ctrl('u'), 1);
    for c in "/work".chars() {
        a.handle(ch(c), 1);
    }
    a.handle(press(KeyCode::Down), 1); // model
    a.handle(press(KeyCode::Down), 1); // permission
    a.handle(press(KeyCode::Down), 1); // prompt
    for c in prompt.chars() {
        a.handle(ch(c), 1);
    }
    let fx2 = a.handle(press(KeyCode::Enter), 2);
    let resolve = fx2
        .iter()
        .find(|e| {
            matches!(
                e,
                Effect::Run {
                    tag: ExecTag::ResolveDir,
                    ..
                }
            )
        })
        .expect("resolves the directory first");
    let Effect::Run { host, .. } = resolve else {
        unreachable!()
    };
    let host = host.clone();
    let fx3 = a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::ResolveDir,
            result: ok("/work\n"),
        },
        3,
    );
    let launch = fx3
        .iter()
        .find_map(|e| match e {
            Effect::Run {
                tag: ExecTag::Launch(id),
                exec,
                ..
            } => Some((id.clone(), exec.clone())),
            _ => None,
        })
        .expect("launch effect");
    assert!(launch.1.script.contains("spec.json"));
    fx.extend(fx2);
    fx.extend(fx3);
    (SessionKey::new(&host, &launch.0), fx)
}

fn feed(a: &mut App, key: &SessionKey, log: &str, now: u64) -> Vec<Effect> {
    let mut fx = Vec::new();
    for line in log.lines() {
        fx.extend(a.handle(
            AppEvent::FeedLine {
                host: key.host.clone(),
                key: key.log_key(),
                bytes: format!("{line}\n").into_bytes(),
            },
            now,
        ));
    }
    fx
}

#[test]
fn starts_lists_and_probes_hosts() {
    let mut a = app();
    let fx = a.start(0);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Run {
            tag: ExecTag::List,
            ..
        }
    )));
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Run {
            tag: ExecTag::Probe,
            ..
        }
    )));
    assert_eq!(a.hosts().len(), 1);
    assert_eq!(a.hosts()[0].id(), "local");
}

#[test]
fn launches_claude_follows_the_log_and_renders_the_chat() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "hi");
    let fx = a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("4242\n"),
        },
        4,
    );
    let follows: Vec<&Effect> = fx.iter().filter(|e| matches!(e, Effect::Follow { .. })).collect();
    assert_eq!(follows.len(), 2, "log and exit file are followed");
    let input = fx.iter().find_map(|e| match e {
        Effect::Input { exec, .. } => Some(exec.clone()),
        _ => None,
    });
    let input = input.expect("the handshake and first prompt go out");
    let text = String::from_utf8_lossy(&input.stdin);
    assert!(text.contains("initialize") && text.contains("\"hi\""));

    let fx = feed(&mut a, &key, CLAUDE_HI, 5);
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::Hook { .. })),
        "no hooks are configured"
    );
    let slot = a.selected().expect("the new session is selected");
    let session = slot.session.as_ref().unwrap();
    assert!(session.transcript().items().iter().any(|i| i.text == "hi"));
    assert!(
        session
            .options()
            .iter()
            .any(|o| o.id == acc_core::command::OptionId::Model)
    );

    let screen = a.screen();
    let text: Vec<String> = screen.transcript.lines.iter().map(|l| l.text()).collect();
    assert!(text.iter().any(|l| l.contains("hi")), "{text:?}");
    let status = screen.status.text();
    assert!(status.contains("Claude Code"), "{status}");
    let sidebar = screen.sidebar.expect("sidebar is shown at 120 columns");
    assert!(sidebar.lines.iter().any(|l| l.text().contains("claude")));
    let path = sidebar.lines.iter().position(|l| l.text().trim() == "/work");
    assert!(path.is_some(), "the session's directory sits under it");
    assert_eq!(sidebar.hits[path.unwrap()], sidebar.hits[path.unwrap() - 1]);
}

fn transcript_text(a: &mut App) -> Vec<String> {
    a.screen().transcript.lines.iter().map(|l| l.text()).collect()
}

#[test]
fn a_subagent_opens_as_its_own_conversation_and_esc_goes_back() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "hi");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("4242\n"),
        },
        4,
    );
    feed(&mut a, &key, CLAUDE_SUBAGENTS, 5);

    // The subagent that ran `ls -la ...`: its command is distinctive enough
    // to use as a marker for whether its conversation is showing.
    let marker = "ls -la /tmp/acc-sub-spike";
    let (sub_id, sub_idx, items_len, n_agents) = {
        let session = a.selected().unwrap().session.as_ref().unwrap();
        let transcript = session.transcript();
        let cmd = transcript
            .items()
            .iter()
            .find(|i| {
                matches!(i.detail(Harness::Claude), ItemDetail::Command { command, .. } if command.contains(marker))
            })
            .expect("the subagent's ls command is in the transcript");
        let sub_id = cmd.parent.clone().expect("the command belongs to a subagent run");
        assert!(transcript.subagents().any(|s| s.id == sub_id));
        let sub_idx = transcript.items().iter().position(|i| i.id == sub_id).unwrap();
        (sub_id, sub_idx, transcript.len(), transcript.subagents().count())
    };

    // Main shows only main's own items.
    let main = transcript_text(&mut a);
    assert!(!main.iter().any(|l| l.contains(marker)), "{main:?}");

    // Put the cursor on the subagent's step; it offers to open.
    a.handle(AppEvent::Key(Key::alt(KeyCode::Char('2'))), 6);
    for _ in 0..items_len {
        if a.selected().and_then(|s| s.cursor) == Some(sub_idx) {
            break;
        }
        a.handle(ch('k'), 6);
    }
    assert_eq!(a.selected().and_then(|s| s.cursor), Some(sub_idx));
    assert!(transcript_text(&mut a).iter().any(|l| l.contains("⏎ open")));

    // Enter opens its conversation under a breadcrumb.
    a.handle(press(KeyCode::Enter), 7);
    assert_eq!(a.selected().unwrap().scope(), Some(sub_id.as_str()));
    let sub = transcript_text(&mut a);
    assert!(sub[0].starts_with("main › "), "{sub:?}");
    assert!(sub.iter().any(|l| l.contains(marker)), "{sub:?}");
    // j/k stay inside it.
    for _ in 0..items_len {
        a.handle(ch('j'), 7);
    }
    let c = a.selected().and_then(|s| s.cursor).unwrap();
    let session = a.selected().unwrap().session.as_ref().unwrap();
    assert_eq!(
        session.transcript().items()[c].parent.as_deref(),
        Some(sub_id.as_str())
    );

    // Esc goes back to main, the cursor where it was.
    a.handle(press(KeyCode::Esc), 8);
    assert_eq!(a.selected().unwrap().scope(), None);
    assert_eq!(a.selected().and_then(|s| s.cursor), Some(sub_idx));
    assert!(!transcript_text(&mut a).iter().any(|l| l.contains(marker)));
    assert_eq!(
        a.focus(),
        chameleon_app::Pane::Transcript,
        "esc only left the subagent"
    );

    // ] steps through every subagent and wraps around to main.
    let mut seen = Vec::new();
    for _ in 0..=n_agents {
        a.handle(ch(']'), 9);
        seen.push(a.selected().unwrap().scope().map(str::to_owned));
    }
    assert_eq!(seen.last(), Some(&None), "back to main after the last one");
    assert!(seen.contains(&Some(sub_id.clone())));
    a.handle(ch('['), 9);
    assert!(
        a.selected().unwrap().scope().is_some(),
        "[ goes to the last subagent"
    );

    // The picker lists main and every run, and opens the chosen one.
    a.handle(ch('a'), 10);
    let Some(chameleon_app::overlay::Overlay::Agents { selected, .. }) = a.overlay() else {
        panic!("the agents picker opens");
    };
    assert_ne!(*selected, 0, "it starts on the conversation shown");
    a.handle(ch('g'), 10);
    a.handle(press(KeyCode::Enter), 10);
    assert!(a.overlay().is_none());
    assert_eq!(a.selected().unwrap().scope(), None, "picked main");

    // A prompt goes to the main agent, so the view returns there.
    a.handle(ch(']'), 11);
    assert!(a.selected().unwrap().scope().is_some());
    a.handle(ch('i'), 11);
    a.handle(ch('x'), 11);
    let fx = a.handle(press(KeyCode::Enter), 11);
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Input { .. })),
        "the prompt is sent"
    );
    assert_eq!(a.selected().unwrap().scope(), None);
}

#[test]
fn several_questions_are_answered_one_by_one() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        4,
    );
    let ask = serde_json::json!({"type":"control_request","request_id":"q1","request":{
        "subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"toolu_q",
        "input":{"questions":[
            {"question":"Which DB?","header":"DB","multiSelect":false,
                "options":[{"label":"Postgres","description":""},{"label":"SQLite","description":""}]},
            {"question":"Which extras?","header":"Extras","multiSelect":true,
                "options":[{"label":"Cache","description":""},{"label":"Search","description":""}]}]}}});
    feed(&mut a, &key, &format!("{ask}\n"), 5);
    let card: Vec<String> = a.screen().transcript.lines.iter().map(|l| l.text()).collect();
    assert!(card.iter().any(|l| l.contains("+ 1 more questions")), "{card:?}");

    // Enter on the transcript opens the card the footer promises.
    a.handle(press(KeyCode::Esc), 6);
    a.handle(press(KeyCode::Enter), 6);
    let ov = a.screen().overlay.expect("question overlay");
    assert!(ov.lines.iter().any(|l| l.text().contains("DB · 1 of 2")));
    assert!(
        a.handle(ch('2'), 7).is_empty(),
        "the first answer waits for the rest"
    );
    let ov = a.screen().overlay.expect("still open for question two");
    assert!(ov.lines.iter().any(|l| l.text().contains("[ ] Cache")));
    a.handle(ch(' '), 7);
    a.handle(ch('j'), 7);
    a.handle(ch(' '), 7);
    let fx = a.handle(press(KeyCode::Enter), 8);
    assert!(a.overlay().is_none());
    let input = fx
        .iter()
        .find_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .expect("the answers are sent");
    assert!(input.contains(r#""Which DB?":"SQLite""#), "{input}");
    assert!(input.contains(r#""Which extras?":"Cache, Search""#), "{input}");
}

#[test]
fn permission_card_answers_and_clears() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        4,
    );
    // Feed up to (not including) the tool result, so the request stays open.
    let partial: String = CLAUDE_PERMISSION
        .lines()
        .take_while(|l| !l.contains("\"tool_result\""))
        .map(|l| format!("{l}\n"))
        .collect();
    let fx = feed(&mut a, &key, &partial, 5);
    assert!(fx.iter().all(|e| !matches!(e, Effect::Hook { .. })));
    let perms = a
        .selected()
        .unwrap()
        .session
        .as_ref()
        .unwrap()
        .permissions()
        .len();
    assert_eq!(perms, 1);
    let screen = a.screen();
    assert!(
        screen.status.text().contains("approval"),
        "{}",
        screen.status.text()
    );
    assert!(
        screen
            .transcript
            .lines
            .iter()
            .any(|l| l.text().contains("y approve"))
    );

    // `N` opens the card with the denial preselected; enter answers.
    a.handle(press(KeyCode::Esc), 6); // leave the composer
    a.handle(ch('N'), 6);
    assert!(matches!(
        a.overlay(),
        Some(chameleon_app::overlay::Overlay::Permission { .. })
    ));
    let ov = a.screen().overlay.expect("permission overlay");
    assert_eq!(ov.title, "approval");
    let fx = a.handle(press(KeyCode::Enter), 7);
    let input = fx
        .iter()
        .find_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .expect("the answer is written to the harness");
    assert!(input.contains("deny") || input.contains("behavior"), "{input}");
    assert!(
        a.selected()
            .unwrap()
            .session
            .as_ref()
            .unwrap()
            .permissions()
            .is_empty()
    );
    assert!(a.overlay().is_none());
}

#[test]
fn hooks_fire_for_live_permissions_but_not_replay() {
    let cfg = Config::parse(
        r#"
[[hooks]]
name = "notify"
on = ["permission"]
command = "true"
"#,
    )
    .unwrap();
    let mut a = App::new(cfg, "test");
    a.set_entropy(|| [1; 15]);
    a.handle(
        AppEvent::Resize {
            width: 100,
            height: 30,
        },
        0,
    );
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        1,
    );
    let partial: String = CLAUDE_PERMISSION
        .lines()
        .take_while(|l| !l.contains("\"tool_result\""))
        .map(|l| format!("{l}\n"))
        .collect();
    let fx = feed(&mut a, &key, &partial, 2);
    let hook = fx
        .iter()
        .find(|e| matches!(e, Effect::Hook { .. }))
        .expect("hook fires live");
    let Effect::Hook { env, payload, .. } = hook else {
        unreachable!()
    };
    assert!(env.iter().any(|(k, v)| k == "CHAM_EVENT" && v == "permission"));
    assert_eq!(payload["host"], "local");

    // The same log replayed through an attach fires nothing.
    let spec = a.selected().unwrap().spec.to_json();
    let mut b = App::new(
        Config::parse("[[hooks]]\nname='n'\non=['permission']\ncommand='true'").unwrap(),
        "two",
    );
    b.handle(
        AppEvent::Resize {
            width: 100,
            height: 30,
        },
        0,
    );
    let list = format!(
        "S\t{}\talive 1 {}\t1700000000\nJ\t{spec}\n",
        key.id,
        partial.len()
    );
    let fx = b.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::List,
            result: ok(&list),
        },
        3,
    );
    assert!(
        fx.iter().any(|e| matches!(
            e,
            Effect::Run {
                tag: ExecTag::Status(_),
                ..
            }
        )),
        "alive sessions attach automatically"
    );
    let fx = b.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::Status(key.id.clone()),
            result: ok(&format!("alive 1 {}\n", partial.len())),
        },
        4,
    );
    let follow = fx.iter().find_map(|e| match e {
        Effect::Follow { entry, .. } if entry.key.starts_with("l:") => Some(entry.offset),
        _ => None,
    });
    assert_eq!(follow, Some(0));
    let fx = feed(&mut b, &key, &partial, 5);
    assert!(
        fx.iter().all(|e| !matches!(e, Effect::Hook { .. })),
        "replay fires no hooks"
    );
    let slot = b.selected().unwrap();
    assert!(!slot.attention, "replay never asks for attention");
    assert_eq!(
        slot.session.as_ref().unwrap().permissions().len(),
        1,
        "the open request is visible"
    );
}

#[test]
fn codex_quota_shows_in_the_status_bar() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Codex, "go");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("9\n"),
        },
        1,
    );
    let fx = feed(&mut a, &key, CODEX_HANDSHAKE, 2);
    let sent: String = fx
        .iter()
        .filter_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .collect();
    assert!(
        sent.contains("turn/start"),
        "the first prompt waits for the thread: {sent}"
    );
    let session = a.selected().unwrap().session.as_ref().unwrap();
    if session.quota().is_some() {
        let status = a.screen().status.text();
        assert!(status.contains('%'), "{status}");
    }
}

#[test]
fn pi_select_dialog_denies_with_the_blocking_option() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Pi, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("9\n"),
        },
        1,
    );
    let mut asked = false;
    for line in PI_TURN.lines() {
        a.handle(
            AppEvent::FeedLine {
                host: key.host.clone(),
                key: key.log_key(),
                bytes: format!("{line}\n").into_bytes(),
            },
            2,
        );
        let s = a.selected().unwrap().session.as_ref().unwrap();
        if let Some(p) = s.permissions().first() {
            asked = true;
            assert!(p.options.iter().any(|o| o.label == "Block"), "{p:?}");
            a.handle(press(KeyCode::Esc), 3);
            a.handle(ch('N'), 3);
            let Some(chameleon_app::overlay::Overlay::Permission { .. }) = a.overlay() else {
                panic!("N opens the card");
            };
            let fx = a.handle(press(KeyCode::Enter), 4);
            let input = fx
                .iter()
                .find_map(|e| match e {
                    Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
                    _ => None,
                })
                .expect("the answer is sent");
            assert!(!input.contains("cancelled"), "{input}");
            assert!(input.contains("Block") || input.contains("\"1\""), "{input}");
            break;
        }
    }
    assert!(asked, "the fixture contains a dialog");
}

#[test]
fn exit_file_closes_the_session_and_unfollows_later() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        1,
    );
    feed(&mut a, &key, CLAUDE_HI, 2);
    a.handle(
        AppEvent::FeedLine {
            host: key.host.clone(),
            key: key.exit_key(),
            bytes: b"0\n".to_vec(),
        },
        3,
    );
    assert!(!a.selected().unwrap().is_alive());
    let fx = a.handle(AppEvent::Tick, 10_000);
    assert_eq!(
        fx.iter().filter(|e| matches!(e, Effect::Unfollow { .. })).count(),
        2
    );
    // Typing then goes nowhere, with a hint.
    a.handle(ch('x'), 10_001);
    let fx = a.handle(press(KeyCode::Enter), 10_002);
    assert!(fx.is_empty());
    assert!(
        a.screen()
            .toasts
            .iter()
            .any(|t| t.text().contains("isn't running"))
    );
}

#[test]
fn palette_runs_harness_commands_with_arguments() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        1,
    );
    feed(&mut a, &key, CLAUDE_HI, 2);
    let n = a.selected().unwrap().session.as_ref().unwrap().commands().len();
    assert!(n > 0, "the fixture reports slash commands");
    a.handle(ch('/'), 3);
    assert!(matches!(
        a.overlay(),
        Some(chameleon_app::overlay::Overlay::Palette { .. })
    ));
    let first = a.selected().unwrap().session.as_ref().unwrap().commands()[0]
        .name
        .clone();
    for c in format!("{first} some args").chars() {
        a.handle(ch(c), 3);
    }
    let fx = a.handle(press(KeyCode::Enter), 4);
    let input = fx
        .iter()
        .find_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .expect("the command is sent");
    assert!(input.contains(&first) && input.contains("some args"), "{input}");
}

#[test]
fn sidebar_collapses_and_the_header_keeps_the_tally() {
    let mut a = app();
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        1,
    );
    feed(&mut a, &key, CLAUDE_HI, 2);
    assert!(a.screen().sidebar.is_some());
    a.handle(press(KeyCode::Esc), 3); // leave the composer
    a.handle(ch('b'), 3);
    let screen = a.screen();
    assert!(screen.sidebar.is_none(), "b hides the list");
    assert!(!a.sidebar_shown());
    assert!(
        screen.header.text().contains("ctrl-b"),
        "{}",
        screen.header.text()
    );
    assert!(
        screen.transcript.rect.x <= 1,
        "the transcript takes the whole width: {:?}",
        screen.transcript.rect
    );
    // Tab cycles without landing on the hidden list; h brings it back.
    a.handle(press(KeyCode::Tab), 4);
    a.handle(press(KeyCode::Tab), 4);
    assert_ne!(a.focus(), chameleon_app::Pane::Sidebar);
    a.handle(press(KeyCode::Esc), 5);
    a.handle(ch('h'), 5);
    assert!(a.sidebar_shown());
    assert_eq!(a.focus(), chameleon_app::Pane::Sidebar);
    a.handle(ctrl('b'), 6);
    assert!(!a.sidebar_shown());
    assert_eq!(
        a.focus(),
        chameleon_app::Pane::Transcript,
        "focus leaves the hidden list"
    );
}

#[test]
fn keys_navigate_and_quit() {
    let mut a = app();
    let fx = a.handle(ch('q'), 1);
    assert_eq!(fx, vec![Effect::Quit]);
    a.handle(ch('?'), 2);
    assert!(a.screen().overlay.is_some_and(|o| o.title == "help"));
    a.handle(press(KeyCode::Esc), 3);
    assert!(a.overlay().is_none());
}

/// A live Claude session fed the recorded handshake, for palette tests.
fn claude_ready(a: &mut App) -> SessionKey {
    let (key, _) = launch(a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        1,
    );
    feed(a, &key, CLAUDE_HI, 2);
    key
}

fn palette(a: &mut App, text: &str) -> Vec<Effect> {
    a.handle(ch('/'), 3);
    for c in text.chars() {
        a.handle(ch(c), 3);
    }
    a.handle(press(KeyCode::Enter), 4)
}

/// What the effects write to the harness, as JSON lines.
fn written(fx: &[Effect]) -> Vec<serde_json::Value> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .flat_map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn option_commands_open_a_picker() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    let _key = claude_ready(&mut a);
    let fx = palette(&mut a, "mode");
    assert!(
        written(&fx).is_empty(),
        "nothing is sent before a value is picked"
    );
    let Some(Overlay::Pick { choices, command, .. }) = a.overlay() else {
        panic!("expected a picker, got {:?}", a.overlay());
    };
    assert_eq!(command, "mode");
    let plan = choices
        .iter()
        .position(|c| c.value == "plan")
        .expect("plan mode is offered");
    let at = match a.overlay() {
        Some(Overlay::Pick { selected, .. }) => *selected,
        _ => 0,
    };
    for _ in 0..(plan + choices.len() - at) % choices.len() {
        a.handle(press(KeyCode::Down), 5);
    }
    let fx = a.handle(press(KeyCode::Enter), 6);
    let sent = written(&fx);
    assert_eq!(sent[0]["request"]["subtype"], "set_permission_mode");
    assert_eq!(sent[0]["request"]["mode"], "plan");
    assert!(a.overlay().is_none());

    // A typed value skips the picker.
    let fx = palette(&mut a, "mode acceptEdits");
    assert_eq!(written(&fx)[0]["request"]["mode"], "acceptEdits");
}

#[test]
fn panels_open_refresh_and_run_actions() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    let key = claude_ready(&mut a);
    let fx = palette(&mut a, "mcp");
    let req = written(&fx).remove(0);
    assert_eq!(req["request"]["subtype"], "mcp_status");
    let reply = |id: &serde_json::Value, status: &str| {
        serde_json::json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
            "response": {"mcpServers": [{"name": "sentry", "status": status, "scope": "user", "config": {"type": "http"}}]}}})
        .to_string()
    };
    feed(&mut a, &key, &reply(&req["request_id"], "failed"), 5);
    assert!(matches!(a.overlay(), Some(Overlay::Panel { id, .. }) if id == "mcp"));
    let shown = format!("{:?}", a.screen());
    assert!(
        shown.contains("sentry") && shown.contains("Reconnect"),
        "the panel is drawn"
    );

    // Enter runs the first action; the panel stays open for the refresh.
    let fx = a.handle(press(KeyCode::Enter), 6);
    let sent = written(&fx);
    assert_eq!(sent[0]["request"]["subtype"], "mcp_reconnect");
    assert_eq!(sent[0]["request"]["serverName"], "sentry");
    assert!(matches!(a.overlay(), Some(Overlay::Panel { .. })));
    let ack = serde_json::json!({"type": "control_response", "response": {"subtype": "success", "request_id": sent[0]["request_id"], "response": {}}}).to_string();
    let fx = feed(&mut a, &key, &ack, 7);
    let again = written(&fx).remove(0);
    assert_eq!(again["request"]["subtype"], "mcp_status");
    feed(&mut a, &key, &reply(&again["request_id"], "connected"), 8);
    let slot = a.selected().unwrap();
    assert_eq!(
        slot.panels["mcp"].sections[0].rows[0].value.as_deref(),
        Some("connected")
    );
    a.handle(press(KeyCode::Esc), 9);
    assert!(a.overlay().is_none());
}

#[test]
fn terminal_only_commands_open_the_harness_tui() {
    let mut a = app();
    claude_ready(&mut a);
    let fx = palette(&mut a, "doctor");
    assert!(written(&fx).is_empty());
    let argv = fx
        .iter()
        .find_map(|e| match e {
            Effect::Terminal { argv } => Some(argv.clone()),
            _ => None,
        })
        .expect("opens the TUI");
    assert_eq!(argv[..2], ["sh", "-c"]);
    assert!(
        argv[2].starts_with("cd /work && exec ") && argv[2].contains("claude"),
        "{argv:?}"
    );
}

#[test]
fn a_missing_argument_is_reported_not_sent() {
    let mut a = app();
    claude_ready(&mut a);
    let fx = palette(&mut a, "mcp reconnect");
    assert!(written(&fx).is_empty());
    assert!(a.overlay().is_none());
    assert!(format!("{:?}", a.screen()).contains("MCP server name"));
}

#[test]
fn a_feed_stuck_behind_the_log_is_restarted_once() {
    let mut a = app();
    let key = claude_ready(&mut a);
    let spec = a.selected().unwrap().spec.to_json();
    let fed = CLAUDE_HI.len() as u64 + u64::from(!CLAUDE_HI.ends_with('\n'));
    let list = |size: u64| {
        let r = ok(&format!("S\t{}\talive 1 {size}\t1700000000\nJ\t{spec}\n", key.id));
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::List,
            result: r,
        }
    };
    let refed = |fx: &[Effect]| fx.iter().any(|e| matches!(e, Effect::Refeed { .. }));

    // Caught up: nothing to do.
    assert!(!refed(&a.handle(list(fed), 10)));
    // Behind once may just be a listing racing the feed.
    assert!(!refed(&a.handle(list(fed + 500), 20)));
    // Still at the same offset a listing later: the feed has stalled.
    assert!(refed(&a.handle(list(fed + 900), 30)));
    // Not again for the same offset (a log ending mid-line stays "behind").
    assert!(!refed(&a.handle(list(fed + 900), 40)));
    assert!(!refed(&a.handle(list(fed + 900), 50)));

    // Lines arrive, then it sticks at the new offset: restarted again.
    feed(&mut a, &key, r#"{"type":"keep_alive"}"#, 60);
    let fed = fed + r#"{"type":"keep_alive"}"#.len() as u64 + 1;
    assert!(!refed(&a.handle(list(fed + 10), 70)));
    assert!(refed(&a.handle(list(fed + 10), 80)));
}

/// Claude editing `/work/src/lib.rs`, as stream-json.
const CLAUDE_EDIT: &str = concat!(
    r#"{"type":"assistant","message":{"id":"m_e","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_e1","name":"Edit","input":{"file_path":"/work/src/lib.rs","old_string":"fn a() {}\nfn b() {}","new_string":"fn a() {}\nfn c() {}"}}]},"parent_tool_use_id":null,"session_id":"s"}"#,
    "\n",
    r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_e1","type":"tool_result","content":"ok","is_error":false}]},"parent_tool_use_id":null,"session_id":"s"}"#,
    "\n",
);

const GIT_DIFF: &str = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1..2 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,3 +10,3 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n x\n\
diff --git a/notes.md b/notes.md\nnew file mode 100644\n--- /dev/null\n+++ b/notes.md\n@@ -0,0 +1,2 @@\n+# notes\n+todo\n";

fn overlay_text(a: &mut App) -> Vec<String> {
    a.screen()
        .overlay
        .map(|o| o.lines.iter().map(|l| l.text()).collect())
        .unwrap_or_default()
}

fn diff_view(a: &App) -> &chameleon_app::diff::DiffView {
    match a.overlay() {
        Some(chameleon_app::overlay::Overlay::Diff(v)) => v,
        other => panic!("the diff viewer is open, not {other:?}"),
    }
}

#[test]
fn the_diff_viewer_shows_the_repository_and_the_sessions_edits() {
    use chameleon_app::diff::DiffSource;
    let mut a = app();
    let key = claude_ready(&mut a);
    feed(&mut a, &key, CLAUDE_EDIT, 3);

    // Opened anywhere but on an edit: the repository's changes, fetched on the host.
    let fx = a.handle(ctrl('d'), 4);
    let script = fx
        .iter()
        .find_map(|e| match e {
            Effect::Run {
                tag: ExecTag::GitDiff(id),
                exec,
                ..
            } if *id == key.id => Some(exec.script.clone()),
            _ => None,
        })
        .expect("asks the host for the git diff");
    assert!(script.contains("cd /work") && script.contains("ls-files --others"));
    assert_eq!(diff_view(&a).source, DiffSource::Git);
    assert!(
        overlay_text(&mut a)
            .iter()
            .any(|l| l.contains("reading git diff"))
    );

    let done = |result| AppEvent::ExecDone {
        host: key.host.clone(),
        tag: ExecTag::GitDiff(key.id.clone()),
        result,
    };
    a.handle(done(ok(GIT_DIFF)), 5);
    let text = overlay_text(&mut a);
    let joined = text.join("\n");
    assert!(joined.contains("❯± src/lib.rs"), "{joined}");
    assert!(joined.contains("+ notes.md"), "{joined}");
    assert!(joined.contains("@@ -10,3 +10,3 @@"), "{joined}");
    // Old and new line numbers beside each line.
    assert!(text.iter().any(|l| l.contains("11    - fn b() {}")), "{joined}");
    assert!(text.iter().any(|l| l.contains("   11 + fn c() {}")), "{joined}");

    // n moves to the next file.
    a.handle(ch('n'), 6);
    assert_eq!(diff_view(&a).file, 1);
    assert!(overlay_text(&mut a).iter().any(|l| l.contains("2 + todo")));
    // ] past the last hunk of the last file stays put; n wraps around.
    a.handle(ch(']'), 6);
    assert_eq!(diff_view(&a).file, 1);
    a.handle(ch('n'), 6);
    assert_eq!(diff_view(&a).file, 0);

    // s: this session's edits, merged per file, paths relative to the session.
    a.handle(ch('s'), 7);
    assert_eq!(diff_view(&a).source, DiffSource::Session);
    let joined = overlay_text(&mut a).join("\n");
    assert!(
        joined.contains("src/lib.rs") && !joined.contains("/work/src"),
        "{joined}"
    );
    assert!(
        joined.contains("+ fn c() {}") && !joined.contains("notes.md"),
        "{joined}"
    );

    // r reloads the repository's side.
    let fx = a.handle(ch('r'), 8);
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Run {
            tag: ExecTag::GitDiff(_),
            ..
        }
    )));
    a.handle(press(KeyCode::Esc), 8);
    assert!(a.overlay().is_none());

    // Not a git repository: straight to the session's edits.
    a.handle(ctrl('d'), 9);
    a.handle(done(Err("not a git repository".into())), 9);
    assert_eq!(diff_view(&a).source, DiffSource::Session);
    a.handle(ch('s'), 9);
    assert!(
        overlay_text(&mut a)
            .iter()
            .any(|l| l.contains("git: not a git repository"))
    );
}

#[test]
fn the_wizard_offers_the_models_a_session_reported() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    claude_ready(&mut a);
    let models = a
        .selected()
        .unwrap()
        .session
        .as_ref()
        .unwrap()
        .options()
        .iter()
        .find(|o| o.id == acc_core::command::OptionId::Model)
        .map(|o| o.values.clone())
        .unwrap();
    assert!(models.len() > 1, "{models:?}");
    a.handle(ctrl('n'), 5);
    a.handle(press(KeyCode::Down), 5); // model
    a.handle(press(KeyCode::Right), 5);
    a.handle(press(KeyCode::Right), 5);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.fields[1], models[1]);
    a.handle(press(KeyCode::Left), 5);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.fields[1], models[0]);
    let text = overlay_text(&mut a).join("\n");
    assert!(text.contains("ctrl-h/l") && text.contains(&models[0]), "{text}");

    // Another harness has no models here, so the pick is dropped.
    a.handle(press(KeyCode::Up), 5); // directory
    a.handle(press(KeyCode::Up), 5); // harness
    a.handle(press(KeyCode::Right), 5);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_ne!(w.harness, Harness::Claude);
    assert!(w.fields[1].is_empty());
}

#[test]
fn the_wizard_completes_directories_from_the_host() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    let dir = |a: &App| match a.overlay() {
        Some(Overlay::Launch(w)) => w.fields[0].clone(),
        _ => panic!("expected the wizard"),
    };
    let listed = |fx: &[Effect]| {
        fx.iter().find_map(|e| match e {
            Effect::Run {
                tag: ExecTag::ListDirs(p),
                ..
            } => Some(p.clone()),
            _ => None,
        })
    };
    let answer = |a: &mut App, parent: &str, out: &str| {
        a.handle(
            AppEvent::ExecDone {
                host: "local".into(),
                tag: ExecTag::ListDirs(parent.into()),
                result: ok(out),
            },
            1,
        );
    };
    a.handle(ctrl('n'), 1);
    a.handle(ctrl('u'), 1);
    let mut fx = Vec::new();
    for c in "~/re".chars() {
        fx.extend(a.handle(ch(c), 1));
    }
    assert_eq!(listed(&fx).as_deref(), Some("~/"));
    answer(&mut a, "~/", "repos\nrecipes\n.config\nDocuments\n");

    // Two match: tab takes their common start, then highlights the first.
    a.handle(press(KeyCode::Tab), 1);
    assert_eq!(dir(&a), "~/re");
    let text = overlay_text(&mut a).join("\n");
    assert!(
        text.contains("repos/") && text.contains("recipes/") && !text.contains(".config"),
        "{text}"
    );
    a.handle(press(KeyCode::Down), 1);
    a.handle(press(KeyCode::Down), 1);
    let fx = a.handle(press(KeyCode::Enter), 1);
    assert_eq!(dir(&a), "~/repos/");
    assert_eq!(listed(&fx).as_deref(), Some("~/repos/"));
    answer(&mut a, "~/repos/", "chameleon\n");

    // One match: tab completes it.
    a.handle(ch('c'), 1);
    a.handle(press(KeyCode::Tab), 1);
    assert_eq!(dir(&a), "~/repos/chameleon/");

    // Nothing to complete: tab moves on to the model.
    answer(&mut a, "~/repos/chameleon/", "");
    a.handle(press(KeyCode::Tab), 1);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.field, 3);
}

#[test]
fn vim_keys_move_through_the_wizard_and_the_palette() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    claude_ready(&mut a);
    let field = |a: &App| match a.overlay() {
        Some(Overlay::Launch(w)) => (w.field, w.harness),
        _ => panic!("expected the wizard"),
    };
    a.handle(ctrl('n'), 5);
    assert_eq!(field(&a).0, 2, "starts on the directory");
    // ctrl-k / ctrl-j move while a text field has focus; typed j stays text.
    a.handle(ctrl('k'), 5);
    assert_eq!(field(&a).0, 1);
    // The harness row takes no text: plain l and k work there.
    let before = field(&a).1;
    a.handle(ch('l'), 5);
    assert_ne!(field(&a).1, before);
    a.handle(ch('h'), 5);
    assert_eq!(field(&a).1, before);
    a.handle(ch('k'), 5);
    assert_eq!(field(&a).0, 0);
    a.handle(ch('j'), 5);
    a.handle(ctrl('j'), 5);
    assert_eq!(field(&a).0, 2);
    a.handle(ctrl('u'), 5);
    a.handle(ch('j'), 5);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.fields[0], "j");
    a.handle(press(KeyCode::Esc), 5);

    // The palette filters on j and k; ctrl-j / ctrl-k move.
    a.handle(ctrl('p'), 6);
    a.handle(ctrl('j'), 6);
    a.handle(ctrl('j'), 6);
    a.handle(ctrl('k'), 6);
    let Some(Overlay::Palette { selected, query }) = a.overlay() else {
        panic!("expected the palette");
    };
    assert_eq!((*selected, query.as_str()), (1, ""));
    a.handle(ch('k'), 6);
    let Some(Overlay::Palette { query, .. }) = a.overlay() else {
        panic!("expected the palette");
    };
    assert_eq!(query, "k");
}
