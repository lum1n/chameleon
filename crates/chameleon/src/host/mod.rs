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

/// Starts a long-running script with stdout piped and stdin closed.
pub fn spawn_long(host: &dyn Host, script: &str) -> Result<Child, String> {
    let mut cmd = host.command(script);
    cmd.stdin(Stdio::null())
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
