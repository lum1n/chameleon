//! Maps the app's terminal-free view-model onto ratatui widgets.

use chameleon_app::Screen;
use chameleon_app::config::Theme;
use chameleon_app::style::{Color, Line, Style};
use chameleon_app::view::{OverlayView, PaneView, Rect};
use ratatui::Frame;
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

fn color(c: Color) -> RColor {
    match c {
        Color::Reset => RColor::Reset,
        Color::Black => RColor::Black,
        Color::Red => RColor::Red,
        Color::Green => RColor::Green,
        Color::Yellow => RColor::Yellow,
        Color::Blue => RColor::Blue,
        Color::Magenta => RColor::Magenta,
        Color::Cyan => RColor::Cyan,
        Color::Gray => RColor::Gray,
        Color::DarkGray => RColor::DarkGray,
        Color::White => RColor::White,
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
    }
}

fn style(s: Style) -> RStyle {
    let mut r = RStyle::default();
    if s.fg != Color::Reset {
        r = r.fg(color(s.fg));
    }
    if s.bg != Color::Reset {
        r = r.bg(color(s.bg));
    }
    let mut m = Modifier::empty();
    if s.bold {
        m |= Modifier::BOLD;
    }
    if s.dim {
        m |= Modifier::DIM;
    }
    if s.italic {
        m |= Modifier::ITALIC;
    }
    if s.underline {
        m |= Modifier::UNDERLINED;
    }
    if s.reversed {
        m |= Modifier::REVERSED;
    }
    r.add_modifier(m)
}

fn line(l: &Line) -> RLine<'static> {
    RLine::from(
        l.spans
            .iter()
            .map(|s| RSpan::styled(s.text.clone(), style(s.style)))
            .collect::<Vec<_>>(),
    )
}

fn rect(r: Rect) -> RRect {
    RRect::new(r.x, r.y, r.w, r.h)
}

fn pane(frame: &mut Frame, p: &PaneView, theme: &Theme) {
    if p.rect.is_empty() {
        return;
    }
    let border = if p.focused {
        RStyle::default().fg(color(theme.accent))
    } else {
        RStyle::default().fg(color(theme.muted))
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(line(&p.title));
    let area = rect(p.rect);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<RLine> = p.lines.iter().map(line).collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some(sb) = &p.scrollbar
        && sb.total > sb.visible
    {
        let mut state = ScrollbarState::new(sb.total.saturating_sub(sb.visible)).position(sb.offset);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_style(RStyle::default().fg(color(theme.muted))),
            area,
            &mut state,
        );
    }
    if let Some((x, y)) = p.cursor {
        frame.set_cursor_position((x, y));
    }
}

fn overlay(frame: &mut Frame, o: &OverlayView, theme: &Theme) {
    let area = rect(o.rect);
    if area.is_empty() {
        return;
    }
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(RStyle::default().fg(color(theme.accent)))
        .title(format!(" {} ", o.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let body = RRect {
        height: inner.height.saturating_sub(u16::from(o.footer.is_some())),
        ..inner
    };
    let lines: Vec<RLine> = o.lines.iter().map(line).collect();
    frame.render_widget(Paragraph::new(lines), body);
    if let Some(f) = &o.footer
        && inner.height > 1
    {
        let fr = RRect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        frame.render_widget(Paragraph::new(line(f)), fr);
    }
    if let Some((x, y)) = o.cursor {
        frame.set_cursor_position((x, y));
    }
}

pub fn draw(frame: &mut Frame, screen: &Screen, theme: &Theme) {
    if let Some(sb) = &screen.sidebar {
        pane(frame, sb, theme);
    }
    pane(frame, &screen.transcript, theme);
    pane(frame, &screen.composer, theme);
    let full = frame.area();
    if full.height > 0 {
        let status = RRect::new(0, full.height - 1, full.width, 1);
        frame.render_widget(Paragraph::new(line(&screen.status)), status);
    }
    // Toasts sit in the transcript's top-right corner.
    let t = rect(screen.transcript.rect);
    for (i, toast) in screen.toasts.iter().enumerate() {
        let text = toast.text();
        let w = (chameleon_app::wrap::width(&text) as u16 + 2).min(t.width.saturating_sub(2));
        if w == 0 || t.height < 3 {
            break;
        }
        let area = RRect::new(t.x + t.width - w - 1, t.y + 1 + i as u16, w, 1);
        if area.y >= t.y + t.height - 1 {
            break;
        }
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(line(toast)).style(RStyle::default().bg(RColor::Black)),
            area,
        );
    }
    if let Some(o) = &screen.overlay {
        overlay(frame, o, theme);
    }
}
