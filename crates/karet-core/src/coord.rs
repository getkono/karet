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

/// An absolute offset measured in Unicode scalar values (`char`s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CharPos(pub usize);

/// A zero-based line/column position.
///
/// `col` is counted in the active [`PositionEncoding`]; karet's internal default
/// is `Utf32` (Unicode scalar values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LineCol {
    /// Zero-based line index.
    pub line: u32,
    /// Zero-based column index, in [`PositionEncoding`] units.
    pub col: u32,
}

impl LineCol {
    /// Create a position at `line` / `col`.
    #[must_use]
    pub const fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

/// The unit in which a [`LineCol`] column is counted at a protocol boundary.
///
/// karet's canonical internal unit is [`PositionEncoding::Utf32`]; LSP defaults to
/// `Utf16` and must be translated at the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PositionEncoding {
    /// Columns counted in UTF-8 code units (bytes).
    Utf8,
    /// Columns counted in UTF-16 code units.
    Utf16,
    /// Columns counted in Unicode scalar values (`char`s).
    #[default]
    Utf32,
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
        assert_eq!(PositionEncoding::default(), PositionEncoding::Utf32);
    }
}
