//! Headless frame capture: render the shell off-screen and print it as ANSI.
//!
//! This is the `--capture` path. It is [`super::runtime::run`] with every
//! terminal-bound step removed — no alternate screen, no raw mode, no capability
//! handshake, no input thread — but with the same session backend, so the captured
//! frame carries the real syntax highlighting, diagnostics, and Source Control state
//! that a live session would show. The loop draws until the backend falls quiet (or
//! a deadline passes), writes the final grid to stdout, and returns.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::style::Modifier;

use super::*;
use crate::cli::CaptureSpec;

/// Render `app` off-screen and write the settled frame to stdout as ANSI.
///
/// Mirrors [`super::runtime::run`]'s backend wiring so the frame is a real render,
/// then returns instead of entering an interactive loop.
pub(crate) fn capture(mut app: App, spec: CaptureSpec) -> color_eyre::Result<()> {
    // The live shell gates on kitty keyboard support; a capture has no terminal to
    // ask, and the frame should show the shell's normal state rather than the
    // degraded one, so record the capability as present without probing for it.
    app.kitty_keyboard_supported = true;
    // Halfblocks draw images into the buffer itself. Kitty graphics would instead be
    // written out of band by `flush_graphics` (which a capture never calls), leaving
    // a hole in the grid, so pin the in-band protocol regardless of the environment.
    app.graphics = GraphicsProtocol::Halfblocks;
    app.kitty_graphics_supported = false;
    app.pointer_shapes_supported = false;

    let runtime = tokio::runtime::Runtime::new().map_err(|e| eyre!("tokio runtime: {e}"))?;
    let (session, events, snaps) = Session::new(SessionConfig {
        roots: vec![app.root.clone()],
        settings: app.settings.clone(),
        loaded_config: app.loaded_config.clone(),
        // A capture is a throwaway read-only session: never write crash-recovery
        // swaps into the user's data directory.
        swap_dir: None,
        process_supervisor: std::env::current_exe().ok(),
        lsp_registry_dir: directories::ProjectDirs::from("", "getkono", "karet")
            .map(|dirs| dirs.data_local_dir().join("language-servers")),
    });

    let mut terminal = Terminal::new(TestBackend::new(spec.cols, spec.rows))
        .map_err(|e| eyre!("capture terminal: {e}"))?;

    // Borrow rather than move, so the settled buffer and the theme are still here to
    // serialize once the runtime returns.
    runtime.block_on(drive(&mut terminal, &mut app, session, events, snaps, spec))?;

    let ansi = buffer_to_ansi(terminal.backend().buffer(), &app.theme);
    let mut out = io::stdout().lock();
    out.write_all(ansi.as_bytes())
        .map_err(|e| eyre!("write capture: {e}"))?;
    out.flush().map_err(|e| eyre!("flush capture: {e}"))?;
    Ok(())
}

/// Attach the session backend, apply the startup notifications, and settle.
///
/// This is [`super::runtime::run`]'s in-runtime prologue with the terminal-bound
/// steps dropped: the backend wiring is identical, which is what makes the captured
/// frame a real one.
async fn drive(
    terminal: &mut Terminal<TestBackend>,
    app: &mut App,
    session: Session,
    events: EventRx,
    snaps: SnapshotRx,
    spec: CaptureSpec,
) -> color_eyre::Result<()> {
    let backend: Arc<dyn Backend> = Arc::new(local(session));
    app.backend = Some(backend);
    app.register_open_tabs();
    for diag in std::mem::take(&mut app.config_diagnostics) {
        app.notify(
            diag.severity,
            NotificationKind::System,
            format!("config: {}", diag.message),
        );
    }
    let Some(prepared) = app.prepare_rx.take() else {
        return Err(eyre!("diff preparation result stream is unavailable"));
    };
    settle(terminal, app, events, snaps, prepared, spec).await
}

/// Draw until the backend stops producing work, then draw one last frame.
///
/// Each backend event, document snapshot, or diff-preparation result restarts the
/// quiet timer, so a workspace that highlights slowly still captures a settled UI.
/// Returns once nothing has arrived for `spec.settle` or `spec.timeout` has elapsed
/// — the deadline is a ceiling, not a failure, so a workspace that never falls quiet
/// still yields a frame.
async fn settle(
    terminal: &mut Terminal<TestBackend>,
    app: &mut App,
    mut events: EventRx,
    mut snaps: SnapshotRx,
    mut prepared: tokio::sync::mpsc::UnboundedReceiver<prepare::PrepareResult>,
    spec: CaptureSpec,
) -> color_eyre::Result<()> {
    let deadline = Instant::now() + spec.timeout;
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        // A capture never calls `App::flush_graphics`: it writes Kitty escapes
        // straight to stdout, which is where the captured grid is going.

        let now = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            break;
        };
        let quiet = spec.settle.min(remaining);

        // A closed channel means the backend is gone; treat that as "no more work"
        // rather than selecting on a future that is permanently ready.
        let progressed = tokio::select! {
            biased;
            event = events.recv() => match event {
                Some((id, ev)) => { app.on_backend_event(id, ev); true },
                None => false,
            },
            snap = snaps.recv() => match snap {
                Some((doc, snap)) => { app.on_snapshot(doc, &snap); true },
                None => false,
            },
            result = prepared.recv() => match result {
                Some(result) => { app.on_prepare_result(result); true },
                None => false,
            },
            () = tokio::time::sleep(quiet) => false,
        };
        if !progressed {
            break;
        }

        // Collapse the rest of the burst into this same frame.
        while let Some((id, ev)) = events.try_recv() {
            app.on_backend_event(id, ev);
        }
        while let Some((doc, snap)) = snaps.try_recv() {
            app.on_snapshot(doc, &snap);
        }
        while let Ok(result) = prepared.try_recv() {
            app.on_prepare_result(result);
        }
        app.notifications.expire(Instant::now());
    }
    terminal.draw(|f| ui::draw(f, app))?;
    Ok(())
}

/// Serialize a rendered buffer as truecolor ANSI: one line per row, every cell
/// carrying an explicit foreground and background.
///
/// Every colour is resolved to RGB against `theme`, so the output depends on the
/// theme rather than on the reader's palette — [`Color::Reset`] becomes the theme's
/// background/foreground and indexed/named colours become their xterm values. Rows
/// are emitted in full (never trimmed) and closed with a reset, so the grid is
/// exactly the buffer's width and height.
///
/// A wide glyph occupies two cells, the second holding an empty symbol; that
/// continuation cell is emitted as a space. Column alignment is therefore preserved
/// across the row, at the cost of the wide glyph itself reading as one column wide.
fn buffer_to_ansi(buffer: &Buffer, theme: &Theme) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut previous: Option<CellStyle> = None;
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let style = CellStyle::resolve(cell.style(), theme);
            if previous != Some(style) {
                style.write_sgr(&mut out);
                previous = Some(style);
            }
            match cell.symbol() {
                "" => out.push(' '),
                symbol => out.push_str(symbol),
            }
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// One cell's fully-resolved appearance: RGB foreground, RGB background, and the
/// emphasis bits the ANSI writer reproduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellStyle {
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
    bold: bool,
    italic: bool,
    underlined: bool,
    crossed_out: bool,
}

impl CellStyle {
    /// Resolve a ratatui style against the theme, folding `REVERSED` into the
    /// colours so the reader never has to reproduce terminal reverse-video rules.
    fn resolve(style: ratatui::style::Style, theme: &Theme) -> Self {
        let mut fg = rgb_of(
            style.fg.unwrap_or(Color::Reset),
            theme.role(ThemeRole::Foreground),
        );
        let mut bg = rgb_of(
            style.bg.unwrap_or(Color::Reset),
            theme.role(ThemeRole::Background),
        );
        if style.add_modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }
        Self {
            fg,
            bg,
            bold: style.add_modifier.contains(Modifier::BOLD),
            italic: style.add_modifier.contains(Modifier::ITALIC),
            underlined: style.add_modifier.contains(Modifier::UNDERLINED),
            crossed_out: style.add_modifier.contains(Modifier::CROSSED_OUT),
        }
    }

    /// Append one self-contained SGR sequence: a reset followed by every attribute
    /// this cell needs, so a reader never has to track state across sequences.
    fn write_sgr(self, out: &mut String) {
        out.push_str("\x1b[0");
        for (enabled, code) in [
            (self.bold, "1"),
            (self.italic, "3"),
            (self.underlined, "4"),
            (self.crossed_out, "9"),
        ] {
            if enabled {
                out.push(';');
                out.push_str(code);
            }
        }
        let (fr, fg, fb) = self.fg;
        let (br, bg, bb) = self.bg;
        out.push_str(&format!(";38;2;{fr};{fg};{fb};48;2;{br};{bg};{bb}m"));
    }
}

/// Resolve a ratatui colour to RGB, using `fallback` for [`Color::Reset`].
fn rgb_of(color: Color, fallback: karet_theme::Rgba) -> (u8, u8, u8) {
    match color {
        Color::Reset => (fallback.r, fallback.g, fallback.b),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => xterm_rgb(index),
        Color::Black => xterm_rgb(0),
        Color::Red => xterm_rgb(1),
        Color::Green => xterm_rgb(2),
        Color::Yellow => xterm_rgb(3),
        Color::Blue => xterm_rgb(4),
        Color::Magenta => xterm_rgb(5),
        Color::Cyan => xterm_rgb(6),
        Color::Gray => xterm_rgb(7),
        Color::DarkGray => xterm_rgb(8),
        Color::LightRed => xterm_rgb(9),
        Color::LightGreen => xterm_rgb(10),
        Color::LightYellow => xterm_rgb(11),
        Color::LightBlue => xterm_rgb(12),
        Color::LightMagenta => xterm_rgb(13),
        Color::LightCyan => xterm_rgb(14),
        Color::White => xterm_rgb(15),
    }
}

/// The xterm 256-colour palette: 16 system colours, a 6×6×6 RGB cube, 24 greys.
fn xterm_rgb(index: u8) -> (u8, u8, u8) {
    /// The 16 system colours, in xterm's default palette.
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    /// The six levels each channel of the colour cube takes.
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    match index {
        0..=15 => SYSTEM[index as usize],
        16..=231 => {
            let offset = index - 16;
            (
                LEVELS[(offset / 36) as usize],
                LEVELS[(offset % 36 / 6) as usize],
                LEVELS[(offset % 6) as usize],
            )
        },
        // 232..=255 is a 24-step grey ramp from #080808 to #eeeeee.
        _ => {
            let level = 8 + (index - 232) * 10;
            (level, level, level)
        },
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    use super::*;

    /// A buffer whose cells can be styled individually for serialization tests.
    fn buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn every_row_is_emitted_in_full_and_reset() {
        let theme = Theme::dark();
        let ansi = buffer_to_ansi(&buffer(3, 2), &theme);
        let lines: Vec<&str> = ansi.lines().collect();
        assert_eq!(lines.len(), 2, "one line per row");
        for line in lines {
            assert!(
                line.ends_with("\x1b[0m"),
                "rows close with a reset: {line:?}"
            );
            // Trailing blanks are kept, so the grid stays exactly as wide as the
            // buffer: three spaces survive between the SGR prefix and the reset.
            assert!(
                line.contains("   "),
                "blank cells are not trimmed: {line:?}"
            );
        }
    }

    #[test]
    fn reset_colors_resolve_to_the_theme() {
        let theme = Theme::dark();
        let background = theme.role(ThemeRole::Background);
        let foreground = theme.role(ThemeRole::Foreground);
        let ansi = buffer_to_ansi(&buffer(1, 1), &theme);
        assert!(
            ansi.contains(&format!(
                "38;2;{};{};{}",
                foreground.r, foreground.g, foreground.b
            )),
            "unset foreground uses the theme foreground: {ansi:?}"
        );
        assert!(
            ansi.contains(&format!(
                "48;2;{};{};{}",
                background.r, background.g, background.b
            )),
            "unset background uses the theme background: {ansi:?}"
        );
    }

    #[test]
    fn a_style_run_emits_one_sequence() {
        let theme = Theme::dark();
        let mut buf = buffer(4, 1);
        let style = Style::default().fg(Color::Rgb(1, 2, 3));
        for x in 0..4 {
            buf[(x, 0)].set_symbol("a").set_style(style);
        }
        let ansi = buffer_to_ansi(&buf, &theme);
        assert_eq!(
            ansi.matches("38;2;1;2;3").count(),
            1,
            "an unchanging style is written once per row: {ansi:?}"
        );
        assert!(ansi.contains("aaaa"));
    }

    #[test]
    fn a_style_change_emits_a_new_sequence() {
        let theme = Theme::dark();
        let mut buf = buffer(2, 1);
        buf[(0, 0)]
            .set_symbol("a")
            .set_style(Style::default().fg(Color::Rgb(1, 2, 3)));
        buf[(1, 0)]
            .set_symbol("b")
            .set_style(Style::default().fg(Color::Rgb(4, 5, 6)));
        let ansi = buffer_to_ansi(&buf, &theme);
        assert!(ansi.contains("38;2;1;2;3"));
        assert!(ansi.contains("38;2;4;5;6"));
    }

    #[test]
    fn emphasis_bits_are_written_before_the_colors() {
        let theme = Theme::dark();
        let mut buf = buffer(1, 1);
        buf[(0, 0)].set_symbol("x").set_style(
            Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC | Modifier::UNDERLINED),
        );
        let ansi = buffer_to_ansi(&buf, &theme);
        assert!(ansi.contains("\x1b[0;1;3;4;38;2;"), "got {ansi:?}");
    }

    #[test]
    fn reverse_video_is_folded_into_the_colors() {
        let theme = Theme::dark();
        let mut buf = buffer(1, 1);
        buf[(0, 0)].set_symbol("x").set_style(
            Style::default()
                .fg(Color::Rgb(1, 2, 3))
                .bg(Color::Rgb(4, 5, 6))
                .add_modifier(Modifier::REVERSED),
        );
        let ansi = buffer_to_ansi(&buf, &theme);
        // Swapped: the reader never has to implement reverse-video itself.
        assert!(ansi.contains("38;2;4;5;6"), "got {ansi:?}");
        assert!(ansi.contains("48;2;1;2;3"), "got {ansi:?}");
        assert!(!ansi.contains(";7;"), "no bare reverse attribute: {ansi:?}");
    }

    #[test]
    fn a_wide_glyphs_continuation_cell_becomes_a_space() {
        let theme = Theme::dark();
        let mut buf = buffer(3, 1);
        buf[(0, 0)].set_symbol("漢");
        // ratatui leaves the second cell of a wide glyph empty.
        buf[(1, 0)].set_symbol("");
        buf[(2, 0)].set_symbol("z");
        let ansi = buffer_to_ansi(&buf, &theme);
        assert!(
            ansi.contains("漢 z"),
            "the continuation cell keeps later columns aligned: {ansi:?}"
        );
    }

    #[test]
    fn named_and_indexed_colors_resolve_to_xterm_values() {
        let fallback = karet_theme::Rgba {
            r: 9,
            g: 9,
            b: 9,
            a: 255,
        };
        assert_eq!(rgb_of(Color::Red, fallback), (128, 0, 0));
        assert_eq!(rgb_of(Color::LightRed, fallback), (255, 0, 0));
        assert_eq!(rgb_of(Color::Indexed(1), fallback), (128, 0, 0));
        assert_eq!(rgb_of(Color::Rgb(7, 8, 9), fallback), (7, 8, 9));
        assert_eq!(rgb_of(Color::Reset, fallback), (9, 9, 9));
    }

    #[test]
    fn the_xterm_cube_and_grey_ramp_are_correct() {
        // Cube corners and one interior point.
        assert_eq!(xterm_rgb(16), (0, 0, 0));
        assert_eq!(xterm_rgb(231), (255, 255, 255));
        assert_eq!(xterm_rgb(21), (0, 0, 255));
        assert_eq!(xterm_rgb(46), (0, 255, 0));
        // Grey ramp endpoints.
        assert_eq!(xterm_rgb(232), (8, 8, 8));
        assert_eq!(xterm_rgb(255), (238, 238, 238));
    }
}
