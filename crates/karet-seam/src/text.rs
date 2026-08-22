//! Byte offset → line/column conversion.
//!
//! The walk speaks byte spans; navigation speaks line and column. One table per file,
//! built once, converts between them.

use karet_core::BytePos;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Span;

/// The byte offset each line of a source file starts at.
#[derive(Debug, Clone, Default)]
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    /// Build the table for `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0usize];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { starts }
    }

    /// How many lines the file has.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The zero-based line/column for a byte offset.
    ///
    /// Columns count Unicode scalar values, matching karet's canonical unit, so a
    /// multi-byte character advances the column by one rather than by its byte width.
    /// An offset past the end clamps to the last position rather than failing.
    #[must_use]
    pub fn line_col(&self, text: &str, offset: BytePos) -> LineCol {
        let offset = offset.0.min(text.len());
        let line = self
            .starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let start = self.starts.get(line).copied().unwrap_or(0);
        let col = text
            .get(start..offset)
            .map_or(0, |slice| slice.chars().count());
        LineCol {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            col: u32::try_from(col).unwrap_or(u32::MAX),
        }
    }

    /// The line/column range covering a byte span.
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
    fn maps_offsets_to_lines_and_columns() {
        let text = "fn a() {}\nfn b() {}\n";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(index.line_col(text, BytePos(0)), LineCol::new(0, 0));
        assert_eq!(index.line_col(text, BytePos(3)), LineCol::new(0, 3));
        assert_eq!(index.line_col(text, BytePos(10)), LineCol::new(1, 0));
        assert_eq!(index.line_col(text, BytePos(13)), LineCol::new(1, 3));
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        // `é` is two bytes but one column.
        let text = "let é = 1;";
        let index = LineIndex::new(text);
        let after = index.line_col(text, BytePos("let é".len()));
        assert_eq!(after, LineCol::new(0, 5));
    }

    #[test]
    fn an_offset_past_the_end_clamps() {
        let text = "abc";
        let index = LineIndex::new(text);
        assert_eq!(index.line_col(text, BytePos(999)), LineCol::new(0, 3));
    }

    #[test]
    fn spans_convert_to_ranges() {
        let text = "fn a() {}\nfn b() {}\n";
        let index = LineIndex::new(text);
        let range = index.range(
            text,
            Span {
                start: BytePos(10),
                end: BytePos(19),
            },
        );
        assert_eq!(range.start, LineCol::new(1, 0));
        assert_eq!(range.end, LineCol::new(1, 9));
    }

    #[test]
    fn an_empty_file_has_one_line() {
        let index = LineIndex::new("");
        assert_eq!(index.line_count(), 1);
        assert_eq!(index.line_col("", BytePos(0)), LineCol::new(0, 0));
    }
}
