//! View-models: everything the terminal needs to paint one frame, with no
//! terminal types. The binary maps `Line`s onto ratatui and draws boxes.

pub mod composer;
pub mod overlay;
pub mod sidebar;
pub mod status;
pub mod transcript;

use crate::model::{App, Pane};
use crate::slot::SessionKey;
use crate::style::Line;

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

/// A bordered pane with its content already fitted to the inner area.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneView {
    pub rect: Rect,
    pub title: Line,
    pub lines: Vec<Line>,
    pub focused: bool,
    /// Absolute terminal position of the text cursor.
    pub cursor: Option<(u16, u16)>,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub sidebar: Option<PaneView>,
    pub transcript: PaneView,
    pub composer: PaneView,
    pub status: Line,
    pub overlay: Option<OverlayView>,
    pub toasts: Vec<Line>,
}

impl App {
    /// Builds the frame for the current size.
    pub fn screen(&mut self) -> Screen {
        let (w, h) = self.size;
        let status_rect = Rect::new(0, h.saturating_sub(1), w, 1);
        let body_h = h.saturating_sub(1);

        let want_sidebar = w >= 50 || self.focus == Pane::Sidebar;
        let sidebar_w = if want_sidebar {
            self.cfg.general.sidebar_width.clamp(20, w / 2).min(w)
        } else {
            0
        };
        let sidebar_rect = Rect::new(0, 0, sidebar_w, body_h);
        let main_x = sidebar_w;
        let main_w = w.saturating_sub(sidebar_w);

        let composer_inner_w = main_w.saturating_sub(2) as usize;
        let composer_lines = composer::wrapped_lines(&self.composer, composer_inner_w.max(1));
        let composer_h = (composer_lines.len().clamp(1, 6) as u16 + 2).min(body_h);
        let composer_rect = Rect::new(main_x, body_h.saturating_sub(composer_h), main_w, composer_h);
        let transcript_rect = Rect::new(main_x, 0, main_w, body_h.saturating_sub(composer_h));

        let sidebar = want_sidebar.then(|| sidebar::build(self, sidebar_rect));
        let transcript = transcript::build(self, transcript_rect);
        let composer = composer::build(self, composer_rect, composer_lines);
        let status = status::build(self, status_rect.w as usize);
        let overlay = overlay::build(self, Rect::new(0, 0, w, body_h));
        let toasts = self.toasts.iter().map(|t| t.line.clone()).collect();
        self.layout = crate::model::Layout {
            sidebar: sidebar.as_ref().map(|s| s.rect),
            sidebar_hits: sidebar.as_ref().map(|s| s.hits.clone()).unwrap_or_default(),
            transcript: transcript_rect,
            composer: composer_rect,
        };
        Screen {
            sidebar,
            transcript,
            composer,
            status,
            overlay,
            toasts,
        }
    }
}
