//! Pointer text selection over a scrollable list of rendered rows.
//!
//! The read-only surfaces — diffs, the hex dump, the markdown preview — paint
//! rows they build themselves, so they have no document model to select over the
//! way the editor does. This module is the one blessed selection model for them:
//! a [`RowSelection`] anchored to *absolute row indices and byte offsets*, never
//! to screen cells, so a selection survives scrolling and a repaint.
//!
//! A surface supplies two things: the copyable text of a row, and where that
//! text starts on screen. [`paint_row`] then lays the selection background over
//! the already-rendered cells, and [`RowSelection::text`] reproduces exactly what
//! is highlighted — decorations such as diff gutters are never part of a row's
//! text, so they are neither selectable nor copied.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use unicode_width::UnicodeWidthChar;

use crate::textfield::byte_at_cell;

/// A position in a row-oriented surface.
///
/// Ordering is lexicographic — row first, then offset — which is what makes
/// [`RowSelection::bounds`] a plain comparison.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct RowPos {
    /// The row's absolute index in the surface's document, not its screen row.
    pub row: usize,
    /// The byte offset into that row's copyable text; always a char boundary.
    pub byte: usize,
}

impl RowPos {
    /// The position at byte offset `byte` of row `row`.
    #[must_use]
    pub const fn new(row: usize, byte: usize) -> Self {
        Self { row, byte }
    }
}

/// An anchor/head text selection over rendered rows.
///
/// The anchor stays where the drag began while the head follows the pointer, so
/// a backwards drag is a perfectly ordinary selection — it is normalized only
/// when the covered range is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowSelection {
    anchor: RowPos,
    head: RowPos,
}

impl RowSelection {
    /// An empty selection collapsed at `at`, ready to be dragged out.
    #[must_use]
    pub const fn new(at: RowPos) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    /// Move the head to `to`, leaving the anchor where the drag began.
    pub fn extend_to(&mut self, to: RowPos) {
        self.head = to;
    }

    /// Where the drag began.
    #[must_use]
    pub const fn anchor(&self) -> RowPos {
        self.anchor
    }

    /// Where the pointer is now.
    #[must_use]
    pub const fn head(&self) -> RowPos {
        self.head
    }

    /// Whether the selection covers no text at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selection as an ordered `(start, end)` pair.
    #[must_use]
    pub fn bounds(&self) -> (RowPos, RowPos) {
        if self.head < self.anchor {
            (self.head, self.anchor)
        } else {
            (self.anchor, self.head)
        }
    }

    /// The byte range selected on `row`, whose text is `len` bytes long, or
    /// `None` when the row falls outside the selection or contributes no cells.
    ///
    /// The first row runs from its start offset to the end of the row, interior
    /// rows are covered whole, and the last row stops at its end offset — the
    /// shape the editor already paints for a multi-line selection.
    #[must_use]
    pub fn row_span(&self, row: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.bounds();
        if row < start.row || row > end.row {
            return None;
        }
        let from = if row == start.row {
            start.byte.min(len)
        } else {
            0
        };
        let to = if row == end.row {
            end.byte.min(len)
        } else {
            len
        };
        (from < to).then_some(from..to)
    }

    /// The selected text, given the rows starting at absolute row `first_row`.
    ///
    /// Rows are joined with `\n`. A covered row that contributes no characters
    /// still yields an empty line, so blank rows and a diff's card chrome keep
    /// on the clipboard the visual break they have on screen.
    #[must_use]
    pub fn text<S: AsRef<str>>(&self, first_row: usize, rows: &[S]) -> String {
        let (start, end) = self.bounds();
        let mut out = String::new();
        let mut first = true;
        for (offset, row) in rows.iter().enumerate() {
            let index = first_row.saturating_add(offset);
            if index < start.row || index > end.row {
                continue;
            }
            let text = row.as_ref();
            let from = if index == start.row {
                clamp_boundary(text, start.byte)
            } else {
                0
            };
            let to = if index == end.row {
                clamp_boundary(text, end.byte)
            } else {
                text.len()
            };
            if !first {
                out.push('\n');
            }
            first = false;
            if from < to {
                out.push_str(&text[from..to]);
            }
        }
        out
    }
}

/// The display column at which the character at byte offset `byte` begins.
///
/// The inverse of [`crate::textfield::byte_at_cell`], and it counts widths the
/// same way (a zero-width character still occupies one cell), so the two round
/// trip on every offset that starts a character.
#[must_use]
pub fn display_col_at_byte(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(index, _)| *index < byte)
        .map(|(_, character)| character.width().unwrap_or(0).max(1))
        .sum()
}

/// Where a surface's copyable text sits on screen.
///
/// Rows are painted into [`area`](Self::area); a row's text starts
/// [`content_x`](Self::content_x) columns in, which is how a diff's line-number
/// gutter or the hex dump's offset column stays outside the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowGeometry {
    /// The rect the surface actually painted its rows into.
    pub area: Rect,
    /// Columns from `area.x` at which a row's copyable text begins.
    pub content_x: u16,
    /// Display columns of that text scrolled off the left edge.
    pub hscroll: usize,
}

impl RowGeometry {
    /// Rows painted into `area`, with their text starting `content_x` columns in.
    #[must_use]
    pub const fn new(area: Rect, content_x: u16) -> Self {
        Self {
            area,
            content_x,
            hscroll: 0,
        }
    }

    /// The same geometry, with `hscroll` display columns scrolled off the left.
    #[must_use]
    pub const fn hscroll(mut self, hscroll: usize) -> Self {
        self.hscroll = hscroll;
        self
    }

    /// The byte offset in `text` under screen column `x`.
    ///
    /// A column left of the text — in the gutter — resolves to the first
    /// visible character, and one past the end to `text.len()`, so a drag that
    /// leaves the row sideways still lands on a sensible offset.
    #[must_use]
    pub fn byte_at(&self, text: &str, x: u16) -> usize {
        let base = self.area.x.saturating_add(self.content_x);
        let cell = usize::from(x.saturating_sub(base)).saturating_add(self.hscroll);
        byte_at_cell(text, cell)
    }
}

/// Lay the selection background `bg` over the run of `text` selected on the row
/// painted at screen row `y`.
///
/// Cells outside `geometry.area` are clipped. Call this after the surface's
/// widget has rendered: it repaints backgrounds only, so the row keeps its own
/// foreground colours.
pub fn paint_row(
    buf: &mut Buffer,
    geometry: &RowGeometry,
    y: u16,
    text: &str,
    span: &Range<usize>,
    bg: Color,
) {
    let area = geometry.area;
    if y < area.y || y >= area.bottom() {
        return;
    }
    let from = display_col_at_byte(text, span.start).saturating_sub(geometry.hscroll);
    let to = display_col_at_byte(text, span.end).saturating_sub(geometry.hscroll);
    if to <= from {
        return;
    }
    let base = usize::from(area.x).saturating_add(usize::from(geometry.content_x));
    let first = base.saturating_add(from);
    let last = base.saturating_add(to).min(usize::from(area.right()));
    for x in first..last {
        let Ok(x) = u16::try_from(x) else { break };
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_bg(bg);
        }
    }
}

/// Clamp `byte` into `text` and back onto the nearest lower char boundary.
fn clamp_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(from: (usize, usize), to: (usize, usize)) -> RowSelection {
        let mut selection = RowSelection::new(RowPos::new(from.0, from.1));
        selection.extend_to(RowPos::new(to.0, to.1));
        selection
    }

    #[test]
    fn a_fresh_selection_is_empty_until_it_is_dragged() {
        let mut sel = RowSelection::new(RowPos::new(3, 7));
        assert!(sel.is_empty());
        assert_eq!(sel.anchor(), sel.head());
        sel.extend_to(RowPos::new(3, 9));
        assert!(!sel.is_empty());
        assert_eq!(sel.anchor(), RowPos::new(3, 7));
    }

    #[test]
    fn a_backwards_drag_normalizes_its_bounds_but_keeps_its_anchor() {
        let sel = selection((5, 2), (1, 4));
        assert_eq!(sel.bounds(), (RowPos::new(1, 4), RowPos::new(5, 2)));
        // The anchor still records where the pointer went down.
        assert_eq!(sel.anchor(), RowPos::new(5, 2));
    }

    #[test]
    fn a_multi_row_selection_covers_first_interior_and_last_rows() {
        let sel = selection((1, 3), (3, 2));
        assert_eq!(sel.row_span(0, 10), None);
        assert_eq!(sel.row_span(1, 10), Some(3..10));
        assert_eq!(sel.row_span(2, 10), Some(0..10));
        assert_eq!(sel.row_span(3, 10), Some(0..2));
        assert_eq!(sel.row_span(4, 10), None);
    }

    #[test]
    fn a_single_row_selection_spans_only_its_own_offsets() {
        let sel = selection((2, 1), (2, 5));
        assert_eq!(sel.row_span(2, 10), Some(1..5));
        assert_eq!(sel.row_span(1, 10), None);
        assert_eq!(sel.row_span(3, 10), None);
    }

    #[test]
    fn row_spans_clamp_to_a_shorter_row_and_vanish_when_empty() {
        let sel = selection((0, 4), (2, 9));
        // Row 1 is shorter than the selection's offsets: covered whole.
        assert_eq!(sel.row_span(1, 3), Some(0..3));
        // The first row is entirely before the anchor offset.
        assert_eq!(sel.row_span(0, 4), None);
        // A blank interior row contributes no cells.
        assert_eq!(sel.row_span(1, 0), None);
    }

    #[test]
    fn text_joins_covered_rows_and_keeps_blank_ones_as_empty_lines() {
        let rows = ["first line", "", "third line"];
        let sel = selection((0, 6), (2, 5));
        assert_eq!(sel.text(0, &rows), "line\n\nthird");
    }

    #[test]
    fn text_is_offset_by_the_first_rows_absolute_index() {
        let rows = ["alpha", "bravo", "charlie"];
        let sel = selection((11, 2), (12, 3));
        // `rows[0]` is absolute row 10, so the selection starts on `bravo`.
        assert_eq!(sel.text(10, &rows), "avo\ncha");
    }

    #[test]
    fn text_of_a_single_row_slices_that_row_alone() {
        let rows = ["hello world"];
        assert_eq!(selection((0, 6), (0, 11)).text(0, &rows), "world");
        // A backwards drag yields the same text.
        assert_eq!(selection((0, 11), (0, 6)).text(0, &rows), "world");
    }

    #[test]
    fn text_never_splits_a_multi_byte_character() {
        let rows = ["a界b"];
        // Byte 2 is inside the three-byte `界`; it floors onto the boundary.
        let sel = selection((0, 0), (0, 2));
        assert_eq!(sel.text(0, &rows), "a");
        // Past the end clamps to the whole row rather than panicking.
        assert_eq!(selection((0, 0), (0, 99)).text(0, &rows), "a界b");
    }

    #[test]
    fn display_columns_round_trip_with_the_text_field_hit_test() {
        let text = "a界b";
        assert_eq!(display_col_at_byte(text, 0), 0);
        assert_eq!(display_col_at_byte(text, 1), 1);
        // `界` is two cells wide, so `b` starts at column three.
        assert_eq!(display_col_at_byte(text, 4), 3);
        for byte in [0usize, 1, 4] {
            assert_eq!(byte_at_cell(text, display_col_at_byte(text, byte)), byte);
        }
    }

    #[test]
    fn hit_testing_skips_the_gutter_and_follows_horizontal_scroll() {
        let area = Rect::new(4, 0, 20, 3);
        let geometry = RowGeometry::new(area, 2);
        let text = "hello world";
        // Text starts at screen column 6 (area.x 4 + a two-column gutter).
        assert_eq!(geometry.byte_at(text, 6), 0);
        assert_eq!(geometry.byte_at(text, 12), 6);
        // Anywhere in the gutter resolves to the first visible character...
        assert_eq!(geometry.byte_at(text, 4), 0);
        assert_eq!(geometry.byte_at(text, 0), 0);
        // ...and past the row's end to its length.
        assert_eq!(geometry.byte_at(text, 99), text.len());
        // Scrolling shifts which character a column names.
        assert_eq!(geometry.hscroll(6).byte_at(text, 6), 6);
    }

    #[test]
    fn hit_testing_lands_on_char_boundaries_of_wide_characters() {
        let geometry = RowGeometry::new(Rect::new(0, 0, 20, 1), 0);
        let text = "a界b";
        assert_eq!(geometry.byte_at(text, 0), 0);
        // Both cells of the wide `界` resolve to its single start offset.
        assert_eq!(geometry.byte_at(text, 1), 1);
        assert_eq!(geometry.byte_at(text, 2), 1);
        assert_eq!(geometry.byte_at(text, 3), 4);
    }

    #[test]
    fn painting_covers_exactly_the_selected_cells() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let text = "hello world";
        // Skip a two-column gutter, select `world`.
        let geometry = RowGeometry::new(area, 2);
        paint_row(&mut buf, &geometry, 1, text, &(6..11), Color::Blue);
        let bg = |x: u16| buf.cell((x, 1)).map(|c| c.bg);
        assert_eq!(
            bg(7),
            Some(Color::Reset),
            "the cell before the run is untouched"
        );
        for x in 8..13 {
            assert_eq!(bg(x), Some(Color::Blue), "column {x} should be selected");
        }
        assert_eq!(
            bg(13),
            Some(Color::Reset),
            "the cell after the run is untouched"
        );
    }

    #[test]
    fn painting_clips_to_the_area_and_to_horizontal_scroll() {
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        let text = "0123456789abcdef";
        // Four columns are scrolled off, so bytes 4..12 start at the left edge
        // and the run is cut at the area's right edge.
        paint_row(
            &mut buf,
            &RowGeometry::new(area, 0).hscroll(4),
            0,
            text,
            &(4..12),
            Color::Red,
        );
        for x in 0..8 {
            assert_eq!(buf.cell((x, 0)).map(|c| c.bg), Some(Color::Red));
        }
        // A run entirely scrolled off paints nothing.
        let mut buf = Buffer::empty(area);
        paint_row(
            &mut buf,
            &RowGeometry::new(area, 0).hscroll(9),
            0,
            text,
            &(0..4),
            Color::Red,
        );
        assert!((0..8).all(|x| buf.cell((x, 0)).map(|c| c.bg) == Some(Color::Reset)));
    }

    #[test]
    fn painting_a_row_outside_the_area_is_a_no_op() {
        let area = Rect::new(0, 5, 8, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 10));
        let geometry = RowGeometry::new(area, 0);
        paint_row(&mut buf, &geometry, 0, "text", &(0..4), Color::Red);
        paint_row(&mut buf, &geometry, 9, "text", &(0..4), Color::Red);
        assert!(
            (0..10).all(|y| (0..8).all(|x| buf.cell((x, y)).map(|c| c.bg) == Some(Color::Reset)))
        );
    }
}
