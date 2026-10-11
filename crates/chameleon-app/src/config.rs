//! `~/.config/chameleon/config.toml`.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::style::Color;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub hosts: Vec<HostConfig>,
    /// `action = "key"` or `action = ["key", "key"]`.
    #[serde(default)]
    pub keys: HashMap<String, OneOrMany>,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub hooks: Vec<HookConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    /// Tag that marks this client's protocol requests; must be unique
    /// among clients sharing a session. Default: `cham-<hostname>-<pid>`.
    pub client_tag: Option<String>,
    #[serde(default = "d_tick")]
    pub tick_ms: u64,
    /// How often the session list is re-read on every host.
    #[serde(default = "d_list")]
    pub list_refresh_s: u64,
    #[serde(default = "d_exec_timeout")]
    pub exec_timeout_s: u64,
    /// Attach every alive session found on a host automatically.
    #[serde(default = "d_true")]
    pub auto_attach: bool,
    #[serde(default = "d_sidebar")]
    pub sidebar_width: u16,
    /// Default harness in the launch wizard (`claude-code`, `codex`, ...).
    pub default_harness: Option<String>,
}

fn d_tick() -> u64 {
    250
}
fn d_list() -> u64 {
    20
}
fn d_exec_timeout() -> u64 {
    20
}
fn d_true() -> bool {
    true
}
fn d_sidebar() -> u16 {
    34
}

impl Default for General {
    fn default() -> Self {
        Self {
            client_tag: None,
            tick_ms: d_tick(),
            list_refresh_s: d_list(),
            exec_timeout_s: d_exec_timeout(),
            auto_attach: true,
            sidebar_width: d_sidebar(),
            default_harness: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub name: String,
    /// An `ssh` destination (`user@host`, or a Host from ~/.ssh/config).
    /// Absent means this machine.
    pub ssh: Option<String>,
    #[serde(default)]
    pub ssh_args: Vec<String>,
    #[serde(default = "d_connect")]
    pub connect_timeout_s: u64,
}

fn d_connect() -> u64 {
    10
}

impl HostConfig {
    pub fn local(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ssh: None,
            ssh_args: Vec::new(),
            connect_timeout_s: d_connect(),
        }
    }
    pub fn is_local(&self) -> bool {
        self.ssh.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s.clone()],
            OneOrMany::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ThemeConfig {
    pub accent: Option<String>,
    pub permission: Option<String>,
    pub error: Option<String>,
    pub tool: Option<String>,
    pub thinking: Option<String>,
    pub done: Option<String>,
    pub muted: Option<String>,
    pub user: Option<String>,
    pub agent: Option<String>,
    pub add: Option<String>,
    pub remove: Option<String>,
}

/// Resolved colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub accent: Color,
    pub permission: Color,
    pub error: Color,
    pub tool: Color,
    pub thinking: Color,
    pub done: Color,
    pub muted: Color,
    pub user: Color,
    pub agent: Color,
    pub add: Color,
    pub remove: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            accent: Color::Magenta,
            permission: Color::Red,
            error: Color::Red,
            tool: Color::Cyan,
            thinking: Color::Yellow,
            done: Color::Green,
            muted: Color::DarkGray,
            user: Color::Blue,
            agent: Color::Reset,
            add: Color::Green,
            remove: Color::Red,
        }
    }
}

impl Theme {
    pub fn from_config(c: &ThemeConfig, problems: &mut Vec<String>) -> Self {
        let mut t = Self::default();
        let mut set = |name: &str, v: &Option<String>, slot: &mut Color| {
            if let Some(s) = v {
                match Color::parse(s) {
                    Some(c) => *slot = c,
                    None => problems.push(format!("unknown colour {s:?} for theme.{name}")),
                }
            }
        };
        set("accent", &c.accent, &mut t.accent);
        set("permission", &c.permission, &mut t.permission);
        set("error", &c.error, &mut t.error);
        set("tool", &c.tool, &mut t.tool);
        set("thinking", &c.thinking, &mut t.thinking);
        set("done", &c.done, &mut t.done);
        set("muted", &c.muted, &mut t.muted);
        set("user", &c.user, &mut t.user);
        set("agent", &c.agent, &mut t.agent);
        set("add", &c.add, &mut t.add);
        set("remove", &c.remove, &mut t.remove);
        t
    }
}

/// A shell command run on this machine for an event or a key.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HookConfig {
    pub name: String,
    pub command: String,
    /// Events that fire it: `permission`, `question`, `error`, `done`,
    /// `idle`, `exited`, `turn`, `notice`.
    #[serde(default)]
    pub on: Vec<String>,
    /// A key that fires it for the selected session.
    pub key: Option<String>,
    /// Leave the TUI while it runs (editors, pagers).
    #[serde(default)]
    pub suspend_tui: bool,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// `$XDG_CONFIG_HOME/chameleon/config.toml` or `~/.config/chameleon/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("chameleon").join("config.toml"))
    }

    /// Hosts to drive; the local machine when none are configured.
    pub fn hosts(&self) -> Vec<HostConfig> {
        if self.hosts.is_empty() {
            vec![HostConfig::local("local")]
        } else {
            self.hosts.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_example() {
        let c = Config::parse(
            r##"
[general]
tick_ms = 100
[[hosts]]
name = "box"
ssh = "box"
ssh_args = ["-p", "2222"]
[keys]
quit = "q"
select_down = ["j", "down"]
[theme]
permission = "red"
accent = "#ff00ff"
[[hooks]]
name = "notify"
on = ["permission"]
command = "notify-send hi"
[[hooks]]
name = "edit"
key = "ctrl-o"
command = "$EDITOR"
suspend_tui = true
"##,
        )
        .expect("parses");
        assert_eq!(c.general.tick_ms, 100);
        assert_eq!(c.hosts()[0].ssh.as_deref(), Some("box"));
        assert_eq!(c.keys["select_down"].to_vec(), vec!["j", "down"]);
        let mut problems = Vec::new();
        let t = Theme::from_config(&c.theme, &mut problems);
        assert_eq!(t.accent, Color::Rgb(255, 0, 255));
        assert!(problems.is_empty());
        assert!(c.hooks[1].suspend_tui);
    }

    #[test]
    fn empty_config_means_local() {
        let c = Config::parse("").expect("empty is fine");
        assert!(c.hosts()[0].is_local());
        assert!(Config::parse("[nope]\nx=1").is_err());
    }
}
