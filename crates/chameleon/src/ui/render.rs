//! Maps the app's terminal-free view-model onto ratatui widgets.

use chameleon_app::Screen;
use chameleon_app::config::Theme;
use chameleon_app::style::{Color, Line, Style};
use chameleon_app::view::{OverlayView, PaneView, Placed, Rect};
use ratatui::Frame;
use ratatui::layout::Rect as RRect;
use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

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

/// One row of text at an absolute position, clipped to the frame.
fn row(frame: &mut Frame, x: u16, y: u16, l: &Line) {
    let full = frame.area();
    if y >= full.height || x >= full.width {
        return;
    }
    let w = (l.width() as u16).min(full.width - x);
    if w == 0 {
        return;
    }
    frame.render_widget(Paragraph::new(line(l)), RRect::new(x, y, w, 1));
}

fn placed(frame: &mut Frame, p: &Placed) {
    row(frame, p.x, p.y, &p.line);
}

fn pane(frame: &mut Frame, p: &PaneView, theme: &Theme) {
    if p.rect.is_empty() {
        return;
    }
    let area = rect(p.rect);
    let lines: Vec<RLine> = p.lines.iter().map(line).collect();
    frame.render_widget(Paragraph::new(lines), area);
    if let Some(sb) = &p.scrollbar
        && sb.total > sb.visible
        && area.x + area.width < frame.area().width
    {
        // The column of air right of the text is the scrollbar's track.
        let track = RRect::new(area.x + area.width, area.y, 1, area.height);
        let mut state = ScrollbarState::new(sb.total.saturating_sub(sb.visible)).position(sb.offset);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("┆"))
                .thumb_symbol("┃")
                .track_style(RStyle::default().fg(color(theme.muted)))
                .thumb_style(RStyle::default().fg(color(theme.accent))),
            track,
            &mut state,
        );
    }
    if let Some((x, y)) = p.cursor {
        frame.set_cursor_position((x, y));
    }
}

/// The theme's text and background colours, where it sets them.
fn base(theme: &Theme, bg: Color) -> RStyle {
    style(Style::new().fg(theme.agent).bg(bg))
}

/// The background of raised things: the theme's surface, or its screen.
fn surface(theme: &Theme) -> Color {
    if theme.surface == Color::Reset {
        theme.bg
    } else {
        theme.surface
    }
}

fn overlay(frame: &mut Frame, o: &OverlayView, theme: &Theme) {
    let area = rect(o.rect);
    if area.is_empty() {
        return;
    }
    frame.render_widget(Clear, area);
    let tone = color(o.tone.unwrap_or(theme.accent));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(base(theme, surface(theme)))
        .border_style(RStyle::default().fg(tone))
        .title(RLine::from(RSpan::styled(
            format!(" {} ", o.title),
            RStyle::default().fg(tone).add_modifier(Modifier::BOLD),
        )));
    if let Some(f) = &o.footer {
        // The hints live in the bottom border, so the body keeps every row.
        let mut l = line(f);
        l.spans.insert(0, RSpan::raw(" "));
        l.spans.push(RSpan::raw(" "));
        block = block.title_bottom(l.right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines: Vec<RLine> = o.lines.iter().map(line).collect();
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some((x, y)) = o.cursor {
        frame.set_cursor_position((x, y));
    }
}

pub fn draw(frame: &mut Frame, screen: &Screen, theme: &Theme) {
    let full = frame.area();
    if full.height == 0 {
        return;
    }
    // The theme's screen, then raised bands for the header and status bar.
    frame.render_widget(Block::default().style(base(theme, theme.bg)), full);
    if theme.surface != Color::Reset {
        let band = Block::default().style(base(theme, theme.surface));
        frame.render_widget(band.clone(), RRect::new(0, 0, full.width, 1));
        frame.render_widget(band, RRect::new(0, full.height - 1, full.width, 1));
    }
    row(frame, 0, 0, &screen.header);
    for p in &screen.chrome {
        placed(frame, p);
    }
    if let Some(sb) = &screen.sidebar {
        pane(frame, sb, theme);
    }
    pane(frame, &screen.transcript, theme);
    pane(frame, &screen.composer, theme);
    let status = RRect::new(0, full.height - 1, full.width, 1);
    frame.render_widget(Paragraph::new(line(&screen.status)), status);
    // Toasts sit in the transcript's top-right corner.
    let t = rect(screen.transcript.rect);
    for (i, toast) in screen.toasts.iter().enumerate() {
        let text = toast.text();
        let w = (chameleon_app::wrap::width(&text) as u16 + 4).min(t.width.saturating_sub(2));
        if w == 0 || t.height < 2 {
            break;
        }
        let area = RRect::new(t.x + t.width - w, t.y + i as u16, w, 1);
        if area.y >= t.y + t.height {
            break;
        }
        frame.render_widget(Clear, area);
        let mut l = line(toast);
        l.spans
            .insert(0, RSpan::styled(" ✦ ", RStyle::default().fg(color(theme.accent))));
        l.spans.push(RSpan::raw(" "));
        let bg = match surface(theme) {
            Color::Reset => RColor::Black,
            c => color(c),
        };
        frame.render_widget(Paragraph::new(l).style(base(theme, theme.bg).bg(bg)), area);
    }
    if let Some(o) = &screen.overlay {
        overlay(frame, o, theme);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use chameleon_app::config::Config;
    use chameleon_app::keymap::{Key, KeyCode};
    use chameleon_app::{App, AppEvent, ExecOutput, ExecTag};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const CLAUDE_HI: &str =
        include_str!("../../../../../acc/crates/acc-core/tests/fixtures/claude/hi-and-controls.ndjson");

    const CLAUDE_PERMISSION: &str =
        include_str!("../../../../../acc/crates/acc-core/tests/fixtures/claude/permission-deny.ndjson");

    fn app(w: u16, h: u16) -> App {
        app_with(w, h, CLAUDE_HI)
    }

    /// An app with one attached Claude session that replayed `log`, the
    /// way the replay example does it.
    fn app_with(w: u16, h: u16, log: &str) -> App {
        let mut app = App::new(Config::default(), "test");
        app.handle(AppEvent::Resize { width: w, height: h }, 0);
        let req = acc_core::session::LaunchRequest {
            harness: Some(acc_core::Harness::Claude),
            cwd: "/w/proj".into(),
            ..Default::default()
        };
        let (session, _) = acc_core::session::Session::launch(req, "s1", "test", 0).unwrap();
        let spec = session.spec().to_json();
        let ok = |s: String| {
            Ok(ExecOutput {
                status: Some(0),
                stdout: s.into_bytes(),
                stderr: Vec::new(),
            })
        };
        app.handle(
            AppEvent::ExecDone {
                host: "local".into(),
                tag: ExecTag::List,
                result: ok(format!("S\ts1\talive 1 {}\t0\nJ\t{spec}\n", log.len())),
            },
            1,
        );
        app.handle(
            AppEvent::ExecDone {
                host: "local".into(),
                tag: ExecTag::Status("s1".into()),
                result: ok(format!("alive 1 {}\n", log.len())),
            },
            2,
        );
        for l in log.lines() {
            app.handle(
                AppEvent::FeedLine {
                    host: "local".into(),
                    key: "l:s1".into(),
                    bytes: format!("{l}\n").into_bytes(),
                },
                3,
            );
        }
        app
    }

    fn paint(app: &mut App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let screen = app.screen();
        let theme = *app.theme();
        term.draw(|f| draw(f, &screen, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
            .collect()
    }

    #[test]
    fn chrome_joins_up_and_the_list_collapses() {
        let (w, h) = (80, 20);
        let mut a = app(w, h);
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        let sb = 34usize; // the default sidebar width; its last column is the rule
        assert!(rows[0].starts_with(" ◆ chameleon "), "{:?}", rows[0]);
        assert_eq!(rows[1].chars().nth(sb - 1), Some('┬'));
        assert!(
            rows[2..]
                .iter()
                .take(15)
                .all(|r| r.chars().nth(sb - 1) == Some('│') || r.contains('├'))
        );
        let rule = rows
            .iter()
            .position(|r| r.contains('├'))
            .expect("the prompt's rule");
        assert_eq!(rows[rule + 1].chars().nth(sb + 1), Some('❯'));
        assert!(rows[rule + 1].chars().nth(sb - 1) == Some('│'));
        assert!(rows[2].contains("local"), "{:?}", rows[2]);
        assert!(
            rows[2].chars().nth(sb - 2) == Some(' '),
            "air before the rule: {:?}",
            rows[2]
        );
        assert!(
            rows[2][..].contains("│ hi"),
            "the transcript starts after the rule: {:?}",
            rows[2]
        );

        a.handle(AppEvent::Key(Key::ctrl(KeyCode::Char('b'))), 4);
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        assert!(!rows[1].contains('┬'));
        let body = &rows[2..h as usize - 1];
        assert!(
            body.iter().all(|r| !r.contains('│')),
            "no vertical rule left in the body"
        );
        assert!(
            rows.iter().any(|r| r.starts_with(" ❯")),
            "the prompt spans the width"
        );
        assert!(rows[0].contains("ctrl-b"), "{:?}", rows[0]);
    }

    #[test]
    fn overlays_keep_their_footer_in_the_border() {
        let (w, h) = (80, 20);
        let mut a = app(w, h);
        a.handle(AppEvent::Key(Key::new(KeyCode::Esc)), 4);
        a.handle(AppEvent::Key(Key::char('?')), 4);
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        assert!(rows.iter().any(|r| r.contains("╭") && r.contains(" help ")));
        assert!(rows.iter().any(|r| r.contains("╰") && r.contains("esc close")));
    }

    fn key(a: &mut App, k: Key) -> Vec<chameleon_app::Effect> {
        a.handle(AppEvent::Key(k), 5)
    }

    #[test]
    fn approvals_are_cards_inline_and_in_the_overlay() {
        let (w, h) = (100, 32);
        // Up to (not including) the tool result, so the request stays open.
        let partial: String = CLAUDE_PERMISSION
            .lines()
            .take_while(|l| !l.contains("\"tool_result\""))
            .map(|l| format!("{l}\n"))
            .collect();
        let mut a = app_with(w, h, &partial);
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        assert!(
            rows.iter().any(|r| r.contains("╭─ ⚠ approval")),
            "the inline card"
        );
        assert!(rows.iter().any(|r| r.contains("y approve · N deny")));
        assert!(
            rows.iter().any(|r| r.contains("│ $ touch acc-probe.txt")),
            "the command it's for"
        );

        key(&mut a, Key::char('N'));
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        assert!(rows.iter().any(|r| r.contains("╭ approval ")));
        assert!(
            rows.iter().any(|r| r.contains("▎ $ touch acc-probe.txt")),
            "the preview"
        );
        let chosen = rows.iter().find(|r| r.contains('❯')).expect("a chosen option");
        assert!(chosen.contains("Deny") || chosen.contains("No"), "{chosen}");
        let top = rows.iter().position(|r| r.contains("╭ approval ")).unwrap_or(0);
        let bottom = rows
            .iter()
            .position(|r| r.contains("esc later"))
            .unwrap_or(h as usize);
        assert!(bottom - top < 14, "the card fits its content: {top}..{bottom}");
    }

    #[test]
    fn the_launch_wizard_lays_out_its_fields() {
        let (w, h) = (100, 32);
        let mut a = app(w, h);
        key(&mut a, Key::char('n'));
        // Chips keep their words together with no-break spaces.
        let plain = |rows: Vec<String>| -> Vec<String> {
            rows.into_iter().map(|r| r.replace('\u{a0}', " ")).collect()
        };
        let rows = plain(paint(&mut a, w, h));
        println!("{}", rows.join("\n"));
        assert!(rows.iter().any(|r| r.contains("╭ new session ")));
        assert!(
            rows.iter().any(|r| r.contains("❯ directory")),
            "the directory has focus"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("Claude Code") && r.contains("harness"))
        );
        assert!(rows.iter().any(|r| r.contains("┃ optional")));
        // On the permission field, Claude's modes are choices.
        key(&mut a, Key::new(KeyCode::Down));
        key(&mut a, Key::new(KeyCode::Down));
        key(&mut a, Key::new(KeyCode::Right));
        key(&mut a, Key::new(KeyCode::Right));
        let rows = paint(&mut a, w, h);
        println!("{}", rows.join("\n"));
        let perm = rows
            .iter()
            .find(|r| r.contains("❯ permission"))
            .expect("permission row");
        assert!(perm.contains("acceptEdits"), "{perm}");
        assert!(rows.iter().any(|r| r.contains("dontAsk")), "the modes are listed");
    }

    #[test]
    fn the_theme_picker_previews_and_reverts() {
        let (w, h) = (90, 36);
        let mut a = app(w, h);
        assert_eq!(a.theme().bg, Color::Reset);
        key(&mut a, Key::char('T'));
        key(&mut a, Key::new(KeyCode::Down));
        let previewing = *a.theme();
        assert!(matches!(previewing.bg, Color::Rgb(..)), "moving previews a theme");
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let screen = a.screen();
        term.draw(|f| draw(f, &screen, &previewing)).unwrap();
        let buf = term.backend().buffer().clone();
        assert_eq!(
            buf[(0, 5)].bg,
            color(previewing.bg),
            "the screen takes the theme's background"
        );
        assert_eq!(buf[(0, 0)].bg, color(previewing.surface), "the header is raised");
        let rows: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
            .collect();
        println!("{}", rows.join("\n"));
        assert!(rows.iter().any(|r| r.contains("╭ theme ")));
        assert!(rows.iter().any(|r| r.contains("light")) || rows.iter().any(|r| r.contains("dark")));
        key(&mut a, Key::new(KeyCode::Esc));
        assert_eq!(a.theme().bg, Color::Reset, "esc reverts");

        key(&mut a, Key::char('T'));
        key(&mut a, Key::new(KeyCode::Down));
        let name = a.theme_name().map(str::to_owned);
        let fx = key(&mut a, Key::new(KeyCode::Enter));
        assert!(name.is_some());
        assert!(fx.contains(&chameleon_app::Effect::SaveTheme { name }), "{fx:?}");
    }
}
