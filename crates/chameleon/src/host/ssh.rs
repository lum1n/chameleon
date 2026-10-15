//! A host reached through the system `ssh`, so ~/.ssh/config, agents and
//! ControlMaster all apply unchanged.

use tokio::process::Command;

use super::Host;

#[derive(Debug)]
pub struct SshHost {
    name: String,
    dest: String,
    args: Vec<String>,
    connect_timeout_s: u64,
}

impl SshHost {
    pub fn new(name: &str, dest: &str, args: Vec<String>, connect_timeout_s: u64) -> Self {
        Self {
            name: name.to_owned(),
            dest: dest.to_owned(),
            args,
            connect_timeout_s,
        }
    }
}

impl Host for SshHost {
    fn name(&self) -> &str {
        &self.name
    }

    fn command(&self, script: &str) -> Command {
        let mut c = Command::new("ssh");
        c.arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg(format!("ConnectTimeout={}", self.connect_timeout_s.max(1)))
            .args(&self.args)
            // Without these a dropped link (sleep, a network change) leaves the
            // follower blocked on a read forever instead of failing and retrying.
            .arg("-o")
            .arg("ServerAliveInterval=15")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("--")
            .arg(&self.dest)
            .arg(format!("sh -c {}", acc_core::shell::quote(script)));
        c
    }

    fn kills_group(&self) -> bool {
        // Killing the local ssh ends the remote shell's stdin, which stops its followers.
        false
    }
}
