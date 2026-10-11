//! This machine.

use tokio::process::Command;

use super::Host;

#[derive(Debug)]
pub struct LocalHost {
    name: String,
}

impl LocalHost {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
        }
    }
}

impl Host for LocalHost {
    fn name(&self) -> &str {
        &self.name
    }

    fn command(&self, script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    fn kills_group(&self) -> bool {
        true
    }
}
