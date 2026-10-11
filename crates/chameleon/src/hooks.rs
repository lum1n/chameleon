//! Runs hook commands on this machine.

use std::process::Stdio;

use chameleon_app::AppEvent;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc::UnboundedSender;

pub struct HookRun {
    pub name: String,
    pub command: String,
    pub payload: String,
    pub env: Vec<(String, String)>,
}

/// Runs a hook in the background and reports when it ends.
pub fn spawn(run: HookRun, tx: UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&run.command)
            .envs(run.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let result = match cmd.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(run.payload.as_bytes()).await;
                    let _ = stdin.shutdown().await;
                }
                match child.wait_with_output().await {
                    Ok(o) => (
                        o.status.code(),
                        String::from_utf8_lossy(&o.stderr).trim().to_owned(),
                    ),
                    Err(e) => (None, e.to_string()),
                }
            }
            Err(e) => (None, e.to_string()),
        };
        let _ = tx.send(AppEvent::HookDone {
            name: run.name,
            status: result.0,
            stderr: result.1,
        });
    });
}

/// Runs a hook in the foreground, with the terminal handed to it.
pub fn run_blocking(run: &HookRun) -> (Option<i32>, String) {
    use std::io::Write as _;
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&run.command)
        .envs(run.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (None, e.to_string()),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(run.payload.as_bytes());
    }
    match child.wait() {
        Ok(s) => (s.code(), String::new()),
        Err(e) => (None, e.to_string()),
    }
}
