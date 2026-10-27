//! Row geometry of a painted [`PreparedDiff`], available without the `view`
//! feature.
//!
//! The painters (`unified_lines` / `side_by_side_lines`, feature `view`) lay a
//! diff out as, per hunk, an optional scope line, the `@@ … @@` header, then the
//! hunk body, followed by one trailing blank line. The functions here answer
//! questions about that layout — which hunk a display row falls in, which row a
//! hunk opens on, what copyable text a row carries — without ratatui, so a
//! headless consumer can map rows to hunks and the painters stay pure
//! presentation.

use crate::align::Cell;
use crate::align_hunk;
use crate::model::Hunk;
use crate::prepared::PreparedDiff;

/// The display rows one hunk occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HunkSpan {
    /// The hunk's index in the diff.
    index: usize,
    /// The display row its first line (scope or header) is painted on.
    start: usize,
    /// How many chrome rows (scope and header) open it.
    head: usize,
    /// How many body rows follow that chrome.
    body: usize,
}

impl HunkSpan {
    /// The first display row past this hunk.
    fn end(self) -> usize {
        self.start + self.head + self.body
    }
}

/// The row spans of every hunk of `prepared`, in display order, with
/// `body_rows` counting each hunk's body. Empty for a binary diff, whose
/// placeholder carries no hunk.
fn hunk_spans(
    prepared: &PreparedDiff,
    body_rows: impl Fn(&Hunk) -> usize,
) -> impl Iterator<Item = HunkSpan> {
    let hunks: &[Hunk] = if prepared.is_binary() {
        &[]
    } else {
        &prepared.diff().hunks
    };
    let mut next_row = 0usize;
    hunks.iter().enumerate().map(move |(index, hunk)| {
        let span = HunkSpan {
            index,
            start: next_row,
            head: usize::from(hunk.scope.is_some()) + 1,
            body: body_rows(hunk),
        };
        next_row = span.end();
        span
    })
}

/// The unified body row count of `hunk`: one row per diff line.
fn unified_body(hunk: &Hunk) -> usize {
    hunk.lines.len()
}

/// The side-by-side body row count of `hunk`: one row per aligned pair.
fn side_by_side_body(hunk: &Hunk) -> usize {
    align_hunk(&hunk.lines).len()
}

/// Which hunk of `prepared` the unified view's display `row` falls in.
///
/// Mirrors `unified_lines`'s layout exactly (optional scope line, header, then
/// the hunk body). Rows past the last hunk (the trailing blank line) map to the
/// last hunk, so "the hunk at the viewport top" stays well-defined at the end of
/// the document; a binary or hunkless diff has no answer.
#[must_use]
pub fn unified_hunk_at_row(prepared: &PreparedDiff, row: usize) -> Option<usize> {
    hunk_at_row(prepared, row, unified_body)
}

/// Which hunk of `prepared` the side-by-side view's display `row` falls in
/// (both panes share row geometry). Same conventions as
/// [`unified_hunk_at_row`].
#[must_use]
pub fn side_by_side_hunk_at_row(prepared: &PreparedDiff, row: usize) -> Option<usize> {
    hunk_at_row(prepared, row, side_by_side_body)
}

fn hunk_at_row(
    prepared: &PreparedDiff,
    row: usize,
    body_rows: impl Fn(&Hunk) -> usize,
) -> Option<usize> {
    hunk_spans(prepared, body_rows)
        .take_while(|span| span.start <= row)
        .last()
        .map(|span| span.index)
}

/// The unified view's display row on which hunk `index` of `prepared` opens:
/// its scope line when it has one, otherwise its header.
///
/// The inverse of [`unified_hunk_at_row`]: scrolling the view to this row puts
/// the hunk at the top of the viewport. `None` for a binary or hunkless diff,
/// or an `index` past the last hunk.
#[must_use]
pub fn unified_hunk_row(prepared: &PreparedDiff, index: usize) -> Option<usize> {
    hunk_row(prepared, index, unified_body)
}

/// The side-by-side view's display row on which hunk `index` of `prepared`
/// opens; the inverse of [`side_by_side_hunk_at_row`], with the conventions of
/// [`unified_hunk_row`].
#[must_use]
pub fn side_by_side_hunk_row(prepared: &PreparedDiff, index: usize) -> Option<usize> {
    hunk_row(prepared, index, side_by_side_body)
}

fn hunk_row(
    prepared: &PreparedDiff,
    index: usize,
    body_rows: impl Fn(&Hunk) -> usize,
) -> Option<usize> {
    hunk_spans(prepared, body_rows)
        .nth(index)
        .map(|span| span.start)
}

/// The copyable content of one painted diff row.
///
/// The text is the line's own content — the line-number gutter and the
/// `+`/`-` marker are chrome, so they are neither part of it nor of anything a
/// selection over the row can copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowContent {
    /// The row's text, exactly as painted after its gutter.
    pub text: String,
    /// How many display columns of gutter precede that text.
    pub gutter_width: u16,
}

/// The content of the unified view's display `row`.
///
/// `None` when the row carries no copyable text: a scope line, a hunk header, a
/// binary/hunkless placeholder, or the trailing blank. Mirrors
/// `unified_lines`'s layout exactly, the same invariant
/// [`unified_hunk_at_row`] holds.
#[must_use]
pub fn unified_row(prepared: &PreparedDiff, row: usize) -> Option<RowContent> {
    let (hunk, body) = body_row_at(prepared, row, unified_body)?;
    let line = prepared.diff().hunks.get(hunk)?.lines.get(body)?;
    Some(RowContent {
        text: line.content.clone(),
        gutter_width: gutter_width(line.old_lineno)
            .saturating_add(gutter_width(line.new_lineno))
            .saturating_add(1),
    })
}

/// The `(old, new)` content of the side-by-side view's display `row`.
///
/// Either side is `None` where the alignment left that pane empty, and both are
/// `None` on a row with no copyable text. Mirrors `side_by_side_lines`'s
/// layout, as [`side_by_side_hunk_at_row`] does.
#[must_use]
pub fn side_by_side_row(
    prepared: &PreparedDiff,
    row: usize,
) -> (Option<RowContent>, Option<RowContent>) {
    let content = |cell: Option<&Cell>| {
        cell.map(|cell| RowContent {
            text: cell.content.clone(),
            gutter_width: gutter_width(Some(cell.lineno)),
        })
    };
    let Some((index, body)) = body_row_at(prepared, row, side_by_side_body) else {
        return (None, None);
    };
    let Some(hunk) = prepared.diff().hunks.get(index) else {
        return (None, None);
    };
    let rows = align_hunk(&hunk.lines);
    let Some(pair) = rows.get(body) else {
        return (None, None);
    };
    (content(pair.left.as_ref()), content(pair.right.as_ref()))
}

/// Which `(hunk, body row)` of `prepared` the display `row` falls on, skipping
/// the scope and header rows that open each hunk.
fn body_row_at(
    prepared: &PreparedDiff,
    row: usize,
    body_rows: impl Fn(&Hunk) -> usize,
) -> Option<(usize, usize)> {
    let span = hunk_spans(prepared, body_rows).find(|span| row < span.end())?;
    let body_start = span.start + span.head;
    (row >= body_start).then(|| (span.index, row - body_start))
}

/// The display width of one line-number gutter cell, mirroring the painters'
/// gutter: a right-aligned number of at least four digits, then one space. A
/// line number wider than four digits widens the gutter, so this is not a
/// constant.
fn gutter_width(lineno: Option<u32>) -> u16 {
    let digits = lineno.map_or(4, |lineno| lineno.to_string().len().max(4));
    u16::try_from(digits).unwrap_or(u16::MAX).saturating_add(1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::DiffOptions;
    use crate::diff_text;

    pub(crate) fn prepared(old: &str, new: &str) -> PreparedDiff {
        prepared_with(old, new, |_| {})
    }

    /// Like [`prepared`], with `adjust` applied to the line diff before it is
    /// prepared (a prepared diff is immutable).
    pub(crate) fn prepared_with(
        old: &str,
        new: &str,
        adjust: impl FnOnce(&mut crate::FileDiff),
    ) -> PreparedDiff {
        let mut diff = diff_text(old, new, &DiffOptions::default());
        adjust(&mut diff);
        PreparedDiff::new(diff, Vec::new(), Vec::new())
    }

    /// Three hunks, the first rewriting two lines (so the side-by-side body,
    /// which pairs them, is shorter than the unified one) and the second
    /// carrying a scope line.
    pub(crate) fn three_hunks() -> PreparedDiff {
        let old: String = (0..30).map(|n| format!("l{n}\n")).collect();
        let new = old
            .replacen("l0\nl1\n", "L0\nL1\n", 1)
            .replacen("l14\n", "L14\n", 1)
            .replacen("l29\n", "", 1);
        prepared_with(&old, &new, |diff| {
            if let Some(hunk) = diff.hunks.get_mut(1) {
                hunk.scope = Some("fn middle".to_string());
            }
        })
    }

    fn binary() -> PreparedDiff {
        let mut binary_diff = diff_text("a\n", "b\n", &DiffOptions::default());
        binary_diff.is_binary = true;
        PreparedDiff::new(binary_diff, Vec::new(), Vec::new())
    }

    #[test]
    fn hunk_spans_tile_the_rows_with_scope_header_and_body() {
        let prep = three_hunks();
        let spans: Vec<HunkSpan> = hunk_spans(&prep, unified_body).collect();
        assert_eq!(spans.len(), 3);
        // Each span starts where the previous one ends: no gap, no overlap.
        assert_eq!(spans[0].start, 0);
        for pair in spans.windows(2) {
            assert_eq!(pair[1].start, pair[0].end());
        }
        // Only the scoped hunk carries the extra chrome row.
        let heads: Vec<usize> = spans.iter().map(|span| span.head).collect();
        assert_eq!(heads, [1, 2, 1]);
        for (span, hunk) in spans.iter().zip(&prep.diff().hunks) {
            assert_eq!(span.body, hunk.lines.len());
        }
        // A binary diff yields no spans even when it still carries hunks.
        let binary = binary();
        assert!(!binary.diff().hunks.is_empty());
        assert_eq!(hunk_spans(&binary, unified_body).count(), 0);
    }

    #[test]
    fn hunk_at_row_answers_every_row_of_a_hunk_and_clamps_past_the_end() {
        let prep = three_hunks();
        for span in hunk_spans(&prep, unified_body) {
            for row in span.start..span.end() {
                assert_eq!(unified_hunk_at_row(&prep, row), Some(span.index));
            }
        }
        for span in hunk_spans(&prep, side_by_side_body) {
            for row in span.start..span.end() {
                assert_eq!(side_by_side_hunk_at_row(&prep, row), Some(span.index));
            }
        }
        assert_eq!(unified_hunk_at_row(&prep, usize::MAX), Some(2));
        assert_eq!(side_by_side_hunk_at_row(&prep, usize::MAX), Some(2));
        assert_eq!(unified_hunk_at_row(&binary(), 0), None);
        assert_eq!(unified_hunk_at_row(&prepared("same\n", "same\n"), 0), None);
    }

    #[test]
    fn hunk_row_has_no_answer_past_the_hunks_or_for_a_binary_diff() {
        let prep = three_hunks();
        assert_eq!(unified_hunk_row(&prep, 3), None);
        assert_eq!(side_by_side_hunk_row(&prep, usize::MAX), None);
        let binary = binary();
        assert_eq!(unified_hunk_row(&binary, 0), None);
        assert_eq!(side_by_side_hunk_row(&binary, 0), None);
    }

    #[test]
    fn body_row_at_skips_the_chrome_that_opens_each_hunk() {
        let prep = three_hunks();
        let spans: Vec<HunkSpan> = hunk_spans(&prep, unified_body).collect();
        let scoped = spans[1];
        // The scope and header rows of the scoped hunk carry no body row...
        assert_eq!(body_row_at(&prep, scoped.start, unified_body), None);
        assert_eq!(body_row_at(&prep, scoped.start + 1, unified_body), None);
        // ...its first body row is the one after them, its last the one before
        // the next hunk opens.
        assert_eq!(
            body_row_at(&prep, scoped.start + 2, unified_body),
            Some((1, 0))
        );
        assert_eq!(
            body_row_at(&prep, scoped.end() - 1, unified_body),
            Some((1, scoped.body - 1))
        );
        // The trailing blank past the last hunk is no body row.
        assert_eq!(body_row_at(&prep, spans[2].end(), unified_body), None);
    }

    #[test]
    fn row_content_reports_the_line_text_and_both_gutters() {
        let prep = prepared("before\n", "after\n");
        // Row 0 is the header; rows 1 and 2 are the remove and the add.
        assert_eq!(unified_row(&prep, 0), None);
        let removed = unified_row(&prep, 1);
        assert_eq!(
            removed,
            Some(RowContent {
                text: "before".to_string(),
                gutter_width: 11,
            })
        );
        assert_eq!(
            unified_row(&prep, 2).map(|content| content.text),
            Some("after".to_string())
        );
        // Side by side, the pair shares one row with one gutter per pane.
        let (old, new) = side_by_side_row(&prep, 1);
        assert_eq!(
            old,
            Some(RowContent {
                text: "before".to_string(),
                gutter_width: 5,
            })
        );
        assert_eq!(new.map(|content| content.text), Some("after".to_string()));
        assert_eq!(side_by_side_row(&prep, 2), (None, None));
    }

    #[test]
    fn a_wide_line_number_widens_the_gutter_width() {
        // Four digits is the minimum width, so 9999 and 1 gutter alike...
        assert_eq!(gutter_width(Some(1)), 5);
        assert_eq!(gutter_width(Some(9999)), 5);
        assert_eq!(gutter_width(None), 5);
        // ...but a fifth digit pushes the content one column right.
        assert_eq!(gutter_width(Some(10_000)), 6);
    }

    #[test]
    fn a_binary_or_hunkless_diff_has_no_selectable_rows() {
        let mut binary_diff = diff_text("a\n", "b\n", &DiffOptions::default());
        binary_diff.is_binary = true;
        binary_diff.hunks.clear();
        let binary = PreparedDiff::new(binary_diff, Vec::new(), Vec::new());
        // The placeholder and its trailing blank are chrome, not content.
        assert_eq!(unified_row(&binary, 0), None);
        assert_eq!(unified_row(&binary, 1), None);
        assert_eq!(side_by_side_row(&binary, 0), (None, None));
        // A binary diff that still carries hunks has none either.
        assert_eq!(unified_row(&self::tests::binary(), 1), None);

        let identical = prepared("same\n", "same\n");
        assert_eq!(unified_row(&identical, 0), None);
    }
}
