//! Painting an integrated terminal's grid into its pane.
//!
//! Default foreground/background follow the editor theme so a terminal sits
//! flush with the panes around it; the sixteen ANSI colours and the 256-colour
//! cube stay *indexed*, so they render in the host terminal's own palette, and
//! a program's 24-bit colours pass through as RGB. OSC 8 hyperlinks the
//! program emitted are re-emitted to the host (through [`super::osc8`]) and
//! underlined.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::Color as AnsiColor;
use alacritty_terminal::vte::ansi::CursorShape;
use alacritty_terminal::vte::ansi::NamedColor;
use alacritty_terminal::vte::ansi::Rgb;
use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;

use crate::terminal::TerminalPane;

/// The theme colours a terminal grid is painted against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GridPalette {
    /// The default foreground (`NamedColor::Foreground`).
    pub(super) fg: Color,
    /// The default background (`NamedColor::Background`).
    pub(super) bg: Color,
    /// The selection highlight background.
    pub(super) selection: Color,
}

impl GridPalette {
    pub(super) fn from_theme(theme: &Theme) -> Self {
        Self {
            fg: theme.role(ThemeRole::Foreground).to_ratatui(),
            bg: theme.role(ThemeRole::Background).to_ratatui(),
            selection: theme.role(ThemeRole::Selection).to_ratatui(),
        }
    }
}

/// Paint `pane` into `area`, sizing its grid (and PTY) to the area first. The
/// cursor shows only when `focused`. An exited shell keeps its last screen, with
/// a notice in the bottom row.
pub(super) fn draw_terminal(
    buf: &mut Buffer,
    theme: &Theme,
    area: Rect,
    pane: &mut TerminalPane,
    focused: bool,
) {
    let palette = GridPalette::from_theme(theme);
    buf.set_style(area, Style::default().fg(palette.fg).bg(palette.bg));
    let notice = pane
        .exit_status()
        .map(|status| format!("[shell {status} - press any key to close this terminal]"));
    let grid_area = if notice.is_some() {
        Rect {
            height: area.height.saturating_sub(1),
            ..area
        }
    } else {
        area
    };
    pane.area = grid_area;
    if grid_area.width > 0 && grid_area.height > 0 {
        pane.resize(grid_area.width, grid_area.height);
    }
    // Before reading the grid, so output parsed after this read is announced.
    pane.acknowledge_output();
    let links = paint_grid(buf, grid_area, &pane.term(), &palette, focused);
    for (row, start, end, uri) in links {
        super::osc8::link_row(buf, row, start, end, &uri);
    }
    if let Some(notice) = notice
        && area.height > 0
    {
        let row = area.y + area.height - 1;
        buf.set_stringn(
            area.x,
            row,
            notice,
            usize::from(area.width),
            theme.style(ThemeRole::Muted).bg(palette.bg),
        );
    }
}

/// One OSC 8 run to re-emit: the buffer row, the first and one-past-last
/// buffer column, and the target.
type LinkRun = (u16, u16, u16, String);

/// Paint the visible part of `term`'s grid (honouring its scroll position) into
/// `area`, returning the hyperlink runs to wrap in OSC 8 afterwards.
pub(super) fn paint_grid<T: alacritty_terminal::event::EventListener>(
    buf: &mut Buffer,
    area: Rect,
    term: &Term<T>,
    palette: &GridPalette,
    show_cursor: bool,
) -> Vec<LinkRun> {
    let content = term.renderable_content();
    let offset = i32::try_from(content.display_offset).unwrap_or(i32::MAX);
    let cursor = (show_cursor && content.cursor.shape != CursorShape::Hidden)
        .then_some(content.cursor.point);
    let columns = term.columns().min(usize::from(area.width));
    let mut links: Vec<LinkRun> = Vec::new();
    for indexed in content.display_iter {
        let Ok(row) = u16::try_from(indexed.point.line.0.saturating_add(offset)) else {
            continue;
        };
        let column = indexed.point.column.0;
        if row >= area.height || column >= columns {
            continue;
        }
        let Ok(column) = u16::try_from(column) else {
            continue;
        };
        let (x, y) = (area.x + column, area.y + row);
        let cell = indexed.cell;
        let selected = content
            .selection
            .is_some_and(|range| range.contains(indexed.point));
        let mut style = cell_style(
            cell.fg,
            cell.bg,
            cell.flags,
            content.colors,
            palette,
            selected,
        );
        if cursor == Some(indexed.point) {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let uri = cell
            .hyperlink()
            .and_then(|link| crate::links::safe_external(link.uri()).map(str::to_owned));
        if uri.is_some() {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        let Some(target) = buf.cell_mut((x, y)) else {
            continue;
        };
        target.set_style(style);
        // The second half of a wide glyph: ratatui skips it after the glyph.
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' {
            target.set_symbol(" ");
        } else {
            let mut symbol = String::from(cell.c);
            if let Some(extra) = cell.zerowidth() {
                symbol.extend(extra);
            }
            target.set_symbol(&symbol);
        }
        if let Some(uri) = uri {
            match links.last_mut() {
                Some((r, _, end, last)) if *r == y && *end == x && *last == uri => *end = x + 1,
                _ => links.push((y, x, x + 1, uri)),
            }
        }
    }
    links
}

/// The ratatui style of one grid cell.
pub(super) fn cell_style(
    fg: AnsiColor,
    bg: AnsiColor,
    flags: Flags,
    colors: &Colors,
    palette: &GridPalette,
    selected: bool,
) -> Style {
    let mut fg = resolve(fg, colors, palette);
    let mut bg = resolve(bg, colors, palette);
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if selected {
        bg = palette.selection;
    }
    let mut style = Style::default().fg(fg).bg(bg);
    for (flag, modifier) in [
        (Flags::BOLD, Modifier::BOLD),
        (Flags::DIM, Modifier::DIM),
        (Flags::ITALIC, Modifier::ITALIC),
        (Flags::STRIKEOUT, Modifier::CROSSED_OUT),
    ] {
        if flags.contains(flag) {
            style = style.add_modifier(modifier);
        }
    }
    if flags.intersects(Flags::ALL_UNDERLINES) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

fn rgb(color: Rgb) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

/// A grid colour as a ratatui colour: a program-redefined palette entry (OSC 4)
/// wins, the default fg/bg follow the theme, and the indexed colours stay
/// indexed so the host terminal's palette paints them.
pub(super) fn resolve(color: AnsiColor, colors: &Colors, palette: &GridPalette) -> Color {
    match color {
        AnsiColor::Spec(spec) => rgb(spec),
        AnsiColor::Indexed(index) => colors[usize::from(index)].map_or(Color::Indexed(index), rgb),
        AnsiColor::Named(named) => {
            if let Some(spec) = colors[named] {
                return rgb(spec);
            }
            let index = named as usize;
            match named {
                NamedColor::Background => palette.bg,
                NamedColor::Foreground
                | NamedColor::BrightForeground
                | NamedColor::DimForeground
                | NamedColor::Cursor => palette.fg,
                _ if index < 16 => Color::Indexed(u8::try_from(index).unwrap_or_default()),
                // The dim variants of the eight base colours: the base colour
                // (the DIM modifier carries the dimming).
                _ => {
                    let dim = index.saturating_sub(NamedColor::DimBlack as usize);
                    Color::Indexed(u8::try_from(dim.min(7)).unwrap_or_default())
                },
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;
    use crate::terminal::Emulator;

    const PALETTE: GridPalette = GridPalette {
        fg: Color::Rgb(200, 200, 200),
        bg: Color::Rgb(10, 10, 10),
        selection: Color::Rgb(50, 50, 120),
    };

    fn render(bytes: &[u8], columns: u16, rows: u16, cursor: bool) -> (Buffer, Vec<LinkRun>) {
        let mut term = Emulator::new_term(columns, rows, VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, bytes);
        let area = Rect::new(0, 0, columns, rows);
        let mut buf = Buffer::empty(area);
        let links = paint_grid(&mut buf, area, &term, &PALETTE, cursor);
        (buf, links)
    }

    fn symbol(buf: &Buffer, x: u16, y: u16) -> &str {
        buf.cell((x, y)).map_or("", |cell| cell.symbol())
    }

    #[test]
    fn text_lands_in_its_cells_on_the_theme_colours() {
        let (buf, _) = render(b"hi\r\nyo", 5, 2, false);
        assert_eq!(symbol(&buf, 0, 0), "h");
        assert_eq!(symbol(&buf, 1, 1), "o");
        let cell = buf.cell((0, 0)).cloned().unwrap_or_default();
        assert_eq!(cell.fg, PALETTE.fg);
        assert_eq!(cell.bg, PALETTE.bg);
    }

    #[test]
    fn sgr_colours_and_attributes_are_kept() {
        // Red (indexed 1), bold; then 24-bit blue on bright green, inverse.
        let (buf, _) = render(b"\x1b[1;31mR\x1b[0m\x1b[38;2;0;0;255;102;7mX", 4, 1, false);
        let red = buf.cell((0, 0)).cloned().unwrap_or_default();
        assert_eq!(
            red.fg,
            Color::Indexed(1),
            "ANSI colours use the host palette"
        );
        assert!(red.modifier.contains(Modifier::BOLD));
        let inverse = buf.cell((1, 0)).cloned().unwrap_or_default();
        assert_eq!(inverse.fg, Color::Indexed(10), "inverse swaps fg and bg");
        assert_eq!(inverse.bg, Color::Rgb(0, 0, 255));
    }

    #[test]
    fn the_cursor_is_shown_only_when_focused() {
        let (focused, _) = render(b"ab", 4, 1, true);
        let cell = focused.cell((2, 0)).cloned().unwrap_or_default();
        assert!(cell.modifier.contains(Modifier::REVERSED));
        let (blurred, _) = render(b"ab", 4, 1, false);
        let cell = blurred.cell((2, 0)).cloned().unwrap_or_default();
        assert!(!cell.modifier.contains(Modifier::REVERSED));
        let (hidden, _) = render(b"ab\x1b[?25l", 4, 1, true);
        let cell = hidden.cell((2, 0)).cloned().unwrap_or_default();
        assert!(
            !cell.modifier.contains(Modifier::REVERSED),
            "a program that hides the cursor hides it"
        );
    }

    #[test]
    fn safe_hyperlinks_are_underlined_and_collected_as_runs() {
        let (buf, links) = render(
            b"\x1b]8;;https://example.com\x1b\\abc\x1b]8;;\x1b\\ \x1b]8;;javascript:x\x1b\\j\x1b]8;;\x1b\\",
            8,
            1,
            false,
        );
        assert_eq!(links, vec![(0, 0, 3, "https://example.com".to_owned())]);
        let linked = buf.cell((1, 0)).cloned().unwrap_or_default();
        assert!(linked.modifier.contains(Modifier::UNDERLINED));
        let unsafe_link = buf.cell((4, 0)).cloned().unwrap_or_default();
        assert!(
            !unsafe_link.modifier.contains(Modifier::UNDERLINED),
            "a scheme outside the allowlist is not offered as a link"
        );
    }

    #[test]
    fn a_selection_paints_the_theme_highlight() {
        let style = cell_style(
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Named(NamedColor::Background),
            Flags::empty(),
            &Colors::default(),
            &PALETTE,
            true,
        );
        assert_eq!(style.bg, Some(PALETTE.selection));
    }

    #[test]
    fn redefined_and_dim_colours_resolve() {
        let mut colors = Colors::default();
        colors[NamedColor::Red] = Some(Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(
            resolve(AnsiColor::Named(NamedColor::Red), &colors, &PALETTE),
            Color::Rgb(1, 2, 3),
            "an OSC 4 redefinition wins"
        );
        assert_eq!(
            resolve(
                AnsiColor::Named(NamedColor::DimGreen),
                &Colors::default(),
                &PALETTE
            ),
            Color::Indexed(2)
        );
        assert_eq!(
            resolve(AnsiColor::Indexed(200), &Colors::default(), &PALETTE),
            Color::Indexed(200)
        );
    }

    #[test]
    fn history_scrolls_into_view() {
        let mut term = Emulator::new_term(6, 2, VoidListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, b"one\r\ntwo\r\nthree");
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(1));
        let area = Rect::new(0, 0, 6, 2);
        let mut buf = Buffer::empty(area);
        paint_grid(&mut buf, area, &term, &PALETTE, true);
        assert_eq!(symbol(&buf, 0, 0), "o", "the top row is now history");
        assert_eq!(symbol(&buf, 0, 1), "t");
    }
}
