//! Inlay hints indexed for the per-line lookups the visual mapping needs.
//!
//! A hint is **virtual text**: it occupies screen cells but is not in the
//! buffer. Every mapping between a buffer column and a screen column therefore
//! has to know about it, or the caret lands in the wrong cell, a click selects
//! the wrong character, and a wrapped line breaks in the wrong place.
//!
//! The model, in one line:
//!
//! ```text
//! display_col(c) = width of characters before c + width of hints at columns < c
//! ```
//!
//! A hint anchored at column `c` renders immediately *before* the character at
//! `c`, but *after* the caret slot of `c`: a caret at `c` sits **before** the
//! hint, against the text it edits. With the caret at the end of `count` in
//! `let count: i32 = …`, typing inserts exactly where the caret is drawn, left
//! of the annotation, and `Right` steps over both the hint and the space. A
//! click on a hint cell resolves to `c`, so it puts the caret where that same
//! hint begins.

use karet_core::InlayHint;

/// One indexed hint.
#[derive(Clone, Copy, Debug)]
pub(super) struct Span {
    /// The line it is anchored on.
    line: u32,
    /// The column it renders before.
    col: u32,
    /// The screen cells it consumes, label plus padding.
    width: u32,
    /// Its position in the slice the index was built from, so the renderer can
    /// recover the label without searching for it again.
    source: usize,
}

/// The hints on one line, sorted by column.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LineHints<'a> {
    spans: &'a [Span],
}

impl LineHints<'_> {
    /// No hints at all — the shape every mapping had before this existed, and
    /// the one it must stay byte-identical for.
    pub(super) const EMPTY: LineHints<'static> = LineHints { spans: &[] };

    /// Whether this line has no hints, so callers can take the original path.
    pub(super) fn is_empty(self) -> bool {
        self.spans.is_empty()
    }

    /// Total width of the hints at exactly `col`.
    pub(super) fn width_at(self, col: u32) -> u32 {
        self.spans
            .iter()
            .filter(|span| span.col == col)
            .fold(0_u32, |total, span| total.saturating_add(span.width))
    }

    /// The source indices of the hints at exactly `col`, in paint order.
    pub(super) fn at(self, col: u32) -> impl Iterator<Item = usize> {
        self.spans
            .iter()
            .filter(move |span| span.col == col)
            .map(|span| span.source)
    }
}

/// Every hint in a document, sorted so one line's are a contiguous slice.
///
/// Built once per render from the slice the application supplies, and cached
/// on the editor state, so hit-testing outside a render — a mouse click —
/// resolves against exactly the geometry that was last painted.
#[derive(Clone, Debug, Default)]
pub(super) struct HintIndex {
    spans: Vec<Span>,
}

impl HintIndex {
    /// Index `hints`, measuring each label's display width.
    ///
    /// A hint that would render as nothing is dropped: zero cells cannot be
    /// seen, and keeping it would put a hint boundary at a column where the
    /// mapping shifts by nothing, which is a boundary with no meaning.
    pub(super) fn new(hints: &[InlayHint]) -> Self {
        let mut spans: Vec<Span> = hints
            .iter()
            .enumerate()
            .filter_map(|(source, hint)| {
                let width = hint_width(hint);
                (width > 0).then_some(Span {
                    line: hint.position.line,
                    col: hint.position.col,
                    width,
                    source,
                })
            })
            .collect();
        // Sorted by line then column so a line is a contiguous slice and
        // the slicing below is sound. `sort_by_key` is stable, so two
        // hints at one column keep the order the server sent them, which is the
        // order they are painted in.
        spans.sort_by_key(|span| (span.line, span.col));
        Self { spans }
    }

    /// The hints on `line`.
    pub(super) fn line(&self, line: u32) -> LineHints<'_> {
        let start = self.spans.partition_point(|span| span.line < line);
        let end = self.spans.partition_point(|span| span.line <= line);
        LineHints {
            spans: self.spans.get(start..end).unwrap_or_default(),
        }
    }
}

/// The screen cells one hint consumes: its label plus its padding.
fn hint_width(hint: &InlayHint) -> u32 {
    let label = unicode_width::UnicodeWidthStr::width(hint.label.as_str());
    let padding = usize::from(hint.padding_left) + usize::from(hint.padding_right);
    u32::try_from(label.saturating_add(padding)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use karet_core::InlayHintKind;
    use karet_core::LineCol;

    use super::*;

    fn hint(line: u32, col: u32, label: &str) -> InlayHint {
        InlayHint {
            position: LineCol::new(line, col),
            label: label.to_owned(),
            kind: InlayHintKind::Type,
            padding_left: false,
            padding_right: false,
        }
    }

    #[test]
    fn an_empty_index_reports_nothing_on_every_line() {
        let index = HintIndex::new(&[]);
        assert!(index.line(0).is_empty());
        assert_eq!(index.line(0).width_at(0), 0);
    }

    #[test]
    fn hints_group_by_line_whatever_order_they_arrive_in() {
        let index = HintIndex::new(&[
            hint(4, 2, "b"),
            hint(0, 9, ": i32"),
            hint(4, 0, "a"),
            hint(2, 1, "mid"),
        ]);
        assert_eq!(index.line(0).width_at(9), 5);
        assert!(index.line(1).is_empty());
        assert_eq!(index.line(2).width_at(1), 3);
        // Both of line 4's hints, each found at its own column.
        assert_eq!(index.line(4).width_at(0), 1);
        assert_eq!(index.line(4).width_at(2), 1);
    }

    #[test]
    fn a_hint_is_found_only_at_the_column_it_annotates() {
        // A hint is found only at its own anchor, never at its neighbours:
        // the mappings decide which side of the caret it falls on, not the
        // index.
        let index = HintIndex::new(&[hint(0, 9, ": i32")]);
        let line = index.line(0);
        assert_eq!(line.width_at(9), 5);
        assert_eq!(line.width_at(8), 0);
        assert_eq!(line.width_at(10), 0);
    }

    #[test]
    fn padding_counts_toward_the_cells_a_hint_occupies() {
        let padded = InlayHint {
            padding_left: true,
            padding_right: true,
            ..hint(0, 0, "x")
        };
        let index = HintIndex::new(&[padded]);
        assert_eq!(index.line(0).width_at(0), 3);
    }

    #[test]
    fn a_hint_that_would_render_as_nothing_is_dropped() {
        // A zero-width span is a mapping boundary where nothing shifts, which
        // would make `source_col_at_display_offset` able to return a column
        // that occupies no cells.
        let index = HintIndex::new(&[hint(0, 3, "")]);
        assert!(index.line(0).is_empty());
        assert_eq!(index.line(0).width_at(3), 0);
    }

    #[test]
    fn a_wide_label_counts_its_display_width_not_its_bytes() {
        let index = HintIndex::new(&[hint(0, 0, "日本")]);
        assert_eq!(index.line(0).width_at(0), 4);
    }

    #[test]
    fn two_hints_at_one_column_keep_the_order_they_arrived_in() {
        let index = HintIndex::new(&[hint(0, 2, "first"), hint(0, 2, "second")]);
        let line = index.line(0);
        assert_eq!(line.width_at(2), 11);
        assert_eq!(line.at(2).collect::<Vec<_>>(), vec![0, 1]);
    }
}
