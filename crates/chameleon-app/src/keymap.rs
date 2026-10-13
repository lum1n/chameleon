//! Keys and the actions they map to. Keys are parsed from config strings
//! such as `ctrl-n`, `alt-enter`, `j`, `down`, `f5`.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Key {
    pub const fn new(code: KeyCode) -> Self {
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn ctrl(code: KeyCode) -> Self {
        Self {
            ctrl: true,
            ..Self::new(code)
        }
    }
    pub const fn alt(code: KeyCode) -> Self {
        Self {
            alt: true,
            ..Self::new(code)
        }
    }
    pub const fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c))
    }

    /// A key with no modifiers that types a character.
    pub fn plain_char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if !self.ctrl && !self.alt => Some(c),
            _ => None,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        // A single character (including '-') is the character itself.
        if s.chars().count() == 1 {
            return s.chars().next().map(Self::char);
        }
        let mut key = Self::new(KeyCode::Esc);
        let parts: Vec<&str> = s.split(['-', '+']).collect();
        let (mods, last) = parts.split_at(parts.len() - 1);
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "c" | "control" => key.ctrl = true,
                "alt" | "meta" | "m" | "opt" => key.alt = true,
                "shift" | "s" => key.shift = true,
                _ => return None,
            }
        }
        let name = last[0];
        key.code = match name.to_ascii_lowercase().as_str() {
            "enter" | "return" | "cr" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" | "btab" => KeyCode::BackTab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "space" => KeyCode::Char(' '),
            "" => KeyCode::Char('-'),
            f if f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
                KeyCode::F(f[1..].parse().unwrap_or(1))
            }
            other => {
                let mut it = other.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return None,
                }
            }
        };
        if key.shift
            && let KeyCode::Char(c) = key.code
        {
            key.code = KeyCode::Char(c.to_ascii_uppercase());
            key.shift = false;
        }
        Some(key)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("C-")?;
        }
        if self.alt {
            f.write_str("M-")?;
        }
        if self.shift {
            f.write_str("S-")?;
        }
        match self.code {
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Up => f.write_str("↑"),
            KeyCode::Down => f.write_str("↓"),
            KeyCode::Left => f.write_str("←"),
            KeyCode::Right => f.write_str("→"),
            KeyCode::PageUp => f.write_str("pgup"),
            KeyCode::PageDown => f.write_str("pgdn"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::F(n) => write!(f, "F{n}"),
        }
    }
}

/// Everything a key can do outside of typing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    Help,
    SelectUp,
    SelectDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    FollowTail,
    FocusNext,
    FocusPrev,
    FocusSidebar,
    FocusTranscript,
    FocusComposer,
    Open,
    Back,
    Palette,
    NewSession,
    Refresh,
    Kill,
    Remove,
    Interrupt,
    Compact,
    Fork,
    Options,
    Diff,
    Approve,
    Deny,
    ToggleExpand,
    Agents,
    NextAgent,
    PrevAgent,
    ToggleSidebar,
    Theme,
    Jump(u8),
}

impl Action {
    pub const ALL: [Action; 35] = [
        Action::Quit,
        Action::Help,
        Action::SelectUp,
        Action::SelectDown,
        Action::PageUp,
        Action::PageDown,
        Action::ScrollTop,
        Action::ScrollBottom,
        Action::FollowTail,
        Action::FocusNext,
        Action::FocusPrev,
        Action::FocusSidebar,
        Action::FocusTranscript,
        Action::FocusComposer,
        Action::Open,
        Action::Back,
        Action::Palette,
        Action::NewSession,
        Action::Refresh,
        Action::Kill,
        Action::Remove,
        Action::Interrupt,
        Action::Compact,
        Action::Fork,
        Action::Options,
        Action::Diff,
        Action::Approve,
        Action::Deny,
        Action::ToggleExpand,
        Action::Agents,
        Action::NextAgent,
        Action::PrevAgent,
        Action::ToggleSidebar,
        Action::Theme,
        Action::Jump(0),
    ];

    /// The config name of the action.
    pub fn name(self) -> String {
        match self {
            Action::Jump(_) => "jump".into(),
            other => serde_json::to_value(other)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        if name == "jump" {
            return Some(Action::Jump(0));
        }
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
    }

    pub fn describe(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Help => "help",
            Action::SelectUp => "select previous",
            Action::SelectDown => "select next",
            Action::PageUp => "page up",
            Action::PageDown => "page down",
            Action::ScrollTop => "scroll to top",
            Action::ScrollBottom => "scroll to bottom",
            Action::FollowTail => "follow the tail",
            Action::FocusNext => "next pane",
            Action::FocusPrev => "previous pane",
            Action::FocusSidebar => "focus sessions",
            Action::FocusTranscript => "focus transcript",
            Action::FocusComposer => "focus composer",
            Action::Open => "open / attach / open subagent",
            Action::Back => "back / cancel / leave subagent",
            Action::Palette => "command palette",
            Action::NewSession => "new session",
            Action::Refresh => "refresh sessions",
            Action::Kill => "stop the harness",
            Action::Remove => "remove the session",
            Action::Interrupt => "interrupt the turn",
            Action::Compact => "compact the context",
            Action::Fork => "fork the session",
            Action::Options => "model / effort / mode / permission",
            Action::Diff => "show diffs",
            Action::Approve => "approve the first request",
            Action::Deny => "deny the first request",
            Action::ToggleExpand => "expand / collapse item",
            Action::Agents => "pick main or a subagent to view",
            Action::NextAgent => "view the next subagent",
            Action::PrevAgent => "view the previous subagent",
            Action::ToggleSidebar => "show / hide the session list",
            Action::Theme => "colour theme",
            Action::Jump(_) => "jump to session 1-9, 0",
        }
    }
}

/// Which keys trigger which actions, by pane.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// Keys that act everywhere, even while typing (modifier keys only).
    global: HashMap<Key, Action>,
    /// Keys that act when the composer isn't capturing text.
    nav: HashMap<Key, Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        use KeyCode::*;
        let mut m = Self {
            global: HashMap::new(),
            nav: HashMap::new(),
        };
        let g = [
            (Key::ctrl(Char('c')), Action::Interrupt),
            (Key::ctrl(Char('q')), Action::Quit),
            (Key::ctrl(Char('n')), Action::NewSession),
            (Key::ctrl(Char('x')), Action::Kill),
            (Key::ctrl(Char('r')), Action::Refresh),
            (Key::ctrl(Char('f')), Action::Fork),
            (Key::ctrl(Char('l')), Action::Compact),
            (Key::ctrl(Char('o')), Action::Options),
            (Key::ctrl(Char('p')), Action::Palette),
            (Key::ctrl(Char('d')), Action::Diff),
            (Key::ctrl(Char('y')), Action::Approve),
            (Key::ctrl(Char('u')), Action::PageUp),
            (Key::ctrl(Char('e')), Action::PageDown),
            (Key::ctrl(Char('g')), Action::FollowTail),
            (Key::ctrl(Char('b')), Action::ToggleSidebar),
            (Key::new(Tab), Action::FocusNext),
            (Key::new(BackTab), Action::FocusPrev),
            (Key::new(Esc), Action::Back),
            (Key::new(PageUp), Action::PageUp),
            (Key::new(PageDown), Action::PageDown),
            (Key::new(F(1)), Action::Help),
            (Key::alt(Char('1')), Action::FocusSidebar),
            (Key::alt(Char('2')), Action::FocusTranscript),
            (Key::alt(Char('3')), Action::FocusComposer),
        ];
        for (k, a) in g {
            m.global.insert(k, a);
        }
        let n = [
            (Key::char('q'), Action::Quit),
            (Key::char('?'), Action::Help),
            (Key::char('j'), Action::SelectDown),
            (Key::char('k'), Action::SelectUp),
            (Key::new(Down), Action::SelectDown),
            (Key::new(Up), Action::SelectUp),
            (Key::char('g'), Action::ScrollTop),
            (Key::char('G'), Action::FollowTail),
            (Key::new(Home), Action::ScrollTop),
            (Key::new(End), Action::ScrollBottom),
            (Key::new(Enter), Action::Open),
            (Key::char('/'), Action::Palette),
            (Key::char('n'), Action::NewSession),
            (Key::char('r'), Action::Refresh),
            (Key::char('x'), Action::Kill),
            (Key::char('D'), Action::Remove),
            (Key::char('f'), Action::Fork),
            (Key::char('c'), Action::Compact),
            (Key::char('o'), Action::Options),
            (Key::char('d'), Action::Diff),
            (Key::char('y'), Action::Approve),
            (Key::char('N'), Action::Deny),
            (Key::char('i'), Action::FocusComposer),
            (Key::char(' '), Action::ToggleExpand),
            (Key::char('a'), Action::Agents),
            (Key::char(']'), Action::NextAgent),
            (Key::char('['), Action::PrevAgent),
            (Key::char('b'), Action::ToggleSidebar),
            (Key::char('T'), Action::Theme),
            (Key::char('h'), Action::FocusSidebar),
            (Key::char('l'), Action::FocusTranscript),
        ];
        for (k, a) in n {
            m.nav.insert(k, a);
        }
        for d in '0'..='9' {
            let n = d.to_digit(10).unwrap_or(0) as u8;
            m.nav.insert(Key::char(d), Action::Jump(n));
        }
        m
    }
}

impl Keymap {
    /// Applies `action = key | [keys]` overrides from config. Unknown
    /// actions and keys are reported, not fatal.
    pub fn apply(&mut self, overrides: &HashMap<String, Vec<String>>) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, keys) in overrides {
            let Some(action) = Action::from_name(name) else {
                problems.push(format!("unknown action {name:?} in [keys]"));
                continue;
            };
            self.global.retain(|_, a| *a != action);
            self.nav.retain(|_, a| *a != action);
            for k in keys {
                match Key::parse(k) {
                    Some(key) if key.ctrl || key.alt || !matches!(key.code, KeyCode::Char(_)) => {
                        self.global.insert(key, action);
                    }
                    Some(key) => {
                        self.nav.insert(key, action);
                    }
                    None => problems.push(format!("unknown key {k:?} for {name}")),
                }
            }
        }
        problems
    }

    pub fn global(&self, key: &Key) -> Option<Action> {
        self.global.get(key).copied()
    }

    pub fn nav(&self, key: &Key) -> Option<Action> {
        self.nav.get(key).copied()
    }

    /// Keys bound to an action, for help text.
    pub fn keys_for(&self, action: Action) -> Vec<Key> {
        let mut keys: Vec<Key> = self
            .nav
            .iter()
            .chain(self.global.iter())
            .filter(|(_, a)| match (a, action) {
                (Action::Jump(_), Action::Jump(_)) => true,
                (a, b) => **a == b,
            })
            .map(|(k, _)| *k)
            .collect();
        keys.sort_by_key(|k| (k.ctrl, k.alt, k.to_string()));
        keys.dedup();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys() {
        assert_eq!(Key::parse("ctrl-n"), Some(Key::ctrl(KeyCode::Char('n'))));
        assert_eq!(Key::parse("alt-enter"), Some(Key::alt(KeyCode::Enter)));
        assert_eq!(Key::parse("j"), Some(Key::char('j')));
        assert_eq!(Key::parse("-"), Some(Key::char('-')));
        assert_eq!(Key::parse("down"), Some(Key::new(KeyCode::Down)));
        assert_eq!(Key::parse("F5"), Some(Key::new(KeyCode::F(5))));
        assert_eq!(Key::parse("shift-d"), Some(Key::char('D')));
        assert_eq!(Key::parse("wat"), None);
    }

    #[test]
    fn overrides_replace_bindings() {
        let mut m = Keymap::default();
        let mut o = HashMap::new();
        o.insert("quit".to_owned(), vec!["Q".to_owned(), "ctrl-w".to_owned()]);
        assert!(m.apply(&o).is_empty());
        assert_eq!(m.nav(&Key::char('q')), None);
        assert_eq!(m.nav(&Key::char('Q')), Some(Action::Quit));
        assert_eq!(m.global(&Key::ctrl(KeyCode::Char('w'))), Some(Action::Quit));
        assert!(Action::from_name("new_session").is_some());
        assert_eq!(Action::NewSession.name(), "new_session");
    }
}
