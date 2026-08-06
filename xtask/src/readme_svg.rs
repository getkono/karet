//! Turn a `karet --capture` ANSI grid into the README hero SVG.
//!
//! The artwork is a real frame: every colour, glyph, and column here comes from the
//! app's own render, so the hero cannot drift from the product without the drift
//! being visible. Nothing in this module reads a clock, a font, or the host, so the
//! same capture always produces byte-identical SVG.
//!
//! Columns are pinned rather than flowed: each text run declares `textLength` with
//! `lengthAdjust="spacingAndGlyphs"`, so the grid stays aligned in a viewer whose
//! monospace font has different metrics from the one that produced the capture.

mod ansi;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use ansi::Cell;
use ansi::Grid;
use ansi::Rgb;
use ansi::Style;

/// Width of one terminal column, in SVG user units.
const ADVANCE: usize = 9;
/// Height of one terminal row, in SVG user units.
const LINE_HEIGHT: usize = 20;
/// Glyph size; slightly under [`LINE_HEIGHT`] so rows do not collide.
const FONT_SIZE: usize = 15;
/// Baseline offset within a row box.
const BASELINE: usize = 15;
/// Padding between the window edge and the grid.
const PADDING: usize = 18;
/// Height of the window's title bar.
const TITLE_BAR: usize = 34;
/// Margin between the window and the SVG edge, leaving room for the drop shadow.
const MARGIN: usize = 20;
/// Corner radius of the window.
const RADIUS: usize = 10;

/// The window chrome's colours, chosen to frame a dark terminal capture.
const CHROME_BAR: &str = "#1b1e2b";
/// Hairline around the window.
const CHROME_EDGE: &str = "#2f3347";
/// The page behind the window.
const PAGE: &str = "#0b0d16";

/// Escape the five characters that cannot appear literally in SVG text content.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Format a colour as a lowercase `#rrggbb` literal.
fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

/// The background colour covering the most cells, used as the window fill so the
/// common case needs no `<rect>` at all.
///
/// Ties break toward the numerically smallest colour (a `BTreeMap` walk), so the
/// choice does not depend on iteration order.
fn dominant_background(grid: &Grid) -> Rgb {
    let mut counts: BTreeMap<Rgb, usize> = BTreeMap::new();
    for row in &grid.rows {
        for cell in row {
            *counts.entry(cell.style.bg).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|&(colour, count)| (count, std::cmp::Reverse(colour)))
        .map_or((0, 0, 0), |(colour, _)| colour)
}

/// Split a row into maximal runs of identical style, as `(start_column, cells)`.
fn style_runs(row: &[Cell]) -> Vec<(usize, Vec<&Cell>)> {
    let mut runs: Vec<(usize, Vec<&Cell>)> = Vec::new();
    for (column, cell) in row.iter().enumerate() {
        match runs.last_mut() {
            Some((start, cells))
                if cells.last().is_some_and(|last| last.style == cell.style)
                    && *start + cells.len() == column =>
            {
                cells.push(cell);
            },
            _ => runs.push((column, vec![cell])),
        }
    }
    runs
}

/// Emit the background rectangles for one row, skipping runs that already match the
/// window fill.
fn write_backgrounds(out: &mut String, row: &[Cell], y: usize, fill: Rgb) -> std::fmt::Result {
    for (column, cells) in style_runs(row) {
        let style = match cells.first() {
            Some(cell) => cell.style,
            None => continue,
        };
        if style.bg == fill {
            continue;
        }
        write!(
            out,
            "\n    <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{LINE_HEIGHT}\" fill=\"{}\"/>",
            column * ADVANCE,
            y,
            cells.len() * ADVANCE,
            hex(style.bg)
        )?;
    }
    Ok(())
}

/// The SVG attributes a style contributes beyond its fill.
fn emphasis_attributes(style: Style) -> String {
    let mut attributes = String::new();
    if style.bold {
        attributes.push_str(" font-weight=\"700\"");
    }
    if style.italic {
        attributes.push_str(" font-style=\"italic\"");
    }
    match (style.underlined, style.crossed_out) {
        (true, true) => attributes.push_str(" text-decoration=\"underline line-through\""),
        (true, false) => attributes.push_str(" text-decoration=\"underline\""),
        (false, true) => attributes.push_str(" text-decoration=\"line-through\""),
        (false, false) => {},
    }
    attributes
}

/// Emit the text runs for one row.
///
/// Leading and trailing blanks are trimmed off each run and the run's `x` and
/// `textLength` adjusted to match, so whitespace costs nothing but every glyph keeps
/// its exact column.
fn write_text(out: &mut String, row: &[Cell], y: usize) -> std::fmt::Result {
    for (column, cells) in style_runs(row) {
        let Some(first) = cells.first() else {
            continue;
        };
        let style = first.style;
        let visible_cell = |cell: &&Cell| !cell.text.trim().is_empty();
        let Some(start) = cells.iter().position(visible_cell) else {
            continue;
        };
        let Some(end) = cells.iter().rposition(visible_cell) else {
            continue;
        };
        let visible = &cells[start..=end];
        let text: String = visible.iter().map(|cell| cell.text.as_str()).collect();
        write!(
            out,
            "\n    <text x=\"{}\" y=\"{}\" fill=\"{}\" textLength=\"{}\" \
             lengthAdjust=\"spacingAndGlyphs\"{}>{}</text>",
            (column + start) * ADVANCE,
            y + BASELINE,
            hex(style.fg),
            visible.len() * ADVANCE,
            emphasis_attributes(style),
            escape(&text)
        )?;
    }
    Ok(())
}

/// Render a parsed capture as a standalone, self-describing SVG document.
///
/// `title` and `description` become the `<title>`/`<desc>` the `aria-labelledby`
/// points at, so the artwork carries its own accessible name in the README.
pub(crate) fn render(grid: &Grid, title: &str, description: &str) -> Result<String, String> {
    if grid.cols == 0 || grid.rows.is_empty() {
        return Err("the capture is empty — nothing to render".to_owned());
    }
    let fill = dominant_background(grid);
    let grid_w = grid.cols * ADVANCE;
    let grid_h = grid.rows.len() * LINE_HEIGHT;
    let window_w = grid_w + PADDING * 2;
    let window_h = grid_h + PADDING * 2 + TITLE_BAR;
    let width = window_w + MARGIN * 2;
    let height = window_h + MARGIN * 2;

    let mut out = String::new();
    let write = |out: &mut String, args: std::fmt::Arguments| -> Result<(), String> {
        out.write_fmt(args).map_err(|e| e.to_string())
    };

    write(
        &mut out,
        format_args!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\" role=\"img\" aria-labelledby=\"title description\">\n  \
         <title id=\"title\">{}</title>\n  <desc id=\"description\">{}</desc>\n  <defs>\n    \
         <filter id=\"shadow\" x=\"-10%\" y=\"-10%\" width=\"120%\" height=\"130%\">\n      \
         <feDropShadow dx=\"0\" dy=\"8\" stdDeviation=\"10\" flood-color=\"#01030a\" \
         flood-opacity=\".5\"/>\n    </filter>\n    <clipPath id=\"grid-clip\">\n      \
         <rect x=\"0\" y=\"0\" width=\"{grid_w}\" height=\"{grid_h}\"/>\n    </clipPath>\n  \
         </defs>\n  <rect width=\"{width}\" height=\"{height}\" fill=\"{PAGE}\"/>\n",
            escape(title),
            escape(description),
        ),
    )?;

    // Window chrome: a rounded shell, a title bar, and the three controls.
    write(
        &mut out,
        format_args!(
            "  <g filter=\"url(#shadow)\">\n    <rect x=\"{MARGIN}\" y=\"{MARGIN}\" \
         width=\"{window_w}\" height=\"{window_h}\" rx=\"{RADIUS}\" fill=\"{}\" \
         stroke=\"{CHROME_EDGE}\"/>\n  </g>\n  <path d=\"M{MARGIN} {} v-{} a{RADIUS} {RADIUS} 0 0 1 \
         {RADIUS} -{RADIUS} h{} a{RADIUS} {RADIUS} 0 0 1 {RADIUS} {RADIUS} v{} z\" \
         fill=\"{CHROME_BAR}\"/>\n",
            hex(fill),
            MARGIN + TITLE_BAR,
            TITLE_BAR - RADIUS,
            window_w - RADIUS * 2,
            TITLE_BAR - RADIUS,
        ),
    )?;
    for (index, colour) in ["#ff5f57", "#febc2e", "#28c840"].iter().enumerate() {
        write(
            &mut out,
            format_args!(
                "  <circle cx=\"{}\" cy=\"{}\" r=\"6\" fill=\"{colour}\"/>\n",
                MARGIN + 20 + index * 20,
                MARGIN + TITLE_BAR / 2,
            ),
        )?;
    }

    // The captured grid itself.
    write(
        &mut out,
        format_args!(
            "  <g clip-path=\"url(#grid-clip)\" transform=\"translate({} {})\" \
         font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, &quot;DejaVu Sans Mono&quot;, \
         monospace\" font-size=\"{FONT_SIZE}\" xml:space=\"preserve\">",
            MARGIN + PADDING,
            MARGIN + TITLE_BAR + PADDING,
        ),
    )?;
    for (index, row) in grid.rows.iter().enumerate() {
        let y = index * LINE_HEIGHT;
        write_backgrounds(&mut out, row, y, fill).map_err(|e| e.to_string())?;
        write_text(&mut out, row, y).map_err(|e| e.to_string())?;
    }
    out.push_str("\n  </g>\n</svg>\n");
    Ok(out)
}

/// Parse a capture and render it, in one step.
pub(crate) fn from_capture(
    capture: &str,
    title: &str,
    description: &str,
) -> Result<String, String> {
    render(&ansi::parse(capture)?, title, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-row capture in the exact shape `karet --capture` emits.
    const CAPTURE: &str = concat!(
        "\x1b[0;38;2;192;202;245;48;2;26;27;38mfn main\x1b[0m\n",
        "\x1b[0;1;38;2;158;206;106;48;2;26;27;38m  ok   \x1b[0m\n",
    );

    fn hero() -> Result<String, String> {
        from_capture(CAPTURE, "karet", "A karet window.")
    }

    #[test]
    fn renders_a_standalone_accessible_document() -> Result<(), String> {
        let svg = hero()?;
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("role=\"img\""));
        assert!(svg.contains("aria-labelledby=\"title description\""));
        assert!(svg.contains("<title id=\"title\">karet</title>"));
        assert!(svg.contains("<desc id=\"description\">A karet window.</desc>"));
        assert!(svg.ends_with("</svg>\n"));
        Ok(())
    }

    #[test]
    fn the_capture_drives_the_geometry() -> Result<(), String> {
        let svg = hero()?;
        // 7 columns x 2 rows, plus padding, title bar, and margins.
        let width = 7 * ADVANCE + PADDING * 2 + MARGIN * 2;
        let height = 2 * LINE_HEIGHT + PADDING * 2 + TITLE_BAR + MARGIN * 2;
        assert!(
            svg.contains(&format!("width=\"{width}\" height=\"{height}\"")),
            "geometry follows the grid: {svg}"
        );
        Ok(())
    }

    #[test]
    fn the_grid_clip_is_in_the_transformed_coordinate_system() -> Result<(), String> {
        // The clip lives on the same element as the `translate`, so its rectangle is
        // in grid-local space. Giving it page coordinates double-offsets the clip and
        // silently shears the top and left off the artwork.
        let svg = hero()?;
        assert!(
            svg.contains(&format!(
                "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\"/>",
                7 * ADVANCE,
                2 * LINE_HEIGHT
            )),
            "got {svg}"
        );
        Ok(())
    }

    #[test]
    fn captured_colors_reach_the_output() -> Result<(), String> {
        let svg = hero()?;
        // The Tokyo-Night foreground and the green of the second row.
        assert!(svg.contains("fill=\"#c0caf5\""));
        assert!(svg.contains("fill=\"#9ece6a\""));
        // The shared background became the window fill rather than per-cell rects:
        // it appears once, on the window shell, and never as a background run.
        assert_eq!(svg.matches("fill=\"#1a1b26\"").count(), 1);
        assert!(!svg.contains(&format!("height=\"{LINE_HEIGHT}\" fill=\"#1a1b26\"")));
        Ok(())
    }

    #[test]
    fn text_runs_pin_their_columns() -> Result<(), String> {
        let svg = hero()?;
        // "fn main" is 7 columns wide and starts at column 0.
        assert!(
            svg.contains(&format!(
                "<text x=\"0\" y=\"{BASELINE}\" fill=\"#c0caf5\" textLength=\"{}\" \
                 lengthAdjust=\"spacingAndGlyphs\">fn main</text>",
                7 * ADVANCE
            )),
            "got {svg}"
        );
        Ok(())
    }

    #[test]
    fn blanks_are_trimmed_but_columns_are_kept() -> Result<(), String> {
        let svg = hero()?;
        // Row two is "  ok   ": the run starts at column 2 and is 2 columns wide.
        assert!(
            svg.contains(&format!(
                "<text x=\"{}\" y=\"{}\" fill=\"#9ece6a\" textLength=\"{}\"",
                2 * ADVANCE,
                LINE_HEIGHT + BASELINE,
                2 * ADVANCE
            )),
            "got {svg}"
        );
        Ok(())
    }

    #[test]
    fn emphasis_becomes_svg_attributes() -> Result<(), String> {
        assert!(hero()?.contains("font-weight=\"700\""));
        let styled = from_capture("\x1b[0;3;4;9;38;2;1;2;3;48;2;0;0;0mx\x1b[0m\n", "t", "d")?;
        assert!(styled.contains("font-style=\"italic\""));
        assert!(styled.contains("text-decoration=\"underline line-through\""));
        Ok(())
    }

    #[test]
    fn a_differing_background_becomes_a_rect() -> Result<(), String> {
        // Two rows, one with a distinct background: the minority colour is painted.
        let svg = from_capture(
            "\x1b[0;38;2;1;1;1;48;2;0;0;0maaaa\x1b[0m\n\
             \x1b[0;38;2;1;1;1;48;2;0;0;0maa\x1b[0;38;2;1;1;1;48;2;9;9;9mbb\x1b[0m\n",
            "t",
            "d",
        )?;
        assert!(
            svg.contains(&format!(
                "<rect x=\"{}\" y=\"{LINE_HEIGHT}\" width=\"{}\" height=\"{LINE_HEIGHT}\" \
                 fill=\"#090909\"/>",
                2 * ADVANCE,
                2 * ADVANCE
            )),
            "got {svg}"
        );
        Ok(())
    }

    #[test]
    fn xml_special_characters_are_escaped() -> Result<(), String> {
        let svg = from_capture(
            "\x1b[0;38;2;1;1;1;48;2;0;0;0m&<>\x1b[0m\n",
            "a & b",
            "x < y",
        )?;
        assert!(svg.contains("&amp;&lt;&gt;</text>"));
        assert!(svg.contains("<title id=\"title\">a &amp; b</title>"));
        assert!(svg.contains("<desc id=\"description\">x &lt; y</desc>"));
        Ok(())
    }

    #[test]
    fn rendering_is_deterministic() -> Result<(), String> {
        assert_eq!(hero()?, hero()?);
        Ok(())
    }

    #[test]
    fn an_empty_capture_is_an_error() {
        // Better a loud failure than a blank hero silently overwriting the asset.
        assert!(from_capture("", "t", "d").is_err());
    }

    #[test]
    fn dominant_background_breaks_ties_deterministically() -> Result<(), String> {
        let grid =
            ansi::parse("\x1b[0;38;2;1;1;1;48;2;9;9;9ma\x1b[0;38;2;1;1;1;48;2;0;0;0mb\x1b[0m\n")?;
        // One cell each: the smaller colour wins, on every run.
        assert_eq!(dominant_background(&grid), (0, 0, 0));
        Ok(())
    }
}
