//! OSC 8 hyperlinks, painted cell by cell.
//!
//! A terminal hyperlink is an escape sequence wrapped around the text it links.
//! Ratatui owns the buffer and diffs it cell by cell, so the sequence cannot be
//! emitted once around a run: every cell carries its own self-contained
//! `OSC 8 … ST symbol OSC 8 ;; ST`, and an explicit shared id tells the terminal
//! those cells (and later redraws of them) are one hyperlink rather than a fresh
//! one per opening sequence.
//!
//! Both link surfaces — the Markdown preview and the commit-message box — paint
//! through [`link_row`] so the cell walk, the width accounting, and the id scheme
//! cannot drift between them.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::buffer::CellDiffOption;
use unicode_width::UnicodeWidthStr;

/// Wrap `symbol` in a self-contained OSC 8 hyperlink to `uri`.
pub(super) fn osc8_symbol(uri: &str, symbol: &str) -> String {
    let id = osc8_id(uri);
    format!("\u{1b}]8;id={id};{uri}\u{1b}\\{symbol}\u{1b}]8;;\u{1b}\\")
}

/// Return a stable, terminal-safe identity for every cell carrying `uri`.
///
/// The renderer emits one self-contained OSC 8 sequence per cell so Ratatui can
/// diff and repaint cells independently. An explicit shared ID lets the terminal
/// treat those cells (and later redraws) as one hyperlink instead of allocating a
/// fresh implicit hyperlink for every opening sequence.
pub(super) fn osc8_id(uri: &str) -> String {
    // FNV-1a is sufficient here: this is a compact rendering identity, not a
    // security boundary. The URI remains part of OSC 8 and of terminal-side link
    // identity, so a hash collision cannot substitute one validated target for
    // another.
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = uri.as_bytes().iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    });
    format!("karet-{hash:016x}")
}

/// Make the cells of row `y` from `x` up to (excluding) `end` link to `uri`.
///
/// The walk advances by each cell's own display width so a wide glyph's
/// continuation cell is left alone — rewriting it would make Ratatui emit the
/// escape twice for one glyph. `ForcedWidth` keeps the escape bytes out of
/// Ratatui's width accounting, which would otherwise read the sequence as text.
pub(super) fn link_row(buf: &mut Buffer, y: u16, x: u16, end: u16, uri: &str) {
    let mut x = x;
    while x < end {
        let Some(cell) = buf.cell_mut((x, y)) else {
            break;
        };
        let symbol = cell.symbol().to_string();
        let width = u16::try_from(symbol.width()).unwrap_or(u16::MAX).max(1);
        cell.set_symbol(&osc8_symbol(uri, &symbol));
        if let Some(width) = NonZeroU16::new(width) {
            cell.set_diff_option(CellDiffOption::ForcedWidth(width));
        }
        x = x.saturating_add(width);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::*;

    const URI: &str = "https://example.com";

    fn symbol(buf: &Buffer, x: u16) -> String {
        buf.cell((x, 0))
            .map(|cell| cell.symbol().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn a_link_walks_by_glyph_width_and_leaves_continuation_cells_alone() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        // "a界b": the wide glyph owns cells 1 and 2, so cell 2 is its continuation
        // and must not be rewritten — the escape would then be emitted twice for
        // one glyph.
        for (x, glyph) in [(0, "a"), (1, "界"), (3, "b")] {
            if let Some(cell) = buf.cell_mut((x, 0)) {
                cell.set_symbol(glyph);
            }
        }
        link_row(&mut buf, 0, 0, 4, URI);

        assert_eq!(symbol(&buf, 0), osc8_symbol(URI, "a"));
        assert_eq!(symbol(&buf, 1), osc8_symbol(URI, "界"));
        assert_eq!(symbol(&buf, 2), " ", "the continuation cell is skipped");
        assert_eq!(symbol(&buf, 3), osc8_symbol(URI, "b"));
    }

    #[test]
    fn a_link_stops_at_the_end_column_and_outside_the_buffer() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        for x in 0..3 {
            if let Some(cell) = buf.cell_mut((x, 0)) {
                cell.set_symbol("x");
            }
        }
        link_row(&mut buf, 0, 0, 2, URI);
        assert!(symbol(&buf, 1).contains("\u{1b}]8"));
        assert_eq!(symbol(&buf, 2), "x", "past `end` is untouched");

        // A row outside the buffer is a no-op rather than a panic.
        link_row(&mut buf, 9, 0, 3, URI);
    }

    #[test]
    fn osc8_link_cells_are_self_contained_and_share_an_explicit_id() {
        let uri = "https://example.com";
        let id = osc8_id(uri);
        let first = osc8_symbol(uri, "x");
        let second = osc8_symbol(uri, "y");

        assert_eq!(
            first,
            format!("\u{1b}]8;id={id};{uri}\u{1b}\\x\u{1b}]8;;\u{1b}\\")
        );
        assert!(second.starts_with(&format!("\u{1b}]8;id={id};{uri}\u{1b}\\")));
        assert_ne!(id, osc8_id("https://example.org"));
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        );
    }

    #[test]
    fn osc8_link_bytes_reach_the_crossterm_backend() -> Result<(), Box<dyn std::error::Error>> {
        use std::num::NonZeroU16;

        use ratatui::backend::Backend;
        use ratatui::backend::CrosstermBackend;
        use ratatui::buffer::Cell;
        use ratatui::buffer::CellDiffOption;

        let sequence = osc8_symbol("https://example.com", "x");
        let mut cell = Cell::default();
        cell.set_symbol(&sequence);
        if let Some(width) = NonZeroU16::new(1) {
            cell.set_diff_option(CellDiffOption::ForcedWidth(width));
        }
        let mut output = Vec::new();
        {
            let mut backend = CrosstermBackend::new(&mut output);
            backend.draw(std::iter::once((0, 0, &cell)))?;
            Backend::flush(&mut backend)?;
        }

        assert!(
            output
                .windows(sequence.len())
                .any(|window| window == sequence.as_bytes())
        );
        Ok(())
    }
}
