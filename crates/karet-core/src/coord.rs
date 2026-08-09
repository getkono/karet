//! Text coordinates in two deliberately-distinct spaces.
//!
//! *Byte* offsets ([`BytePos`], [`Span`]) index into the UTF-8 buffer and are used
//! by the engines for O(1) edits and highlight spans. *Line/column* positions
//! ([`LineCol`], [`Range`]) are snapshot-stable and are what the presentation layer
//! and the client-server seam speak without owning the rope.
//!
//! Conversions between the two require the text buffer and therefore live on
//! `karet_text::TextBuffer`, not here.

use crate::error::CoreError;

/// An absolute byte offset into a UTF-8 text buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BytePos(pub usize);

/// A zero-based line/column position.
///
/// `col` is counted in Unicode scalar values (`char`s) — karet's canonical
/// internal unit. Boundaries that speak other units (LSP's UTF-16 default)
/// translate at the edge via `karet_text::TextBuffer`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LineCol {
    /// Zero-based line index.
    pub line: u32,
    /// Zero-based column index, in Unicode scalar values.
    pub col: u32,
}

impl LineCol {
    /// Create a position at `line` / `col`.
    #[must_use]
    pub const fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

/// A half-open byte span `[start, end)` within a single buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    /// Inclusive start byte.
    pub start: BytePos,
    /// Exclusive end byte.
    pub end: BytePos,
}

impl Span {
    /// Create a span, returning [`CoreError::InvalidSpan`] when `start > end`.
    ///
    /// # Errors
    /// Returns [`CoreError::InvalidSpan`] if `start` is after `end`.
    pub fn new(start: BytePos, end: BytePos) -> Result<Self, CoreError> {
        if start.0 <= end.0 {
            Ok(Self { start, end })
        } else {
            Err(CoreError::InvalidSpan)
        }
    }

    /// The length of the span in bytes.
    #[must_use]
    pub const fn len(self) -> usize {
        self.end.0.saturating_sub(self.start.0)
    }

    /// Whether the span is empty (zero bytes).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start.0 >= self.end.0
    }

    /// Whether byte position `p` falls within the half-open span.
    #[must_use]
    pub const fn contains(self, p: BytePos) -> bool {
        p.0 >= self.start.0 && p.0 < self.end.0
    }
}

/// A half-open line/column range `[start, end)`.
///
/// This is the coordinate used by every neutral model (diagnostics, decorations,
/// symbols, edits) and across the client-server seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Range {
    /// Inclusive start position.
    pub start: LineCol,
    /// Exclusive end position.
    pub end: LineCol,
}

impl Range {
    /// Create a range, returning [`CoreError::InvalidRange`] when `start > end`.
    ///
    /// # Errors
    /// Returns [`CoreError::InvalidRange`] if `start` is ordered after `end`.
    pub fn new(start: LineCol, end: LineCol) -> Result<Self, CoreError> {
        if start <= end {
            Ok(Self { start, end })
        } else {
            Err(CoreError::InvalidRange)
        }
    }

    /// Whether the range is empty (`start == end`).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Whether line/column position `p` falls within the half-open range.
    #[must_use]
    pub fn contains(self, p: LineCol) -> bool {
        p >= self.start && p < self.end
    }
}

/// A precomputed line-start index over one immutable text snapshot, for cheap
/// byte-offset → row / line-column lookups.
///
/// Engines that work over a borrowed `&str` (highlighters, outline extractors,
/// blame narrowing) all need "which row is this byte on?"; this is that one
/// shared implementation. Buffers that *own* mutable text keep using
/// `karet_text::TextBuffer`'s conversions instead.
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Byte offset of the start of each row; `starts[0] == 0`.
    starts: Vec<usize>,
}

impl LineIndex {
    /// Index `text`'s line starts (one `O(n)` scan).
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        );
        Self { starts }
    }

    /// The number of rows in the indexed text (always at least 1).
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The 0-based row containing byte offset `pos` (clamped to the last row).
    #[must_use]
    pub fn row_at(&self, pos: BytePos) -> u32 {
        let row = self.starts.partition_point(|&s| s <= pos.0) - 1;
        u32::try_from(row).unwrap_or(u32::MAX)
    }

    /// The byte offset of the start of `row`, or `None` past the last row.
    #[must_use]
    pub fn start_of(&self, row: u32) -> Option<BytePos> {
        self.starts.get(row as usize).map(|&s| BytePos(s))
    }

    /// The `(row, byte-column)` grid point of byte offset `pos`.
    #[must_use]
    pub fn point_at(&self, pos: BytePos) -> (u32, usize) {
        let row = self.row_at(pos);
        let start = self.starts.get(row as usize).copied().unwrap_or(0);
        (row, pos.0.saturating_sub(start))
    }

    /// The [`LineCol`] (char-counted column) of byte offset `pos` within `text`.
    ///
    /// `text` must be the same snapshot the index was built from.
    #[must_use]
    pub fn line_col(&self, text: &str, pos: BytePos) -> LineCol {
        let (row, byte_col) = self.point_at(pos);
        let start = self.starts.get(row as usize).copied().unwrap_or(0);
        let col = text
            .get(start..start + byte_col)
            .map_or(0, |s| s.chars().count());
        LineCol::new(row, u32::try_from(col).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_validation_and_queries() {
        // Construct directly (fields are public) to avoid unwrap/expect in tests.
        let s = Span {
            start: BytePos(2),
            end: BytePos(5),
        };
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        assert!(s.contains(BytePos(2)));
        assert!(!s.contains(BytePos(5)));
        assert_eq!(Span::new(BytePos(2), BytePos(5)), Ok(s));
        assert_eq!(
            Span::new(BytePos(5), BytePos(2)),
            Err(CoreError::InvalidSpan)
        );
    }

    #[test]
    fn range_ordering_and_contains() {
        let r = Range {
            start: LineCol::new(1, 0),
            end: LineCol::new(3, 4),
        };
        assert!(r.contains(LineCol::new(2, 99)));
        assert!(!r.contains(LineCol::new(3, 4)));
        assert_eq!(Range::new(LineCol::new(1, 0), LineCol::new(3, 4)), Ok(r));
        assert_eq!(
            Range::new(LineCol::new(3, 0), LineCol::new(1, 0)),
            Err(CoreError::InvalidRange)
        );
    }

    #[test]
    fn line_index_rows_points_and_columns() {
        let text = "ab\ncdé\n\nx";
        let idx = LineIndex::new(text);
        assert_eq!(idx.row_at(BytePos(0)), 0);
        assert_eq!(idx.row_at(BytePos(2)), 0); // the newline belongs to row 0
        assert_eq!(idx.row_at(BytePos(3)), 1);
        assert_eq!(idx.row_at(BytePos(9)), 3);
        assert_eq!(idx.row_at(BytePos(999)), 3); // clamped
        assert_eq!(idx.start_of(1), Some(BytePos(3)));
        assert_eq!(idx.start_of(9), None);
        // 'é' is 2 bytes: its start (byte 5) is byte-column 2 = char-column 2 on
        // row 1; the position after it (byte 7) is char column 3.
        assert_eq!(idx.point_at(BytePos(5)), (1, 2));
        assert_eq!(idx.line_col(text, BytePos(7)), LineCol::new(1, 3));
        // An empty text still has one row.
        assert_eq!(LineIndex::new("").row_at(BytePos(0)), 0);
    }
}
