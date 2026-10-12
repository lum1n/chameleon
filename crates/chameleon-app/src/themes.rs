//! The colour themes shared with Sessh and Lapp. Sessh's catalog is
//! vendored as is (`themes/sessh.json`, the same schema its theme editor
//! exports); Lapp's themes that Sessh lacks are converted into that schema
//! (`themes/lapp.json`). Lapp's ids for the families both have are aliases.

use serde::Deserialize;

use crate::config::Theme;
use crate::style::Color;

const SESSH: &str = include_str!("../themes/sessh.json");
const LAPP: &str = include_str!("../themes/lapp.json");

/// A theme in Sessh's JSON schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeDef {
    pub id: String,
    pub name: String,
    pub bg: String,
    pub bg_elevated: String,
    pub fg: String,
    pub fg_dim: String,
    pub accent: String,
    pub border: String,
    pub danger: String,
    pub terminal: TerminalDef,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TerminalDef {
    /// ANSI 0–15; missing or null entries fall back to the theme's colours.
    #[serde(default)]
    pub palette: Vec<Option<String>>,
}

/// Lapp ids for themes Sessh has under another name.
const ALIASES: [(&str, &str); 9] = [
    ("tokyonight", "tokyo-night"),
    ("tokyonight-day", "tokyo-night-day"),
    ("gruvbox", "gruvbox-dark"),
    ("everforest", "everforest-dark"),
    ("onedark", "one-dark"),
    ("onelight", "one-light"),
    ("kanagawa", "kanagawa-wave"),
    // Sessh's own legacy ids.
    ("amber", "gruvbox-dark"),
    ("slate", "tokyo-night"),
];

/// Every built-in theme, sorted by name.
pub fn builtin() -> Vec<ThemeDef> {
    let mut all: Vec<ThemeDef> = Vec::new();
    for src in [SESSH, LAPP] {
        // The vendored files are tested; a bad one just contributes nothing.
        all.extend(serde_json::from_str::<Vec<ThemeDef>>(src).unwrap_or_default());
    }
    all.sort_by_key(|t| t.name.to_lowercase());
    all
}

/// Parses a theme file: one theme object (what Sessh's "copy theme"
/// exports), an array of them, or an object with the theme under `theme`.
pub fn parse(text: &str) -> Result<Vec<ThemeDef>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let v = match v {
        serde_json::Value::Array(_) => v,
        serde_json::Value::Object(ref m) if m.contains_key("theme") => {
            serde_json::Value::Array(vec![m["theme"].clone()])
        }
        other => serde_json::Value::Array(vec![other]),
    };
    serde_json::from_value(v).map_err(|e| e.to_string())
}

/// The theme called `id` (or a known alias), custom themes first.
pub fn find<'a>(id: &str, custom: &'a [ThemeDef], builtin: &'a [ThemeDef]) -> Option<&'a ThemeDef> {
    let id = id.trim().to_ascii_lowercase();
    let id = ALIASES
        .iter()
        .find(|(alias, _)| *alias == id)
        .map_or(id.as_str(), |(_, real)| real);
    custom
        .iter()
        .chain(builtin)
        .find(|t| t.id == id || t.name.to_ascii_lowercase() == id)
}

impl ThemeDef {
    /// Chameleon's roles from the theme's colours: chrome from the theme's
    /// own tokens, states from its ANSI palette.
    pub fn resolve(&self) -> Theme {
        let c = |s: &str| Color::parse(s).unwrap_or(Color::Reset);
        let ansi = |i: usize, fallback: &str| {
            self.terminal
                .palette
                .get(i)
                .and_then(|p| p.as_deref())
                .and_then(Color::parse)
                .unwrap_or_else(|| c(fallback))
        };
        Theme {
            accent: c(&self.accent),
            permission: ansi(3, &self.accent),
            error: c(&self.danger),
            tool: ansi(6, &self.accent),
            thinking: ansi(5, &self.accent),
            done: ansi(2, &self.accent),
            muted: c(&self.fg_dim),
            user: ansi(4, &self.accent),
            agent: c(&self.fg),
            add: ansi(2, &self.accent),
            remove: ansi(1, &self.danger),
            bg: c(&self.bg),
            surface: c(&self.bg_elevated),
            border: c(&self.border),
        }
    }

    /// True for themes on a light background.
    pub fn is_light(&self) -> bool {
        match Color::parse(&self.bg) {
            Some(Color::Rgb(r, g, b)) => {
                (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000 > 128
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vendored_catalogs_parse_and_resolve() {
        let sessh: Vec<ThemeDef> = serde_json::from_str(SESSH).expect("sessh.json parses");
        let lapp: Vec<ThemeDef> = serde_json::from_str(LAPP).expect("lapp.json parses");
        assert!(sessh.len() >= 30 && lapp.len() == 8);
        let all = builtin();
        assert_eq!(all.len(), sessh.len() + lapp.len(), "no id is in both");
        for t in &all {
            let r = t.resolve();
            for color in [
                r.accent, r.error, r.muted, r.agent, r.bg, r.surface, r.border, r.tool, r.add,
            ] {
                assert!(matches!(color, Color::Rgb(..)), "{}: {color:?}", t.id);
            }
        }
    }

    #[test]
    fn finds_by_id_name_and_lapp_alias() {
        let all = builtin();
        assert_eq!(
            find("tokyonight", &[], &all).map(|t| t.id.as_str()),
            Some("tokyo-night")
        );
        assert_eq!(
            find("Catppuccin Mocha", &[], &all).map(|t| t.id.as_str()),
            Some("catppuccin-mocha")
        );
        assert_eq!(
            find("nightfox", &[], &all).map(|t| t.id.as_str()),
            Some("nightfox")
        );
        assert!(find("nope", &[], &all).is_none());
        assert!(find("paper", &[], &all).is_some_and(ThemeDef::is_light));
        assert!(!find("phosphor", &[], &all).is_some_and(ThemeDef::is_light));
    }

    #[test]
    fn parses_single_themes_and_exports() {
        let one = r##"{"id":"mine","name":"Mine","bg":"#000000","bgElevated":"#111111","fg":"#eeeeee",
            "fgDim":"#888888","accent":"#ff00ff","border":"#222222","danger":"#ff0000",
            "terminal":{"palette":[null,"#ff0000"]}}"##;
        let t = parse(one).expect("a theme object");
        assert_eq!(t[0].id, "mine");
        let r = t[0].resolve();
        assert_eq!(r.remove, Color::Rgb(255, 0, 0));
        assert_eq!(
            r.tool,
            Color::Rgb(255, 0, 255),
            "missing ANSI falls back to the accent"
        );
        let wrapped = format!(r#"{{"version":1,"theme":{one}}}"#);
        assert_eq!(parse(&wrapped).expect("an export")[0].id, "mine");
        assert!(parse("[1]").is_err());
    }
}
