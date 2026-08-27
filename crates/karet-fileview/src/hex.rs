//! A scrollable hex-dump widget for binary files: `offset | 16 hex bytes | ascii`.

use karet_core::ThemeRole;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;

/// Bytes shown per row.
const ROW_WIDTH: usize = 16;

/// Columns the file-offset prefix occupies before a row's byte columns begin.
pub const OFFSET_WIDTH: u16 = 10;

/// A read-only hex view of a byte slice. Scroll is measured in rows; the
/// application clamps it against [`row_count`](HexView::row_count).
pub struct HexView<'a> {
    bytes: &'a [u8],
    scroll: usize,
    theme: Option<&'a Theme>,
}

impl<'a> HexView<'a> {
    /// View `bytes` from the top.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            scroll: 0,
            theme: None,
        }
    }

    /// Scroll to the given first visible row.
    #[must_use]
    pub fn scroll(mut self, rows: usize) -> Self {
        self.scroll = rows;
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The total number of 16-byte rows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.bytes.len().div_ceil(ROW_WIDTH)
    }
}

impl Widget for HexView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let fallback;
        let theme = match self.theme {
            Some(theme) => theme,
            None => {
                fallback = Theme::dark();
                &fallback
            },
        };
        let offset_style = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
        let byte_style = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
        let ascii_style = Style::default().fg(theme.role(ThemeRole::LineNumberActive).to_ratatui());

        let rows = self.row_count();
        for screen_row in 0..area.height {
            let row = self.scroll + screen_row as usize;
            if row >= rows {
                break;
            }
            let offset = row * ROW_WIDTH;
            let (hex, ascii) = format_row(self.bytes, offset);
            let line = Line::from(vec![
                Span::styled(format!("{offset:08x}  "), offset_style),
                Span::styled(hex, byte_style),
                Span::styled(format!(" |{ascii}|"), ascii_style),
            ]);
            buf.set_line(area.x, area.y + screen_row, &line, area.width);
        }
    }
}

/// The copyable text of hex row `row`: its byte and ASCII columns, exactly as
/// painted, without the leading file-offset column.
///
/// The offset column is chrome — it names where the row sits rather than what it
/// holds — so a selection over the dump skips it, and [`OFFSET_WIDTH`] is where
/// the text this returns begins on screen. `None` past the last row.
#[must_use]
pub fn row_text(bytes: &[u8], row: usize) -> Option<String> {
    if row >= bytes.len().div_ceil(ROW_WIDTH) {
        return None;
    }
    let (hex, ascii) = format_row(bytes, row * ROW_WIDTH);
    Some(format!("{hex} |{ascii}|"))
}

/// Format the 16-byte row at `offset` into `(hex, ascii)` columns, padding a short
/// final row so columns stay aligned.
fn format_row(bytes: &[u8], offset: usize) -> (String, String) {
    let mut hex = String::new();
    let mut ascii = String::new();
    for i in 0..ROW_WIDTH {
        if i == ROW_WIDTH / 2 {
            hex.push(' ');
        }
        match bytes.get(offset + i) {
            Some(&b) => {
                hex.push_str(&format!("{b:02x} "));
                if (0x20..0x7f).contains(&b) {
                    ascii.push(b as char);
                } else {
                    ascii.push('.');
                }
            },
            None => {
                hex.push_str("   ");
                ascii.push(' ');
            },
        }
    }
    (hex, ascii)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_count_rounds_up() {
        assert_eq!(HexView::new(&[]).row_count(), 0);
        assert_eq!(HexView::new(&[0u8; 1]).row_count(), 1);
        assert_eq!(HexView::new(&[0u8; 16]).row_count(), 1);
        assert_eq!(HexView::new(&[0u8; 17]).row_count(), 2);
    }

    #[test]
    fn format_row_renders_hex_and_ascii() {
        let (hex, ascii) = format_row(b"AB", 0);
        assert!(hex.starts_with("41 42 "));
        assert!(ascii.starts_with("AB"));
        // Non-printable bytes render as '.'.
        let (_, ascii) = format_row(&[0x00, 0x41], 0);
        assert!(ascii.starts_with(".A"));
    }

    #[test]
    fn render_draws_offset_and_bytes() {
        let theme = Theme::dark();
        let area = Rect::new(0, 0, 80, 2);
        let mut buf = Buffer::empty(area);
        HexView::new(b"hello").theme(&theme).render(area, &mut buf);
        let rendered: String = buf
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("00000000"));
        assert!(rendered.contains("68 65 6c 6c 6f")); // "hello"
        assert!(rendered.contains("|hello"));
    }
}

#[cfg(test)]
mod row_text_tests {
    use super::*;

    #[test]
    fn row_text_matches_what_the_row_paints_after_its_offset_column() {
        let bytes: Vec<u8> = (0u8..20).collect();
        let text = row_text(&bytes, 0);
        assert!(text.is_some());
        let Some(text) = text else { return };
        // Byte columns, the mid-row gap, then the ASCII column in pipes.
        assert!(text.starts_with("00 01 02 03 04 05 06 07  08 09"));
        assert!(text.ends_with('|'));
        assert!(text.contains(" |"));
    }

    #[test]
    fn a_short_final_row_keeps_its_column_alignment() {
        let bytes: Vec<u8> = (0u8..20).collect();
        let full = row_text(&bytes, 0).unwrap_or_default();
        let short = row_text(&bytes, 1).unwrap_or_default();
        assert_eq!(
            full.len(),
            short.len(),
            "padding keeps every row the same width"
        );
        // Only four bytes remain, so the rest is blank.
        assert!(short.starts_with("10 11 12 13    "));
    }

    #[test]
    fn printable_bytes_show_as_themselves_and_the_rest_as_dots() {
        let text = row_text(b"hi\x00there", 0).unwrap_or_default();
        assert!(text.contains("|hi.there"), "{text}");
    }

    #[test]
    fn there_is_no_text_past_the_last_row() {
        assert_eq!(row_text(&[], 0), None);
        let bytes = [0u8; 17];
        assert!(
            row_text(&bytes, 1).is_some(),
            "17 bytes spill into a second row"
        );
        assert_eq!(row_text(&bytes, 2), None);
    }

    #[test]
    fn the_offset_column_width_matches_what_the_widget_paints() {
        let bytes: Vec<u8> = (0u8..16).collect();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 1));
        HexView::new(&bytes).render(Rect::new(0, 0, 80, 1), &mut buf);
        let painted: String = (0..80)
            .filter_map(|x| buf.cell((x, 0)).map(|cell| cell.symbol().to_owned()))
            .collect();
        let text = row_text(&bytes, 0).unwrap_or_default();
        assert!(
            painted[usize::from(OFFSET_WIDTH)..].starts_with(&text),
            "row text should begin exactly OFFSET_WIDTH columns in: {painted:?}"
        );
    }
}
