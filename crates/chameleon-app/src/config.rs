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
    /// Themes from `themes/*.json` next to the config, loaded by the binary.
    #[serde(skip)]
    pub custom_themes: Vec<crate::themes::ThemeDef>,
    /// Problems found while loading (bad theme files), shown in help.
    #[serde(skip)]
    pub problems: Vec<String>,
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
    /// Start with the session list open; `b` / `ctrl-b` toggles it.
    #[serde(default = "d_true")]
    pub show_sidebar: bool,
    /// Default harness in the launch wizard (`claude-code`, `codex`, ...).
    pub default_harness: Option<String>,
    /// How the transcript sets messages apart; `B` cycles it.
    #[serde(default)]
    pub blocks: Blocks,
}

/// How the transcript marks where one message ends and the next begins.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Blocks {
    /// A coloured bar down the left of every message and run of steps.
    #[default]
    Bars,
    /// Messages in rounded boxes, steps behind a bar.
    Cards,
    /// Only the user's messages are marked.
    Plain,
}

impl Blocks {
    pub fn next(self) -> Self {
        match self {
            Blocks::Bars => Blocks::Cards,
            Blocks::Cards => Blocks::Plain,
            Blocks::Plain => Blocks::Bars,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Blocks::Bars => "bars",
            Blocks::Cards => "cards",
            Blocks::Plain => "plain",
        }
    }
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
            show_sidebar: true,
            default_harness: None,
            blocks: Blocks::default(),
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
    /// A Sessh / Lapp theme (`tokyo-night`, `catppuccin-latte`, `paper`...)
    /// or a custom one; unset means the terminal's own colours.
    pub name: Option<String>,
    /// Paint the theme's background (default true); false keeps the
    /// terminal's, for transparent terminals.
    pub background: Option<bool>,
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
    pub border: Option<String>,
}

/// Resolved colours. `Reset` means the terminal's own.
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
    /// The screen's background.
    pub bg: Color,
    /// Raised surfaces: the header, the status bar, overlays, toasts.
    pub surface: Color,
    /// Rules between panes.
    pub border: Color,
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
            bg: Color::Reset,
            surface: Color::Reset,
            border: Color::DarkGray,
        }
    }
}

impl Theme {
    /// The named theme (or the terminal's colours), then the overrides.
    pub fn from_config(
        c: &ThemeConfig,
        custom: &[crate::themes::ThemeDef],
        problems: &mut Vec<String>,
    ) -> Self {
        let mut t = match c.name.as_deref() {
            None => Self::default(),
            Some(name) => Self::named(name, custom).unwrap_or_else(|| {
                problems.push(format!("unknown theme {name:?} (see the theme picker: T)"));
                Self::default()
            }),
        };
        if c.background == Some(false) {
            t.bg = Color::Reset;
            t.surface = Color::Reset;
        }
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
        set("border", &c.border, &mut t.border);
        t
    }

    /// A built-in or custom theme by id or name.
    pub fn named(name: &str, custom: &[crate::themes::ThemeDef]) -> Option<Self> {
        let builtin = crate::themes::builtin();
        crate::themes::find(name, custom, &builtin).map(crate::themes::ThemeDef::resolve)
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

    /// Hosts to drive. This machine is always first unless the config
    /// lists a local host itself (any `[[hosts]]` entry without `ssh`).
    pub fn hosts(&self) -> Vec<HostConfig> {
        let mut hosts = self.hosts.clone();
        if !hosts.iter().any(HostConfig::is_local) {
            let name = if hosts.iter().any(|h| h.name == "local") {
                "here"
            } else {
                "local"
            };
            hosts.insert(0, HostConfig::local(name));
        }
        hosts
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
        assert_eq!(c.hosts()[1].ssh.as_deref(), Some("box"));
        assert_eq!(c.keys["select_down"].to_vec(), vec!["j", "down"]);
        let mut problems = Vec::new();
        let t = Theme::from_config(&c.theme, &[], &mut problems);
        assert_eq!(t.accent, Color::Rgb(255, 0, 255));
        assert!(problems.is_empty());
        assert!(c.hooks[1].suspend_tui);
    }

    #[test]
    fn local_is_always_present() {
        let c = Config::parse("").expect("empty is fine");
        assert!(c.hosts()[0].is_local());
        assert!(Config::parse("[nope]\nx=1").is_err());
        let c = Config::parse("[[hosts]]\nname='box'\nssh='box'").expect("parses");
        let hosts = c.hosts();
        assert_eq!(hosts.len(), 2);
        assert!(hosts[0].is_local() && hosts[0].name == "local");
        assert_eq!(hosts[1].name, "box");
        let c = Config::parse("[[hosts]]\nname='mine'\n[[hosts]]\nname='box'\nssh='box'").expect("parses");
        assert_eq!(c.hosts().len(), 2, "an explicit local host is kept as is");
    }
}
