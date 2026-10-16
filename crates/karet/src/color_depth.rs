//! Colour depth: whether the terminal shows 24-bit colour, and the fallback
//! that keeps the theme legible when it does not.
//!
//! Themes are authored in RGB and ratatui writes [`Color::Rgb`] as a 24-bit
//! SGR sequence. A terminal that only knows the xterm 256-colour palette either
//! ignores that sequence or misreads it, so on such a terminal every frame is
//! passed through [`fit_buffer`] once it is drawn, which replaces each RGB colour
//! with its nearest palette entry. Detection lives in
//! [`crate::term_caps::probe_color_depth`], next to the other environment
//! heuristics.

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// The kitty unicode-placeholder character. A cell that starts with it carries
/// an image id in its foreground and a placement id in its underline colour,
/// encoded as RGB, which quantising would corrupt.
const KITTY_PLACEHOLDER: char = '\u{10EEEE}';

/// How many colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorDepth {
    /// 24-bit RGB: theme colours are written as they are.
    TrueColor,
    /// The xterm 256-colour palette: RGB colours are quantised to it.
    Indexed256,
}

impl ColorDepth {
    /// A short human-readable name, as `--doctor` prints it.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::TrueColor => "truecolor (24-bit)",
            Self::Indexed256 => "256 colours",
        }
    }
}

/// Make every colour in `buffer` one the terminal can show at `depth`.
///
/// A no-op at [`ColorDepth::TrueColor`]. At [`ColorDepth::Indexed256`] each
/// [`Color::Rgb`] foreground, background, and underline colour becomes its
/// nearest [`Color::Indexed`] entry; every other colour is left alone, since
/// named, indexed, and reset colours are already palette colours. The id
/// colours of kitty placeholder cells are kept exact.
pub(crate) fn fit_buffer(buffer: &mut Buffer, depth: ColorDepth) {
    if depth == ColorDepth::TrueColor {
        return;
    }
    for cell in &mut buffer.content {
        cell.bg = quantize(cell.bg);
        if cell.symbol().starts_with(KITTY_PLACEHOLDER) {
            continue;
        }
        cell.fg = quantize(cell.fg);
        cell.underline_color = quantize(cell.underline_color);
    }
}

/// `color` as a 256-colour palette entry: an RGB colour becomes its nearest
/// entry, anything else is returned unchanged.
#[must_use]
pub(crate) fn quantize(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Indexed(nearest_256(r, g, b)),
        other => other,
    }
}

/// The xterm palette index nearest to `(r, g, b)`.
///
/// Only the 6×6×6 cube (16–231) and the grey ramp (232–255) are candidates:
/// the 16 system colours are whatever the user's terminal theme makes them, so
/// no RGB value can be relied on to land near one.
#[must_use]
pub(crate) fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    /// The cube level index nearest to one channel value. The levels are
    /// 0, 95, 135, 175, 215, 255, so the midpoints are 48 and 115, then every 40.
    fn cube_level(value: u8) -> u8 {
        match value {
            0..48 => 0,
            48..115 => 1,
            _ => (value - 35) / 40,
        }
    }

    let (cr, cg, cb) = (cube_level(r), cube_level(g), cube_level(b));
    let cube_index = 16 + 36 * cr + 6 * cg + cb;

    let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    // The ramp runs 8, 18, …, 238; the step nearest `average` is
    // (average - 3) / 10, clamped to the ramp's ends.
    let grey_step = (average.saturating_sub(3) / 10).min(23) as u8;
    let grey_index = 232 + grey_step;

    let target = (r, g, b);
    if distance(xterm_rgb(grey_index), target) < distance(xterm_rgb(cube_index), target) {
        grey_index
    } else {
        cube_index
    }
}

/// Squared euclidean distance between two RGB colours.
fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let channel = |x: u8, y: u8| u32::from(x.abs_diff(y)).pow(2);
    channel(a.0, b.0) + channel(a.1, b.1) + channel(a.2, b.2)
}

/// The xterm 256-colour palette: 16 system colours, a 6×6×6 RGB cube, 24 greys.
#[must_use]
pub(crate) fn xterm_rgb(index: u8) -> (u8, u8, u8) {
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

    use super::*;

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

    #[test]
    fn every_cube_and_ramp_colour_maps_back_to_itself() {
        for index in 16..=255u8 {
            let (r, g, b) = xterm_rgb(index);
            let nearest = nearest_256(r, g, b);
            // Cube 16 and 231 are black and white; the ramp has neither, so
            // every exact palette colour must come back as its own index.
            assert_eq!(nearest, index, "#{r:02x}{g:02x}{b:02x}");
        }
    }

    #[test]
    fn the_nearest_entry_is_never_beaten_by_another_candidate() {
        // Exhaustive over a coarse lattice of the RGB space (every 5th value
        // per channel, edges included): no cube or ramp entry is closer than
        // the one chosen.
        let values: Vec<u8> = (0..=255u8).step_by(5).collect();
        for &r in &values {
            for &g in &values {
                for &b in &values {
                    let chosen = distance(xterm_rgb(nearest_256(r, g, b)), (r, g, b));
                    let best = (16..=255u8)
                        .map(|i| distance(xterm_rgb(i), (r, g, b)))
                        .min()
                        .unwrap_or(u32::MAX);
                    assert_eq!(chosen, best, "#{r:02x}{g:02x}{b:02x}");
                }
            }
        }
    }

    #[test]
    fn near_greys_prefer_the_ramp_and_saturated_colours_the_cube() {
        // A typical dark-theme background is a near-grey the ramp matches
        // more closely than the cube's coarse steps.
        assert_eq!(nearest_256(0x28, 0x2c, 0x34), 236);
        assert_eq!(nearest_256(0xe0, 0x6c, 0x75), 168);
        assert_eq!(nearest_256(255, 0, 0), 196);
    }

    #[test]
    fn quantize_only_touches_rgb() {
        assert_eq!(quantize(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        for color in [
            Color::Reset,
            Color::Red,
            Color::Indexed(42),
            Color::LightBlue,
        ] {
            assert_eq!(quantize(color), color);
        }
    }

    #[test]
    fn truecolor_leaves_the_buffer_untouched() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer[(0, 0)]
            .set_fg(Color::Rgb(1, 2, 3))
            .set_bg(Color::Rgb(4, 5, 6));
        let before = buffer.clone();
        fit_buffer(&mut buffer, ColorDepth::TrueColor);
        assert_eq!(buffer, before);
    }

    #[test]
    fn indexed_256_quantises_fg_bg_and_underline() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        let cell = &mut buffer[(0, 0)];
        cell.set_symbol("x")
            .set_fg(Color::Rgb(255, 0, 0))
            .set_bg(Color::Rgb(0, 0, 255));
        cell.underline_color = Color::Rgb(0, 255, 0);
        buffer[(1, 0)].set_fg(Color::Yellow);
        fit_buffer(&mut buffer, ColorDepth::Indexed256);
        let cell = &buffer[(0, 0)];
        assert_eq!(cell.fg, Color::Indexed(196));
        assert_eq!(cell.bg, Color::Indexed(21));
        assert_eq!(cell.underline_color, Color::Indexed(46));
        assert_eq!(cell.symbol(), "x", "the glyph is untouched");
        assert_eq!(buffer[(1, 0)].fg, Color::Yellow, "named colours stay");
    }

    #[test]
    fn kitty_placeholder_ids_survive_quantisation() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        let cell = &mut buffer[(0, 0)];
        cell.set_symbol("\u{10EEEE}\u{0305}\u{0305}")
            .set_fg(Color::Rgb(0, 0, 7))
            .set_bg(Color::Rgb(0x28, 0x2c, 0x34));
        cell.underline_color = Color::Rgb(0, 0, 1);
        fit_buffer(&mut buffer, ColorDepth::Indexed256);
        let cell = &buffer[(0, 0)];
        assert_eq!(cell.fg, Color::Rgb(0, 0, 7), "image id is exact");
        assert_eq!(cell.underline_color, Color::Rgb(0, 0, 1), "placement id");
        assert_eq!(cell.bg, Color::Indexed(236), "the background still fits");
    }
}
