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
