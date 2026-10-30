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
    a.handle(press(KeyCode::Down), 1); // worktree
    a.handle(press(KeyCode::Down), 1); // sandbox
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
        fx.extend(feed_line(a, key, line, now));
    }
    fx
}

/// Feeds one log line, continuing from where the session has read to.
fn feed_line(a: &mut App, key: &SessionKey, line: &str, now: u64) -> Vec<Effect> {
    let read = a
        .slots()
        .iter()
        .find(|s| s.key == *key)
        .and_then(|s| s.session.as_ref())
        .map_or(0, |s| s.offset());
    let bytes = format!("{line}\n").into_bytes();
    let offset = read + bytes.len() as u64;
    a.handle(
        AppEvent::FeedLine {
            host: key.host.clone(),
            key: key.log_key(),
            bytes,
            offset,
        },
        now,
    )
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
    let path = sidebar
        .lines
        .iter()
        .position(|l| l.text().trim_start_matches('▌').trim() == "/work");
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
            .find(|i| matches!(&i.detail, ItemDetail::Command { command, .. } if command.contains(marker)))
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
    assert!(
        card.iter().any(|l| l.contains("Extras  Which extras?")),
        "{card:?}"
    );
    assert!(
        card.iter().any(|l| l.contains("Cache · Search · pick any")),
        "{card:?}"
    );
    assert!(
        !card.iter().any(|l| l.contains("questions:")),
        "no JSON: {card:?}"
    );

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
fn one_question_shows_its_options_not_its_json() {
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
                "options":[{"label":"Postgres","description":"the big one"},{"label":"SQLite","description":""}]}]}}});
    feed(&mut a, &key, &format!("{ask}\n"), 5);
    let card: Vec<String> = a.screen().transcript.lines.iter().map(|l| l.text()).collect();
    assert!(card.iter().any(|l| l.contains("Which DB?")), "{card:?}");
    assert!(
        !card.iter().any(|l| l.contains("questions:")),
        "no JSON: {card:?}"
    );

    a.handle(press(KeyCode::Esc), 6);
    a.handle(press(KeyCode::Enter), 6);
    let ov: Vec<String> = a
        .screen()
        .overlay
        .expect("question overlay")
        .lines
        .iter()
        .map(|l| l.text())
        .collect();
    assert!(ov.iter().any(|l| l.contains("Postgres  the big one")), "{ov:?}");
    assert!(!ov.iter().any(|l| l.contains("questions:")), "no JSON: {ov:?}");
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
fn the_header_shows_a_sandbox_the_spec_has_lost() {
    // A client that predates sandboxes rewrote the spec without one; the
    // host's status still says the harness runs in Seatbelt.
    let mut a = app();
    let id = "claude-abcdefghijklmnopqrstuvwx";
    let spec = format!(
        "{{\"v\":1,\"id\":\"{id}\",\"harness\":\"claude\",\"cwd\":\"/w\",\
         \"launch\":{{\"program\":\"claude\",\"args\":[],\"env\":[],\
         \"protocol\":\"claude_stream_json\"}},\"created_ms\":1}}"
    );
    a.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::List,
            result: ok(&format!("S\t{id}\talive 1 0\t1700000000\nJ\t{spec}\n")),
        },
        3,
    );
    assert!(!a.screen().header.text().contains("sandboxed"));
    a.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::Status(id.into()),
            result: ok("alive 1 0\nB seatbelt\n"),
        },
        4,
    );
    let header = a.screen().header.text();
    assert!(header.contains("sandboxed"), "{header}");
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
        feed_line(&mut a, &key, line, 2);
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
            offset: 2,
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
fn the_diff_viewer_shows_codexs_turn_diff() {
    use chameleon_app::diff::DiffSource;
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
    feed(&mut a, &key, CODEX_HANDSHAKE, 2);

    // Before the harness diffs a turn there are two sources, as on any harness.
    a.handle(ctrl('d'), 3);
    a.handle(ch('s'), 3);
    a.handle(ch('s'), 3);
    assert_eq!(diff_view(&a).source, DiffSource::Git);
    assert!(!overlay_text(&mut a).join("\n").contains(" turn "));
    a.handle(press(KeyCode::Esc), 3);

    // A turn diff (a shell command's change too, which no edit item has).
    let diff = serde_json::to_string(GIT_DIFF).unwrap();
    let line = format!(
        r#"{{"method":"turn/diff/updated","params":{{"threadId":"01a125ba-bc10-7a62-8401-c7b0088728d3","turnId":"t1","diff":{diff}}}}}"#
    );
    feed_line(&mut a, &key, &line, 4);
    a.handle(ctrl('d'), 5);
    a.handle(ch('s'), 5);
    a.handle(ch('s'), 5);
    assert_eq!(diff_view(&a).source, DiffSource::Turn);
    let joined = overlay_text(&mut a).join("\n");
    assert!(joined.contains(" turn "), "{joined}");
    assert!(
        joined.contains("src/lib.rs") && joined.contains("notes.md"),
        "{joined}"
    );
    // s comes back round to git; shift-tab goes the other way.
    a.handle(ch('s'), 6);
    assert_eq!(diff_view(&a).source, DiffSource::Git);
    a.handle(press(KeyCode::BackTab), 6);
    assert_eq!(diff_view(&a).source, DiffSource::Turn);

    // A newer diff replaces it: here, the turn changed nothing.
    let line = r#"{"method":"turn/diff/updated","params":{"threadId":"01a125ba-bc10-7a62-8401-c7b0088728d3","turnId":"t2","diff":""}}"#;
    feed_line(&mut a, &key, line, 7);
    assert!(
        overlay_text(&mut a)
            .iter()
            .any(|l| l.contains("the last turn changed nothing"))
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
    a.handle(press(KeyCode::Down), 5); // worktree
    a.handle(press(KeyCode::Down), 5); // sandbox
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
    a.handle(press(KeyCode::Up), 5); // sandbox
    a.handle(press(KeyCode::Up), 5); // worktree
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
    use chameleon_app::overlay::{Overlay, Wizard};
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

    // Nothing to complete: tab moves on to the worktree choice.
    answer(&mut a, "~/repos/chameleon/", "");
    a.handle(press(KeyCode::Tab), 1);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.field, Wizard::WORKTREE);
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

#[test]
fn quota_windows_get_meters_like_the_context() {
    let mut a = app();
    claude_ready(&mut a);
    let windows = a
        .selected()
        .and_then(|s| s.session.as_ref())
        .and_then(|s| s.quota())
        .map_or(0, |q| {
            q.windows.iter().filter(|w| w.used_percent.is_some()).count()
        });
    assert!(windows > 0, "the fixture reports quota");
    let cells = |a: &mut App, width: u16| {
        a.handle(AppEvent::Resize { width, height: 40 }, 9);
        let status = a.screen().status.text();
        (status.matches('▰').count() + status.matches('▱').count(), status)
    };
    // Wide: six cells for the context and for every window.
    let (n, status) = cells(&mut a, 200);
    assert_eq!(n, 6 * (windows + 1), "{status}");
    // Narrow: bare percentages rather than a clipped window.
    let (n, status) = cells(&mut a, 90);
    assert_eq!(n, 0, "{status}");
    assert!(status.contains("7d"), "{status}");
}

#[test]
fn lines_handed_out_again_are_not_fed_twice() {
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
    fn session(a: &App) -> &acc_core::session::Session {
        a.selected().unwrap().session.as_ref().unwrap()
    }
    let start = session(&a).offset();
    let mut lines = Vec::new();
    let mut offset = start;
    for line in CLAUDE_HI.lines() {
        let bytes = format!("{line}\n").into_bytes();
        offset += bytes.len() as u64;
        lines.push((bytes, offset));
    }
    let send = |a: &mut App, (bytes, offset): &(Vec<u8>, u64)| {
        a.handle(
            AppEvent::FeedLine {
                host: key.host.clone(),
                key: key.log_key(),
                bytes: bytes.clone(),
                offset: *offset,
            },
            2,
        );
    };
    for l in &lines {
        send(&mut a, l);
    }
    let once = format!("{:?}", session(&a).transcript().items());
    // A restarted feed hands the same lines out again.
    for l in &lines {
        send(&mut a, l);
    }
    assert_eq!(session(&a).offset(), offset);
    assert_eq!(format!("{:?}", session(&a).transcript().items()), once);
}

fn app_with(toml: &str) -> App {
    let mut a = App::new(Config::parse(toml).expect("config parses"), "test");
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

/// The first script run with `tag`, and its host.
fn ran(fx: &[Effect], tag: impl Fn(&ExecTag) -> bool) -> Option<(String, String)> {
    fx.iter().find_map(|e| match e {
        Effect::Run {
            host, exec, tag: t, ..
        } if tag(t) => Some((host.clone(), exec.script.clone())),
        _ => None,
    })
}

#[test]
fn a_worktree_session_launches_in_it_diffs_it_and_merges_it() {
    use chameleon_app::overlay::{Overlay, Wizard};
    let mut a = app();
    a.handle(ctrl('n'), 1);
    a.handle(ctrl('u'), 1);
    for c in "/work".chars() {
        a.handle(ch(c), 1);
    }
    a.handle(press(KeyCode::Down), 1);
    a.handle(ch('l'), 1);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!((w.field, w.worktree), (Wizard::WORKTREE, true));
    let fx = a.handle(AppEvent::Key(Key::ctrl(KeyCode::Enter)), 2);
    let (host, _) = ran(&fx, |t| *t == ExecTag::ResolveDir).expect("resolves the directory");

    // The worktree is made where the directory resolved, named for the session.
    let fx = a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::ResolveDir,
            result: ok("/work\n"),
        },
        3,
    );
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Run {
                tag: ExecTag::AddWorktree(id),
                exec,
                ..
            } => {
                assert!(exec.script.contains("worktree add"), "{}", exec.script);
                assert!(exec.script.contains("cd /work "), "{}", exec.script);
                Some(id.clone())
            }
            _ => None,
        })
        .expect("makes the worktree first");
    assert!(id.starts_with("claude-"), "{id}");
    let fx = a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::AddWorktree(id.clone()),
            result: ok(&format!("/home/u/.acc/w/{id}\n")),
        },
        4,
    );
    let (_, script) = ran(&fx, |t| *t == ExecTag::Launch(id.clone())).expect("then launches");
    assert!(script.contains(&format!(".acc/w/{id}")), "{script}");
    let key = SessionKey::new(&host, &id);
    let slot = a.slots().iter().find(|s| s.key == key).unwrap();
    assert_eq!(slot.spec.worktree.as_deref(), Some(id.as_str()));
    let header = a.screen().header.text();
    assert!(header.contains(&format!("⎇ acc/{id}")), "{header}");

    // The diff is the worktree's, since it was made.
    a.handle(press(KeyCode::Esc), 5);
    a.handle(press(KeyCode::Esc), 5);
    let fx = a.handle(ch('d'), 5);
    let (_, script) = ran(&fx, |t| matches!(t, ExecTag::GitDiff(_))).expect("reads the diff");
    assert!(script.contains(&format!(".acc/w/{id}")), "{script}");
    assert!(diff_view(&a).worktree);

    // M asks, y merges; a conflict is said as such.
    a.handle(ch('M'), 6);
    assert!(matches!(a.overlay(), Some(Overlay::Confirm { .. })));
    let fx = a.handle(ch('y'), 6);
    let (_, script) = ran(&fx, |t| matches!(t, ExecTag::Merge(_))).expect("merges");
    assert!(script.contains("--squash"), "{script}");
    a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::Merge(id.clone()),
            result: Ok(ExecOutput {
                status: Some(acc_core::worktree::EXIT_CONFLICT as i32),
                ..ExecOutput::default()
            }),
        },
        7,
    );
    let toasts: Vec<String> = a.screen().toasts.iter().map(|l| l.text()).collect();
    assert!(toasts.iter().any(|t| t.contains("conflict")), "{toasts:?}");

    // Sessions without a worktree have nothing to merge.
    a.set_entropy(|| [9; 15]);
    let plain = claude_ready(&mut a);
    a.handle(press(KeyCode::Esc), 8);
    assert_eq!(a.selected().unwrap().key, plain);
    a.handle(ch('M'), 8);
    assert!(a.overlay().is_none(), "{:?}", a.overlay());
}

#[test]
fn handoff_starts_another_harness_primed_with_the_conversation() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    let key = claude_ready(&mut a);
    a.handle(press(KeyCode::Esc), 3);
    a.handle(ch('H'), 3);
    let Some(Overlay::Handoff { harnesses, .. }) = a.overlay() else {
        panic!("expected the handoff picker, not {:?}", a.overlay());
    };
    assert!(!harnesses.contains(&Harness::Claude));
    let to = harnesses[0];
    let fx = a.handle(press(KeyCode::Enter), 4);
    assert!(
        ran(&fx, |t| matches!(t, ExecTag::Launch(_))).is_some(),
        "launches the other"
    );
    let new = a.selected().unwrap();
    // The primer is the first prompt, sent once the harness can take it.
    let snap = new.session.as_ref().and_then(|s| s.snapshot()).unwrap();
    assert!(
        snap.contains("This conversation started in Claude Code"),
        "{snap}"
    );
    assert_eq!(new.harness(), to);
    assert_eq!(new.spec.parent.as_deref(), Some(key.id.as_str()));
    assert!(
        a.slots().iter().any(|s| s.key == key),
        "the first one keeps running"
    );
}

#[test]
fn the_policy_answers_matching_approvals_and_says_so() {
    let mut a = app_with("[[policy]]\ndecision = 'deny'\n");
    let (key, _) = launch(&mut a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        4,
    );
    let partial: String = CLAUDE_PERMISSION
        .lines()
        .take_while(|l| !l.contains("\"tool_result\""))
        .map(|l| format!("{l}\n"))
        .collect();
    let fx = feed(&mut a, &key, &partial, 5);
    let sent = written(&fx);
    assert!(
        sent.iter()
            .any(|v| v.to_string().contains("\"behavior\":\"deny\"")),
        "{sent:?}"
    );
    let session = a.selected().unwrap().session.as_ref().unwrap();
    assert!(session.permissions().is_empty());
    let toasts: Vec<String> = a.screen().toasts.iter().map(|l| l.text()).collect();
    assert!(
        toasts.iter().any(|t| t.starts_with("Denied by policy")),
        "{toasts:?}"
    );
}

/// A Claude session waiting on approval of `command` (the fixture's own
/// is `touch acc-probe.txt`).
fn claude_asking(a: &mut App, command: &str) -> (SessionKey, Vec<Effect>) {
    let (key, _) = launch(a, Harness::Claude, "");
    a.handle(
        AppEvent::ExecDone {
            host: key.host.clone(),
            tag: ExecTag::Launch(key.id.clone()),
            result: ok("1\n"),
        },
        4,
    );
    let partial: String = CLAUDE_PERMISSION
        .lines()
        .take_while(|l| !l.contains("\"tool_result\""))
        .map(|l| format!("{}\n", l.replace("touch acc-probe.txt", command)))
        .collect();
    let fx = feed(a, &key, &partial, 5);
    (key, fx)
}

fn toasts(a: &mut App) -> Vec<String> {
    a.screen().toasts.iter().map(|l| l.text()).collect()
}

#[test]
fn auto_approve_allows_what_isnt_dangerous() {
    let mut a = app_with("[auto_approve]\nenabled = true\n");
    let (_, fx) = claude_asking(&mut a, "touch acc-probe.txt");
    let sent = written(&fx);
    assert!(
        sent.iter()
            .any(|v| v.to_string().contains("\"behavior\":\"allow\"")),
        "{sent:?}"
    );
    let session = a.selected().unwrap().session.as_ref().unwrap();
    assert!(session.permissions().is_empty());
    let t = toasts(&mut a);
    assert!(t.iter().any(|t| t.starts_with("auto-approved")), "{t:?}");
}

#[test]
fn auto_approve_leaves_dangerous_requests_to_you() {
    let mut a = app_with("[auto_approve]\nenabled = true\n");
    let (_, fx) = claude_asking(&mut a, "rm -rf acc-probe.txt");
    assert!(written(&fx).is_empty());
    let session = a.selected().unwrap().session.as_ref().unwrap();
    assert_eq!(session.permissions().len(), 1);
    let t = toasts(&mut a);
    assert!(t.iter().any(|t| t.contains("`rm` is dangerous")), "{t:?}");
}

#[test]
fn turning_auto_approve_on_answers_what_waits() {
    let mut a = app();
    let (_, fx) = claude_asking(&mut a, "touch acc-probe.txt");
    assert!(written(&fx).is_empty(), "off by default");
    a.handle(press(KeyCode::Esc), 6);
    let fx = a.handle(ch('A'), 6);
    assert!(
        written(&fx)
            .iter()
            .any(|v| v.to_string().contains("\"behavior\":\"allow\"")),
        "{fx:?}"
    );
    assert!(a.screen().header.text().contains("auto"));
    a.handle(ch('A'), 7);
    assert!(!a.screen().header.text().contains("auto"));
}

#[test]
fn usage_adds_up_and_is_saved_on_quit() {
    use chameleon_app::overlay::Overlay;
    let mut a = app();
    claude_ready(&mut a);
    a.handle(press(KeyCode::Esc), 3);
    a.handle(ch('U'), 3);
    assert!(matches!(a.overlay(), Some(Overlay::Usage { .. })));
    let text = overlay_text(&mut a).join("\n");
    assert!(
        text.contains("all sessions") && text.contains("Claude Code"),
        "{text}"
    );
    a.handle(press(KeyCode::Esc), 3);
    let fx = a.handle(ch('q'), 4);
    let json = fx
        .iter()
        .find_map(|e| match e {
            Effect::SaveUsage { json } => Some(json.clone()),
            _ => None,
        })
        .expect("saves before quitting");
    assert!(matches!(fx.last(), Some(Effect::Quit)));

    // The next start shows it before any session reports again.
    let mut b = app();
    b.load_usage(&json);
    assert_eq!(b.usage_records(), a.usage_records());
}

const SKILLS_LISTING: &str = "store\t/h/.agents/skills\nskill\ttdd\nf\tname: tdd\nf\tdescription: Test first\n\
place\tclaude-code\t/h/.claude/skills\town\nlink\tclaude-code\ttdd\t/h/.agents/skills/tdd\n\
place\tcodex\t/h/.codex/skills\town\nlink\tcodex\ttdd\t/elsewhere/tdd\nown\tcodex\tmine\n\
place\tpi\t/h/.pi/agent/skills\tnative\n";

fn loadout_runs(fx: &[Effect]) -> Vec<(chameleon_app::loadout::LoadoutTag, acc_core::Exec)> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Run {
                tag: ExecTag::Loadout(t),
                exec,
                ..
            } => Some((t.clone(), exec.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn skills_and_mcp_servers_are_linked_and_copied_between_harnesses() {
    use acc_core::loadout::Scope;
    use chameleon_app::loadout::LoadoutTag;
    let mut a = app();
    a.start(0);
    let done = |tag: LoadoutTag, result| AppEvent::ExecDone {
        host: "local".into(),
        tag: ExecTag::Loadout(tag),
        result,
    };

    // L reads the skill store and every harness's MCP config.
    let runs = loadout_runs(&a.handle(ch('L'), 1));
    assert!(runs.iter().any(|(t, _)| *t == LoadoutTag::Skills(Scope::User)));
    assert!(
        runs.iter()
            .any(|(t, _)| *t == LoadoutTag::McpRead(Harness::Claude, Scope::User))
    );
    assert!(
        runs.iter()
            .any(|(t, _)| *t == LoadoutTag::McpRead(Harness::Codex, Scope::User))
    );
    assert!(
        overlay_text(&mut a)
            .iter()
            .any(|l| l.contains("reading the skills"))
    );

    a.handle(done(LoadoutTag::Skills(Scope::User), ok(SKILLS_LISTING)), 2);
    let text = overlay_text(&mut a).join("\n");
    assert!(text.contains("❯ tdd") && text.contains("Test first"), "{text}");
    assert!(text.contains("only in codex"), "{text}");

    // Space on Claude's column unlinks it there; the listing is read again after.
    let runs = loadout_runs(&a.handle(ch(' '), 3));
    let (tag, exec) = runs.first().expect("unlinks");
    assert_eq!(*tag, LoadoutTag::SkillsChanged(Scope::User));
    assert!(exec.script.contains("rm -f"), "{}", exec.script);
    let runs = loadout_runs(&a.handle(done(tag.clone(), ok("")), 3));
    assert_eq!(runs[0].0, LoadoutTag::Skills(Scope::User));

    // Codex has something else in the way: nothing runs.
    a.handle(ch('l'), 4);
    assert!(loadout_runs(&a.handle(ch(' '), 4)).is_empty());
    // A skill only in Codex's directory is adopted into the store.
    a.handle(ch('j'), 4);
    let runs = loadout_runs(&a.handle(ch('A'), 4));
    assert!(runs[0].1.script.contains("mv \"$src\""), "{}", runs[0].1.script);

    // MCP: Claude has fs; space on Codex's column copies it there.
    a.handle(press(KeyCode::Tab), 5);
    let claude = r#"{"mcpServers": {"fs": {"command": "npx", "args": ["-y", "fs"]}}, "other": 1}"#;
    a.handle(
        done(LoadoutTag::McpRead(Harness::Claude, Scope::User), ok(claude)),
        5,
    );
    a.handle(
        done(
            LoadoutTag::McpRead(Harness::Codex, Scope::User),
            ok("model = \"o3\"\n"),
        ),
        5,
    );
    let text = overlay_text(&mut a).join("\n");
    assert!(text.contains("❯ fs") && text.contains("npx -y fs"), "{text}");
    let runs = loadout_runs(&a.handle(ch(' '), 6));
    let (tag, exec) = runs.first().expect("writes Codex's config");
    assert_eq!(*tag, LoadoutTag::McpWrite(Harness::Codex, Scope::User));
    let written = String::from_utf8(exec.stdin.clone()).unwrap();
    assert!(
        written.contains("model = \"o3\"") && written.contains("[mcp_servers.fs]"),
        "{written}"
    );

    // n adds a new server to the column's harness, on top of the last edit.
    a.handle(ch('n'), 7);
    for c in "docs https://example.com/mcp".chars() {
        a.handle(ch(c), 7);
    }
    let runs = loadout_runs(&a.handle(press(KeyCode::Enter), 7));
    let written = String::from_utf8(runs[0].1.stdin.clone()).unwrap();
    assert!(
        written.contains("[mcp_servers.fs]") && written.contains("example.com/mcp"),
        "{written}"
    );

    // X twice deletes fs from every harness that has it; Claude's other keys stay.
    a.handle(ch('g'), 8);
    a.handle(ch('j'), 8);
    assert!(loadout_runs(&a.handle(ch('X'), 8)).is_empty());
    let runs = loadout_runs(&a.handle(ch('X'), 8));
    assert_eq!(runs.len(), 2, "{runs:?}");
    for (_, exec) in &runs {
        let w = String::from_utf8(exec.stdin.clone()).unwrap();
        assert!(!w.contains("\"fs\"") && !w.contains("mcp_servers.fs"), "{w}");
    }
    a.handle(press(KeyCode::Esc), 9);
    assert!(a.overlay().is_none());
}

#[test]
fn the_agents_plan_is_pinned_under_the_transcript_until_it_is_done() {
    let mut a = app();
    let key = claude_ready(&mut a);
    let todos = |statuses: [&str; 3]| {
        let todos: Vec<_> = ["Read the test", "Fix the parser", "Run the suite"]
            .iter()
            .zip(statuses)
            .map(|(c, s)| serde_json::json!({"content": c, "status": s, "activeForm": c}))
            .collect();
        serde_json::json!({"type": "assistant", "message": {"id": "m_t", "type": "message", "role": "assistant", "content": [{"type": "tool_use", "id": format!("toolu_{}", statuses.join("")), "name": "TodoWrite", "input": {"todos": todos}}]}, "parent_tool_use_id": null, "session_id": "s"}).to_string()
    };
    assert!(a.screen().plan.is_none());

    feed_line(&mut a, &key, &todos(["completed", "in_progress", "pending"]), 3);
    let screen = a.screen();
    let plan = screen.plan.as_ref().expect("a plan with work left is shown");
    let text: Vec<String> = plan.lines.iter().map(|l| l.text()).collect();
    assert_eq!(
        text,
        [
            "plan 1/3",
            "✓ Read the test",
            "◐ Fix the parser",
            "○ Run the suite"
        ]
    );
    assert_eq!(
        screen.transcript.rect.y + screen.transcript.rect.h,
        plan.rect.y,
        "the plan takes the transcript's last rows"
    );

    feed_line(&mut a, &key, &todos(["completed", "completed", "completed"]), 4);
    assert!(a.screen().plan.is_none(), "a finished plan goes away");
}

#[test]
fn a_narrow_terminal_with_the_sidebar_focused_draws() {
    let mut a = app();
    for width in [0, 1, 10, 39, 40, 49] {
        a.handle(AppEvent::Resize { width, height: 5 }, 0);
        let _ = a.screen();
    }
}

#[test]
fn the_header_falls_back_to_an_emoji_and_the_name_without_a_nerd_font() {
    assert!(app().screen().header.text().starts_with(" \u{f314} no session"));
    let mut a = App::new(Config::parse("[general]\nnerd_font = false").unwrap(), "test");
    assert!(a.screen().header.text().starts_with(" 🦎 chameleon "));
}

#[test]
fn a_sandboxed_launch_says_what_the_host_did_with_it() {
    use acc_core::sandbox::Preset;
    use chameleon_app::overlay::{Overlay, Wizard};
    // `[sandbox]` turns it on for new sessions, with its extra paths.
    let mut a = app_with("[sandbox]\nwrite = [\"~/scratch\"]\n");
    a.handle(ctrl('n'), 1);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!(w.sandbox, Some(Preset::Workspace));
    // From the directory: worktree, then sandbox; `l` makes it read only.
    a.handle(press(KeyCode::Down), 1);
    a.handle(press(KeyCode::Down), 1);
    a.handle(ch('l'), 1);
    let Some(Overlay::Launch(w)) = a.overlay() else {
        panic!("expected the wizard");
    };
    assert_eq!((w.field, w.sandbox), (Wizard::SANDBOX, Some(Preset::ReadOnly)));
    let fx = a.handle(AppEvent::Key(Key::ctrl(KeyCode::Enter)), 2);
    let (host, _) = ran(&fx, |t| *t == ExecTag::ResolveDir).expect("resolves the directory");
    let fx = a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::ResolveDir,
            result: ok("/work\n"),
        },
        3,
    );
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Run {
                tag: ExecTag::Launch(id),
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .expect("launches");
    let key = SessionKey::new(&host, &id);
    let slot = a.slots().iter().find(|s| s.key == key).unwrap();
    let sandbox = slot.spec.sandbox.as_ref().expect("asks for a sandbox");
    assert_eq!(sandbox.preset, Preset::ReadOnly);
    assert_eq!(sandbox.write, vec!["~/scratch".to_owned()]);

    // The host couldn't: the PID is still read, and a notice says so.
    a.handle(
        AppEvent::ExecDone {
            host: host.clone(),
            tag: ExecTag::Launch(id.clone()),
            result: ok("4242\nB none bwrap not found\n"),
        },
        4,
    );
    let slot = a.slots().iter().find(|s| s.key == key).unwrap();
    assert!(matches!(
        slot.status,
        Some(acc_core::detached::Status::Alive { pid: 4242, .. })
    ));
    let toasts: Vec<String> = a.screen().toasts.iter().map(|l| l.text()).collect();
    assert!(toasts.iter().any(|t| t.contains("unconfined")), "{toasts:?}");
    let header = a.screen().header.text();
    assert!(header.contains("unconfined"), "{header}");
}

#[test]
fn a_click_opens_a_link_and_a_drag_copies_the_text() {
    use chameleon_app::event::MouseKind;
    let mut a = app();
    let key = claude_ready(&mut a);
    feed(
        &mut a,
        &key,
        r#"{"type":"assistant","message":{"id":"m_l","type":"message","role":"assistant","content":[{"type":"text","text":"Opened a PR: https://github.com/lum1n/chameleon/pull/6.\n\n```sh\ngh pr view 6\n```"}]},"parent_tool_use_id":null,"session_id":"s"}"#,
        2,
    );
    let screen = a.screen();
    let t = screen.transcript.rect;
    let find = |needle: &str| {
        screen.transcript.lines.iter().enumerate().find_map(|(row, l)| {
            let text = l.text();
            text.find(needle).map(|i| {
                (
                    t.x + chameleon_app::wrap::width(&text[..i]) as u16,
                    t.y + row as u16,
                )
            })
        })
    };
    let (x, y) = find("github.com").expect("the link is drawn");
    let mouse = |a: &mut App, x: u16, y: u16, kind: MouseKind| a.handle(AppEvent::Mouse { x, y, kind }, 3);

    // A click opens it, without the sentence's full stop.
    mouse(&mut a, x, y, MouseKind::Click);
    let fx = mouse(&mut a, x, y, MouseKind::Release);
    assert!(
        fx.contains(&Effect::Open {
            url: "https://github.com/lum1n/chameleon/pull/6".into()
        }),
        "{fx:?}"
    );

    // A drag over the code block copies the code, not its gutter.
    let (x, y) = find("gh pr").expect("the code is drawn");
    mouse(&mut a, t.x, y - 1, MouseKind::Click);
    mouse(&mut a, x + 4, y, MouseKind::Drag);
    mouse(&mut a, t.x + t.w + 5, y + 1, MouseKind::Drag);
    let screen = a.screen();
    let row = &screen.transcript.lines[(y - t.y) as usize];
    assert!(row.spans.iter().any(|s| s.style.reversed), "the selection shows");
    let fx = mouse(&mut a, t.x + t.w + 5, y + 1, MouseKind::Release);
    assert!(
        fx.contains(&Effect::Copy {
            text: "gh pr view 6".into()
        }),
        "{fx:?}"
    );
    // Any key ends it.
    a.handle(press(KeyCode::Esc), 4);
    let row = &a.screen().transcript.lines[(y - t.y) as usize];
    assert!(!row.spans.iter().any(|s| s.style.reversed));
}

#[test]
fn a_dropped_image_goes_inline_with_the_prompt() {
    use chameleon_app::image::Image;
    let mut a = app();
    let _key = claude_ready(&mut a);
    let png = Image::from_bytes("shot.png", b"\x89PNG\r\n\x1a\nPIXELS").unwrap();
    let data = png.attachment.data.clone();
    a.handle(
        AppEvent::Images {
            pasted: "/tmp/shot.png".into(),
            images: vec![
                Ok(png),
                Err("notes.txt isn't a PNG, JPEG, GIF or WebP image".into()),
            ],
        },
        3,
    );
    let screen = a.screen();
    assert!(
        screen
            .composer
            .lines
            .iter()
            .any(|l| l.text().contains("▣ shot.png"))
    );
    assert!(screen.toasts.iter().any(|t| t.text().contains("notes.txt")));
    // Images alone are a prompt; they ride the harness's own stdin.
    let fx = a.handle(press(KeyCode::Enter), 4);
    let input = fx
        .iter()
        .find_map(|e| match e {
            Effect::Input { exec, .. } => Some(String::from_utf8_lossy(&exec.stdin).into_owned()),
            _ => None,
        })
        .expect("the prompt is written to the harness");
    assert!(input.contains(&data), "{input}");
    let composer = a.screen().composer;
    assert!(
        !composer.lines.iter().any(|l| l.text().contains("▣")),
        "sent and gone"
    );
}

#[test]
fn ctrl_v_asks_for_the_clipboards_image() {
    let mut a = app();
    let fx = a.handle(AppEvent::Key(Key::ctrl(KeyCode::Char('v'))), 1);
    assert_eq!(fx, vec![Effect::PasteImage]);
}
