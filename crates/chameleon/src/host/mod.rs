//! Hosts run acc's `sh` scripts. The only seam between the local machine
//! and an SSH host is how the `sh` command is built.

pub mod feed;
pub mod local;
pub mod ssh;

use std::process::Stdio;
use std::time::Duration;

use acc_core::Exec;
use chameleon_app::ExecOutput;
use tokio::io::AsyncWriteExt as _;
use tokio::process::{Child, Command};

pub trait Host: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    /// A command that runs `script` under `sh` on the host.
    fn command(&self, script: &str) -> Command;
    /// Long-lived children are started in their own process group so the
    /// whole pipeline can be stopped at once.
    fn kills_group(&self) -> bool;
}

/// Runs a short script, feeding `exec.stdin`, with a deadline.
pub async fn run(host: &dyn Host, exec: &Exec, timeout: Duration) -> Result<ExecOutput, String> {
    let mut cmd = host.command(&exec.script);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("can't start sh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = exec.stdin.clone();
        // Write in the background so a full pipe can't deadlock the wait.
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => Ok(ExecOutput {
            status: o.status.code(),
            stdout: o.stdout,
            stderr: o.stderr,
        }),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
    }
}

/// Moves stdin aside and TERMs the script once it ends. sshd sends no
/// SIGHUP without a pty, and an idle `tail -F` never writes to find its
/// pipe closed, so a dropped connection would otherwise leave the followers
/// running on the host for good. The script's own trap stops them.
const STOP_ON_HANGUP: &str = "exec 3<&0 </dev/null\n{ cat <&3 >/dev/null; kill -TERM $$; } &\nexec 3<&-\n";

/// Starts a long-running script with stdout piped. Its stdin stays open
/// while the child is held; closing it, or losing the connection, stops it.
pub fn spawn_long(host: &dyn Host, script: &str) -> Result<Child, String> {
    let mut cmd = host.command(&format!("{STOP_ON_HANGUP}{script}"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    if host.kills_group() {
        cmd.process_group(0);
    }
    cmd.spawn().map_err(|e| format!("can't start the follower: {e}"))
}

/// Stops a long-running child and, on local hosts, its process group.
pub async fn stop(host: &dyn Host, child: &mut Child) {
    if host.kills_group()
        && let Some(pid) = child.id()
    {
        let _ = Command::new("kill")
            .args(["-TERM", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.kill().await;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use acc_core::detached::{FeedEntry, FeedPath, fan_in_script};

    use super::*;

    /// `sh` like an SSH host: nothing kills its group, so only a hangup
    /// tells it to stop.
    #[derive(Debug)]
    struct Remote;

    impl Host for Remote {
        fn name(&self) -> &str {
            "remote"
        }

        fn command(&self, script: &str) -> Command {
            let mut c = Command::new("sh");
            c.arg("-c").arg(script);
            c
        }

        fn kills_group(&self) -> bool {
            false
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_hangup_stops_the_followers() {
        let dir = std::env::temp_dir().join(format!("cham-hangup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("idle.log");
        std::fs::write(&log, "").unwrap();
        let script = fan_in_script(&[FeedEntry {
            key: "a".into(),
            path: FeedPath::File(log.display().to_string()),
            offset: 0,
        }]);
        let mut child = spawn_long(&Remote, &script).unwrap();
        let tails = || {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("pgrep -f '^tail -c .* -F {}' || true", log.display()))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!tails().is_empty(), "the follower is tailing");

        // The connection drops: the script's stdin ends and nothing is signalled.
        drop(child.stdin.take());
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("the script exits on a hangup")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(tails().is_empty(), "tails still running: {}", tails());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
