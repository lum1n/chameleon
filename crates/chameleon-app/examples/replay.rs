//! Renders a detached session's log as plain text, for debugging views.
//!
//! ```text
//! cargo run -p chameleon-app --example replay -- <harness> <out.ndjson> [width] [height]
//! ```
//!
//! `REPLAY_KEYS` types keys before rendering, space-separated as in the
//! config (`REPLAY_KEYS="alt-2 k k enter"`); an open overlay is printed too.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use acc_core::session::LaunchRequest;
use chameleon_app::config::Config;
use chameleon_app::{App, AppEvent, Effect, ExecOutput, ExecTag};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(harness), Some(path)) = (args.first(), args.get(1)) else {
        eprintln!("usage: replay <harness> <out.ndjson> [width] [height]");
        std::process::exit(2);
    };
    let width: u16 = args.get(2).and_then(|w| w.parse().ok()).unwrap_or(100);
    let height: u16 = args.get(3).and_then(|h| h.parse().ok()).unwrap_or(40);
    let harness = chameleon_app::overlay::parse_harness(harness).expect("known harness");
    let log = std::fs::read(path).expect("readable log");

    let mut app = App::new(Config::default(), "replay");
    app.handle(AppEvent::Resize { width, height }, 0);
    let req = LaunchRequest {
        harness: Some(harness),
        cwd: "/w".into(),
        ..LaunchRequest::default()
    };
    // Go through the same path the wizard takes.
    let (session, _) = acc_core::session::Session::launch(req, "replay-1", "replay", 0).unwrap();
    let spec = session.spec().to_json();
    let list = format!("S\treplay-1\talive 1 {}\t0\nJ\t{spec}\n", log.len());
    let fx = app.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::List,
            result: Ok(ExecOutput {
                status: Some(0),
                stdout: list.into_bytes(),
                stderr: Vec::new(),
            }),
        },
        1,
    );
    assert!(fx.iter().any(|e| matches!(
        e,
        Effect::Run {
            tag: ExecTag::Status(_),
            ..
        }
    )));
    app.handle(
        AppEvent::ExecDone {
            host: "local".into(),
            tag: ExecTag::Status("replay-1".into()),
            result: Ok(ExecOutput {
                status: Some(0),
                stdout: format!("alive 1 {}\n", log.len()).into_bytes(),
                stderr: Vec::new(),
            }),
        },
        2,
    );
    for line in log.split_inclusive(|&b| b == b'\n') {
        app.handle(
            AppEvent::FeedLine {
                host: "local".into(),
                key: "l:replay-1".into(),
                bytes: line.to_vec(),
            },
            3,
        );
        if std::env::var_os("REPLAY_FRAMES").is_some() {
            let _ = app.screen();
        }
    }
    let slot = app.selected().expect("selected");
    let session = slot.session.as_ref().expect("attached");
    eprintln!(
        "items: {}  state: {:?}  permissions: {}  options: {}  commands: {}",
        session.transcript().len(),
        session.state().map(|s| &s.state),
        session.permissions().len(),
        session.options().len(),
        session.commands().len()
    );
    for i in session.transcript().items() {
        eprintln!("  {:?} {:?} {} chars", i.kind, i.status, i.text.len());
    }
    for k in std::env::var("REPLAY_KEYS")
        .unwrap_or_default()
        .split_whitespace()
    {
        let key = chameleon_app::keymap::Key::parse(k).expect("a key");
        app.handle(AppEvent::Key(key), 4);
    }
    let screen = app.screen();
    if let Some(o) = &screen.overlay {
        println!("[{}]", o.title);
        for l in &o.lines {
            println!("{}", l.text());
        }
        println!("[{}]", o.footer.as_ref().map(|f| f.text()).unwrap_or_default());
    }
    println!("{}", screen.header.text());
    println!("{}", "─".repeat(width as usize));
    for l in &screen.transcript.lines {
        println!("{}", l.text());
    }
    println!("{}", "─".repeat(width as usize));
    for l in &screen.composer.lines {
        println!("{}", l.text());
    }
    println!("{}", screen.status.text());
}
