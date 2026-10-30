//! Terminal-independent text styling. The binary maps these onto ratatui;
//! the app model and its tests never see a terminal type.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    White,
    Rgb(u8, u8, u8),
}

impl Color {
    /// Parses a named colour or `#rrggbb`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#')
            && hex.len() == 6
            && let Ok(v) = u32::from_str_radix(hex, 16)
        {
            return Some(Self::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        Some(match s.to_ascii_lowercase().as_str() {
            "reset" | "default" => Self::Reset,
            "black" => Self::Black,
            "red" => Self::Red,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "blue" => Self::Blue,
            "magenta" => Self::Magenta,
            "cyan" => Self::Cyan,
            "gray" | "grey" => Self::Gray,
            "darkgray" | "darkgrey" | "dim" => Self::DarkGray,
            "white" => Self::White,
            _ => return None,
        })
    }

    /// `self` moved `t` (0..=1, or past 1 to overshoot) of the way to
    /// `other`; only RGB colours mix.
    pub fn mix(self, other: Color, t: f32) -> Option<Color> {
        let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (self, other) else {
            return None;
        };
        let m = |a: u8, b: u8| {
            (f32::from(a) + (f32::from(b) - f32::from(a)) * t)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Some(Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reversed: bool,
    /// Part of a URL, which a click opens; one id per URL, so a URL a
    /// wrap split over rows is put back together (`0`: not a link).
    pub link: u32,
    /// Bars, gutters and borders: drawn, but left out of copied text.
    pub deco: bool,
}

impl Style {
    pub const fn new() -> Self {
        Self {
            fg: Color::Reset,
            bg: Color::Reset,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            reversed: false,
            link: 0,
            deco: false,
        }
    }
    pub const fn fg(mut self, c: Color) -> Self {
        self.fg = c;
        self
    }
    pub const fn bg(mut self, c: Color) -> Self {
        self.bg = c;
        self
    }
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
    pub const fn reversed(mut self) -> Self {
        self.reversed = true;
        self
    }
    pub const fn deco(mut self) -> Self {
        self.deco = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(text, Style::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn new(spans: Vec<Span>) -> Self {
        Self { spans }
    }
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(vec![Span::raw(text)])
    }
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(vec![Span::new(text, style)])
    }
    pub fn push(&mut self, span: Span) {
        self.spans.push(span);
    }
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| crate::wrap::width(&s.text)).sum()
    }
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

impl From<&str> for Line {
    fn from(s: &str) -> Self {
        Self::raw(s)
    }
}
