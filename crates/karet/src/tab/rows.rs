//! The copyable rows of the read-only tab surfaces: a diff column, a hex dump
//! and a rendered Markdown preview.
//!
//! One builder per surface, shared by the two readers that need the same
//! answer — the app's copy (`app::select`) and the selection painter
//! (`ui::content::select`) — so neither depends on the other's copy of it.

use karet_markdown::WrappedDocument;

use super::ChangedFile;

/// The copyable text of one row, and how wide the chrome before it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceRow {
    /// The row's text, without the gutter or marker painted ahead of it.
    pub(crate) text: String,
    /// Columns of chrome between the region's left edge and that text.
    pub(crate) content_x: u16,
}

/// Which painted column of a diff a row is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiffColumn {
    /// The unified view's single column.
    Unified,
    /// The side-by-side view's old (left) column.
    Old,
    /// The side-by-side view's new (right) column.
    New,
}

/// The copyable content of markdown preview row `row` of `wrapped`.
///
/// A preview row's text is what it renders — bullets, rules and table borders
/// included — because that is what the reader sees and points at. The markdown
/// source is not recoverable per row: `karet-markdown` anchors only top-level
/// blocks, so mapping a row back to source would be an interpolation, not a fact.
pub(crate) fn markdown_row(wrapped: &WrappedDocument, row: usize) -> Option<SurfaceRow> {
    Some(SurfaceRow {
        text: wrapped.lines.get(row)?.text(),
        content_x: 0,
    })
}

/// The copyable content of hex row `row` of `bytes`.
pub(crate) fn hex_row(bytes: &[u8], row: usize) -> Option<SurfaceRow> {
    Some(SurfaceRow {
        text: karet_fileview::hex::row_text(bytes, row)?,
        content_x: karet_fileview::hex::OFFSET_WIDTH,
    })
}

/// The copyable content of `column`'s row `row` of `file`.
///
/// A row's gutter width is not constant across a file — a line number past four
/// digits widens it — so it travels with the text rather than being assumed.
pub(crate) fn diff_row(file: &ChangedFile, column: DiffColumn, row: usize) -> Option<SurfaceRow> {
    let content = match column {
        DiffColumn::Unified => karet_diff::unified_row(&file.change.diff, row),
        DiffColumn::Old => karet_diff::side_by_side_row(&file.change.diff, row).0,
        DiffColumn::New => karet_diff::side_by_side_row(&file.change.diff, row).1,
    }?;
    Some(SurfaceRow {
        text: content.text,
        content_x: content.gutter_width,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::test_changed_file;

    #[test]
    fn a_diff_row_carries_its_text_without_the_gutter() {
        let file = test_changed_file("notes.txt", "old\n", "new\n");
        let rows: Vec<String> = (0..8)
            .filter_map(|row| diff_row(&file, DiffColumn::Unified, row))
            .map(|row| row.text)
            .collect();
        assert!(rows.iter().any(|text| text == "old"), "{rows:?}");
        assert!(rows.iter().any(|text| text == "new"), "{rows:?}");
    }

    #[test]
    fn the_side_by_side_columns_answer_their_own_side() {
        let file = test_changed_file("notes.txt", "old\n", "new\n");
        let side = |column| {
            (0..8)
                .filter_map(|row| diff_row(&file, column, row))
                .map(|row| row.text)
                .collect::<Vec<_>>()
        };
        assert!(side(DiffColumn::Old).iter().any(|text| text == "old"));
        assert!(!side(DiffColumn::Old).iter().any(|text| text == "new"));
        assert!(side(DiffColumn::New).iter().any(|text| text == "new"));
    }

    #[test]
    fn a_hex_row_starts_after_the_offset_column() {
        let row = hex_row(b"abc", 0);
        assert_eq!(
            row.map(|row| row.content_x),
            Some(karet_fileview::hex::OFFSET_WIDTH)
        );
        assert_eq!(hex_row(b"abc", 1), None);
    }
}
