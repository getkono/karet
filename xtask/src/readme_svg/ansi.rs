//! Read the truecolor ANSI grid `karet --capture` writes into a cell matrix.
//!
//! This understands exactly the subset the capture emits — a self-contained
//! `ESC[0;…;38;2;r;g;b;48;2;r;g;bm` sequence per style run, one line per row — so a
//! drift between the two shows up as a parse failure here rather than as silently
//! wrong artwork.

/// A 24-bit colour.
pub(crate) type Rgb = (u8, u8, u8);

/// The appearance shared by a run of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Style {
    /// Foreground colour.
    pub(crate) fg: Rgb,
    /// Background colour.
    pub(crate) bg: Rgb,
    /// Render bold.
    pub(crate) bold: bool,
    /// Render italic.
    pub(crate) italic: bool,
    /// Render underlined.
    pub(crate) underlined: bool,
    /// Render struck through.
    pub(crate) crossed_out: bool,
}

/// One grid cell: the text it shows and how it is styled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cell {
    /// The cell's glyph, plus any combining marks that follow it.
    pub(crate) text: String,
    /// How the cell is painted.
    pub(crate) style: Style,
}

/// A parsed capture: rows of cells, padded to a common width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grid {
    /// One entry per row, each exactly [`Grid::cols`] cells wide.
    pub(crate) rows: Vec<Vec<Cell>>,
    /// Width of every row, in columns.
    pub(crate) cols: usize,
}

/// True for code points that render inside the previous cell rather than in one of
/// their own: combining marks, zero-width joiners/spaces, and variation selectors.
///
/// Attaching these to the preceding cell keeps a row's column count equal to its
/// visible width, which is what the SVG's column positions assume.
fn is_combining(ch: char) -> bool {
    matches!(ch,
        '\u{0300}'..='\u{036f}'
        | '\u{200b}'..='\u{200f}'
        | '\u{20d0}'..='\u{20ff}'
        | '\u{fe00}'..='\u{fe0f}'
    )
}

/// Parse one `ESC[…m` parameter list into `style`.
///
/// Parameters outside the captured subset are ignored rather than rejected, so a
/// future capture can add attributes without breaking older readers; a malformed
/// truecolor triple is an error, because it would silently mis-paint the artwork.
fn apply_sgr(params: &str, style: &mut Style) -> Result<(), String> {
    let fields: Vec<&str> = params.split(';').collect();
    let mut index = 0;
    while index < fields.len() {
        let field = fields[index];
        // An empty parameter means the default, which for SGR is a full reset.
        let code = if field.is_empty() {
            0
        } else {
            field
                .parse::<u16>()
                .map_err(|_| format!("unreadable SGR parameter {field:?}"))?
        };
        match code {
            0 => *style = Style::default(),
            1 => style.bold = true,
            3 => style.italic = true,
            4 => style.underlined = true,
            9 => style.crossed_out = true,
            38 | 48 => {
                let triple = fields
                    .get(index + 1..index + 5)
                    .ok_or_else(|| format!("truncated truecolor sequence in {params:?}"))?;
                if triple[0] != "2" {
                    return Err(format!("expected a truecolor selector in {params:?}"));
                }
                let channel = |value: &str| -> Result<u8, String> {
                    value
                        .parse::<u8>()
                        .map_err(|_| format!("unreadable colour channel {value:?}"))
                };
                let rgb = (
                    channel(triple[1])?,
                    channel(triple[2])?,
                    channel(triple[3])?,
                );
                if code == 38 {
                    style.fg = rgb;
                } else {
                    style.bg = rgb;
                }
                index += 4;
            },
            _ => {},
        }
        index += 1;
    }
    Ok(())
}

/// Parse a captured ANSI grid.
///
/// Rows are split on newlines; a trailing newline does not create an empty row.
/// Short rows are padded with blank cells carrying the row's last background, so
/// every row ends up [`Grid::cols`] wide and the artwork has no ragged edge.
pub(crate) fn parse(input: &str) -> Result<Grid, String> {
    // Drop only the newline that terminates the last row, so a genuinely blank row
    // in the middle of the capture still becomes a row.
    let body = input.strip_suffix('\n').unwrap_or(input);
    if body.is_empty() {
        return Ok(Grid {
            rows: Vec::new(),
            cols: 0,
        });
    }

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for line in body.split('\n') {
        let mut style = Style::default();
        let mut cells: Vec<Cell> = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                if chars.next() != Some('[') {
                    return Err("escape sequence is not a CSI".to_owned());
                }
                let mut params = String::new();
                loop {
                    let Some(next) = chars.next() else {
                        return Err("unterminated escape sequence".to_owned());
                    };
                    if next == 'm' {
                        break;
                    }
                    params.push(next);
                }
                apply_sgr(&params, &mut style)?;
                continue;
            }
            if is_combining(ch)
                && let Some(previous) = cells.last_mut()
            {
                previous.text.push(ch);
                continue;
            }
            cells.push(Cell {
                text: ch.to_string(),
                style,
            });
        }
        rows.push(cells);
    }

    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        // Pad with the row's trailing background so the window has a straight edge.
        let style = row.last().map(|cell| cell.style).unwrap_or_default();
        while row.len() < cols {
            row.push(Cell {
                text: " ".to_owned(),
                style,
            });
        }
    }
    Ok(Grid { rows, cols })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact sequence shape `karet --capture` writes.
    fn sgr(fg: Rgb, bg: Rgb) -> String {
        format!(
            "\x1b[0;38;2;{};{};{};48;2;{};{};{}m",
            fg.0, fg.1, fg.2, bg.0, bg.1, bg.2
        )
    }

    #[test]
    fn parses_a_single_styled_row() -> Result<(), String> {
        let input = format!("{}ab\x1b[0m\n", sgr((1, 2, 3), (4, 5, 6)));
        let grid = parse(&input)?;
        assert_eq!(grid.cols, 2);
        assert_eq!(grid.rows.len(), 1);
        assert_eq!(grid.rows[0][0].text, "a");
        assert_eq!(grid.rows[0][0].style.fg, (1, 2, 3));
        assert_eq!(grid.rows[0][1].style.bg, (4, 5, 6));
        Ok(())
    }

    #[test]
    fn a_reset_clears_every_attribute() -> Result<(), String> {
        let input = "\x1b[0;1;3;4;9;38;2;1;1;1;48;2;2;2;2ma\x1b[0mb\n";
        let grid = parse(input)?;
        let first = &grid.rows[0][0].style;
        assert!(first.bold && first.italic && first.underlined && first.crossed_out);
        let second = &grid.rows[0][1].style;
        assert_eq!(*second, Style::default());
        Ok(())
    }

    #[test]
    fn rows_are_padded_to_a_common_width() -> Result<(), String> {
        let grid = parse("abc\nx\n")?;
        assert_eq!(grid.cols, 3);
        assert_eq!(grid.rows[1].len(), 3);
        assert_eq!(grid.rows[1][2].text, " ");
        Ok(())
    }

    #[test]
    fn a_trailing_newline_does_not_add_a_row() -> Result<(), String> {
        assert_eq!(parse("a\nb\n")?.rows.len(), 2);
        assert_eq!(parse("a\nb")?.rows.len(), 2);
        Ok(())
    }

    #[test]
    fn combining_marks_join_the_previous_cell() -> Result<(), String> {
        // "e" + combining acute stays one column, so later columns keep their x.
        let grid = parse("e\u{0301}x\n")?;
        assert_eq!(grid.cols, 2);
        assert_eq!(grid.rows[0][0].text, "e\u{0301}");
        assert_eq!(grid.rows[0][1].text, "x");
        Ok(())
    }

    #[test]
    fn unknown_parameters_are_ignored() -> Result<(), String> {
        // A future capture may add attributes; an older reader must not choke.
        let grid = parse("\x1b[0;53;38;2;1;2;3ma\n")?;
        assert_eq!(grid.rows[0][0].style.fg, (1, 2, 3));
        Ok(())
    }

    #[test]
    fn malformed_input_is_rejected() {
        // Truncated truecolor triple.
        assert!(parse("\x1b[38;2;1;2m").is_err());
        // A palette selector where truecolor is required.
        assert!(parse("\x1b[38;5;1ma\n").is_err());
        // Unterminated sequence.
        assert!(parse("\x1b[0;38;2;1;2;3").is_err());
        // Not a CSI.
        assert!(parse("\x1bXa").is_err());
        // Out-of-range channel.
        assert!(parse("\x1b[38;2;300;2;3ma\n").is_err());
    }

    #[test]
    fn an_empty_capture_yields_an_empty_grid() -> Result<(), String> {
        let grid = parse("")?;
        assert_eq!(grid.cols, 0);
        assert!(grid.rows.is_empty());
        Ok(())
    }
}
