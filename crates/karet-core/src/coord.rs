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
///
/// A `char` is not what a user perceives as one character: a combining
/// sequence, a ZWJ emoji or a flag spans several. Editing code that moves or
/// deletes by "one character" steps an extended grapheme cluster, so a caret
/// produced by the editor rests on a cluster boundary even though the column
/// itself still counts `char`s.
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
/// Engines that work over a borrowed `&str` (highlighters, outline extractors)
/// need "which row is this byte on?"; this is the shared implementation for the
/// `karet-*` crates. Buffers that *own* mutable text keep using
/// `karet_text::TextBuffer`'s conversions instead, and the standalone
/// `blameline` crate (which takes no direct `karet-core` dependency) keeps its
/// own small line-start table for function narrowing.
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
    ///
    /// An offset that is not a position in `text` is clamped to the nearest one
    /// before it: an offset inside a multi-byte `char` gets that `char`'s column,
    /// and an offset past the end of `text` gets the end of the last row. The row
    /// is the one [`row_at`](Self::row_at) reports.
    #[must_use]
    pub fn line_col(&self, text: &str, pos: BytePos) -> LineCol {
        let (row, byte_col) = self.point_at(pos);
        let start = self.starts.get(row as usize).copied().unwrap_or(0);
        let end = text.floor_char_boundary(start.saturating_add(byte_col));
        let col = text.get(start..end).map_or(0, |s| s.chars().count());
        LineCol::new(row, u32::try_from(col).unwrap_or(u32::MAX))
    }

    /// The [`Range`] covering byte span `span` within `text`: both ends converted
    /// through [`line_col`](Self::line_col).
    ///
    /// `text` must be the same snapshot the index was built from.
    #[must_use]
    pub fn range(&self, text: &str, span: Span) -> Range {
        Range {
            start: self.line_col(text, span.start),
            end: self.line_col(text, span.end),
        }
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
        assert!(Span::default().is_empty());
        assert!(
            Span {
                start: BytePos(4),
                end: BytePos(4),
            }
            .is_empty()
        );
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
        assert!(!r.is_empty());
        assert!(Range::default().is_empty());
        let caret = LineCol::new(2, 7);
        assert!(
            Range {
                start: caret,
                end: caret,
            }
            .is_empty()
        );
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

    #[test]
    fn line_col_at_the_end_of_text_and_off_a_char_boundary() {
        let text = "ab\ncdé\n\nx";
        let idx = LineIndex::new(text);
        // The end of the text is a real position: after the last char.
        assert_eq!(idx.line_col(text, BytePos(text.len())), LineCol::new(3, 1));
        // The empty row between two newlines has only column 0.
        assert_eq!(idx.line_col(text, BytePos(8)), LineCol::new(2, 0));
        // An offset inside 'é' (bytes 5..7) is not a position in `text`; it keeps
        // its row and clamps back to the column of 'é' itself (2), not 0 and not
        // the column after it (3).
        assert_eq!(idx.point_at(BytePos(6)), (1, 3));
        assert_eq!(idx.line_col(text, BytePos(6)), LineCol::new(1, 2));
        // Past the end clamps to the end of the last row, the same point as the
        // end of the text itself.
        assert_eq!(idx.line_col(text, BytePos(999)), LineCol::new(3, 1));
        assert_eq!(idx.line_col(text, BytePos(usize::MAX)), LineCol::new(3, 1));
    }

    #[test]
    fn line_col_clamps_inside_a_wide_char_and_past_the_end_of_a_one_row_text() {
        // '€' is 3 bytes (1..4): both interior offsets clamp to its column 1.
        let text = "a€b";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_col(text, BytePos(2)), LineCol::new(0, 1));
        assert_eq!(idx.line_col(text, BytePos(3)), LineCol::new(0, 1));
        assert_eq!(idx.line_col(text, BytePos(4)), LineCol::new(0, 2));
        // Past the end of a text ending mid-row: the end of that row, the
        // column just after its last char.
        assert_eq!(idx.line_col(text, BytePos(999)), LineCol::new(0, 3));
        // A text ending in a multi-byte char: past the end counts that char.
        let tail = "xé";
        let tail_idx = LineIndex::new(tail);
        assert_eq!(tail_idx.line_col(tail, BytePos(2)), LineCol::new(0, 1));
        assert_eq!(tail_idx.line_col(tail, BytePos(50)), LineCol::new(0, 2));
        // A span that runs past the end converts to a range ending at the end.
        assert_eq!(
            idx.range(
                text,
                Span {
                    start: BytePos(2),
                    end: BytePos(999),
                }
            ),
            Range {
                start: LineCol::new(0, 1),
                end: LineCol::new(0, 3),
            }
        );
    }

    #[test]
    fn line_index_converts_spans_to_ranges() {
        let text = "fn a() {}\nfn é() {}\n";
        let idx = LineIndex::new(text);
        assert_eq!(idx.line_count(), 3);
        // An empty text still has one line, and its only span is the empty range.
        let empty = LineIndex::new("");
        assert_eq!(empty.line_count(), 1);
        assert_eq!(empty.range("", Span::default()), Range::default());
        // A span on the second row, its end counted in chars past the 2-byte 'é'.
        let span = Span {
            start: BytePos(10),
            end: BytePos(text.len() - 1),
        };
        assert_eq!(
            idx.range(text, span),
            Range {
                start: LineCol::new(1, 0),
                end: LineCol::new(1, 9),
            }
        );
        // A span crossing a newline ends on the next row.
        let across = Span {
            start: BytePos(3),
            end: BytePos(13),
        };
        assert_eq!(
            idx.range(text, across),
            Range {
                start: LineCol::new(0, 3),
                end: LineCol::new(1, 3),
            }
        );
    }
}
