//! View-models: everything the terminal needs to paint one frame, with no
//! terminal types. The binary maps `Line`s onto ratatui and paints them.
//!
//! The frame is one header line, a body and one status line. The body is
//! the session list on the left (optional), the transcript and, under a
//! rule, the prompt. Panes have no borders; thin rules carry the focus.
//!
//! ```text
//!  ◆ chameleon  fix the tests  ~/proj · Claude Code · opus        ⚠1 ⠋1 ·2
//! ─────────────────┬─────────────────────────────────────────────────────────
//! ▌1 ⚠ fix the te… │ ▎fix the failing tests
//!  2 ⠋ bump deps   │
//!                  ├─────────────────────────────────────────────────────────
//!                  │ ❯ say something
//!  ⚠ approval  Claude Code │ model opus │ ctx ▰▰▰▱▱▱ 42%
//! ```

pub mod composer;
pub mod header;
pub mod overlay;
pub mod sidebar;
pub mod status;
pub mod transcript;

use crate::model::{App, Pane};
use crate::slot::SessionKey;
use crate::style::{Line, Span, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }
    /// The area inside a one-cell border.
    pub fn inner(self) -> Self {
        Self {
            x: self.x.saturating_add(1),
            y: self.y.saturating_add(1),
            w: self.w.saturating_sub(2),
            h: self.h.saturating_sub(2),
        }
    }
    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: usize,
    pub offset: usize,
    pub visible: usize,
}

/// A borderless pane with its content already fitted to `rect`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneView {
    pub rect: Rect,
    pub lines: Vec<Line>,
    pub focused: bool,
    /// Absolute terminal position of the text cursor.
    pub cursor: Option<(u16, u16)>,
    /// Drawn in the column right of `rect`.
    pub scrollbar: Option<Scrollbar>,
    /// For the sidebar: the session on each line, for mouse clicks.
    pub hits: Vec<Option<SessionKey>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayView {
    pub rect: Rect,
    pub title: String,
    pub lines: Vec<Line>,
    pub footer: Option<Line>,
    pub cursor: Option<(u16, u16)>,
    /// Border and title colour; the accent when `None`.
    pub tone: Option<crate::style::Color>,
}

/// A styled line painted at an absolute position: rules and junctions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub x: u16,
    pub y: u16,
    pub line: Line,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub header: Line,
    /// Rules between the panes; painted before them.
    pub chrome: Vec<Placed>,
    pub sidebar: Option<PaneView>,
    pub transcript: PaneView,
    pub composer: PaneView,
    pub status: Line,
    pub overlay: Option<OverlayView>,
    pub toasts: Vec<Line>,
}

/// Narrower than this, the session list only shows while it has focus.
const SIDEBAR_MIN_COLS: u16 = 50;
/// Lines the prompt grows to before it scrolls.
const COMPOSER_MAX_LINES: usize = 6;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner frame for a moment in time.
pub fn spinner(now_ms: u64) -> &'static str {
    SPINNER[(now_ms / 100) as usize % SPINNER.len()]
}

impl App {
    /// Builds the frame for the current size.
    pub fn screen(&mut self) -> Screen {
        let (w, h) = self.size;
        let theme = self.theme;
        let header_rule_y = 1;
        let body_y = 2;
        let body_h = h.saturating_sub(3); // header, its rule, status

        let show_sidebar = self.sidebar_shown && (w >= SIDEBAR_MIN_COLS || self.focus == Pane::Sidebar);
        // The sidebar's last column carries the vertical rule.
        let sidebar_w = if show_sidebar {
            self.cfg.general.sidebar_width.clamp(20, w / 2).min(w)
        } else {
            0
        };
        // ... and the one before it is air, so text never touches it.
        let sidebar_rect = Rect::new(0, body_y, sidebar_w.saturating_sub(2), body_h);
        let main_x = sidebar_w;
        let main_w = w.saturating_sub(sidebar_w);
        // One column of air on each side of the text; the right one holds
        // the transcript's scrollbar.
        let text_x = main_x.saturating_add(1);
        let text_w = main_w.saturating_sub(2);

        let composer_text_w = (text_w as usize).saturating_sub(composer::PROMPT_W).max(1);
        let composer_lines = composer::wrapped_lines(&self.composer, composer_text_w);
        let composer_h =
            (composer_lines.len().clamp(1, COMPOSER_MAX_LINES) as u16).min(body_h.saturating_sub(1));
        let composer_y = (body_y + body_h).saturating_sub(composer_h);
        let composer_rect = Rect::new(text_x, composer_y, text_w, composer_h);
        let rule_y = composer_y.saturating_sub(1);
        let transcript_rect = Rect::new(text_x, body_y, text_w, rule_y.saturating_sub(body_y));

        let sidebar = show_sidebar.then(|| sidebar::build(self, sidebar_rect));
        let transcript = transcript::build(self, transcript_rect);
        let composer = composer::build(self, composer_rect);
        let header = header::build(self, w as usize);
        let status = status::build(self, w as usize);
        let overlay = overlay::build(self, Rect::new(0, 0, w, h.saturating_sub(1)));
        let toasts = self.toasts.iter().map(|t| t.line.clone()).collect();

        // Rules: each pane owns one, lit in the accent while it has focus.
        let rule = |focused: bool| Style::new().fg(if focused { theme.accent } else { theme.border });
        let top = rule(self.focus == Pane::Transcript);
        let side = rule(self.focus == Pane::Sidebar);
        let bottom = rule(self.focus == Pane::Composer);
        let mut chrome = Vec::new();
        if h > header_rule_y {
            let mut line = Line::default();
            if show_sidebar && sidebar_w > 0 {
                line.push(Span::new("─".repeat(sidebar_w as usize - 1), top));
                line.push(Span::new(
                    "┬",
                    if self.focus == Pane::Sidebar { side } else { top },
                ));
                line.push(Span::new("─".repeat(main_w as usize), top));
            } else {
                line.push(Span::new("─".repeat(w as usize), top));
            }
            chrome.push(Placed {
                x: 0,
                y: header_rule_y,
                line,
            });
        }
        if show_sidebar && sidebar_w > 0 {
            let x = sidebar_w - 1;
            for y in body_y..body_y + body_h {
                if y == rule_y && body_h > 1 {
                    continue;
                }
                chrome.push(Placed {
                    x,
                    y,
                    line: Line::styled("│", side),
                });
            }
        }
        if body_h > 1 {
            let mut line = Line::default();
            let x = if show_sidebar && sidebar_w > 0 {
                line.push(Span::new(
                    "├",
                    if self.focus == Pane::Composer {
                        bottom
                    } else {
                        side
                    },
                ));
                sidebar_w - 1
            } else {
                main_x
            };
            line.push(Span::new("─".repeat(main_w as usize), bottom));
            chrome.push(Placed { x, y: rule_y, line });
        }

        self.layout = crate::model::Layout {
            sidebar: sidebar.as_ref().map(|s| s.rect),
            sidebar_hits: sidebar.as_ref().map(|s| s.hits.clone()).unwrap_or_default(),
            transcript: transcript_rect,
            composer: composer_rect,
        };
        Screen {
            header,
            chrome,
            sidebar,
            transcript,
            composer,
            status,
            overlay,
            toasts,
        }
    }
}
