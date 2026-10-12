//! Virtual text indexed for the per-line lookups the visual mapping and the
//! painter need.
//!
//! Virtual text occupies screen cells but is not in the buffer. It comes in
//! two placements ([`VirtualTextPlacement`]), and each is handled in exactly
//! one place:
//!
//! - **Inline** text is part of the line's layout, so every mapping between a
//!   buffer column and a screen column has to know about it, or the caret
//!   lands in the wrong cell, a click selects the wrong character, and a
//!   wrapped line breaks in the wrong place. [`HintIndex::line`] serves it to
//!   those mappings.
//! - **End-of-line** text is outside the layout: it takes only the cells the
//!   line's last row leaves free, so no mapping changes for it, and
//!   [`place_eol`] fits it into that slot when the row is painted.
//!
//! The inline model, in one line:
//!
//! ```text
//! display_col(c) = width of characters before c + width of inline text at columns < c
//! ```
//!
//! Text anchored at column `c` renders immediately *before* the character at
//! `c`, but *after* the caret slot of `c`: a caret at `c` sits **before** it,
//! against the text it edits. With the caret at the end of `count` in
//! `let count: i32 = …`, typing inserts exactly where the caret is drawn, left
//! of the annotation, and `Right` steps over both the annotation and the
//! space. A click on an annotation cell resolves to `c`, so it puts the caret
//! where that same annotation begins.

use karet_core::EolAlign;
use karet_core::VirtualText;
use karet_core::VirtualTextPlacement;
use ratatui::layout::Rect;
use ratatui::text::Span as RowSpan;

/// One indexed inline text.
#[derive(Clone, Copy, Debug)]
pub(super) struct Span {
    /// The line it is anchored on.
    line: u32,
    /// The column it renders before.
    col: u32,
    /// The screen cells it consumes.
    width: u32,
    /// Its position in the slice the index was built from, so the renderer can
    /// recover the text without searching for it again.
    source: usize,
}

/// The inline text on one line, sorted by column.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LineHints<'a> {
    spans: &'a [Span],
}

impl LineHints<'_> {
    /// Whether this line has no inline text, so callers can take the path
    /// every mapping had before it existed, and must stay byte-identical for.
    pub(super) fn is_empty(self) -> bool {
        self.spans.is_empty()
    }

    /// Total width of the inline text at exactly `col`.
    pub(super) fn width_at(self, col: u32) -> u32 {
        self.spans
            .iter()
            .filter(|span| span.col == col)
            .fold(0_u32, |total, span| total.saturating_add(span.width))
    }

    /// The source indices of the inline text at exactly `col`, in paint order.
    pub(super) fn at(self, col: u32) -> impl Iterator<Item = usize> {
        self.spans
            .iter()
            .filter(move |span| span.col == col)
            .map(|span| span.source)
    }
}

/// Every virtual text in a document, sorted so one line's are a contiguous
/// slice.
///
/// Built once per render from the slice the application supplies, and cached
/// on the editor state, so hit-testing outside a render — a mouse click —
/// resolves against exactly the geometry that was last painted.
#[derive(Clone, Debug, Default)]
pub(super) struct HintIndex {
    spans: Vec<Span>,
    /// `(line, source)` of each end-of-line text, sorted by line.
    eol: Vec<(u32, usize)>,
}

impl HintIndex {
    /// Index `texts`, measuring each one's display width.
    ///
    /// A text that would render as nothing is dropped: zero cells cannot be
    /// seen, and keeping an inline one would put a boundary at a column where
    /// the mapping shifts by nothing, which is a boundary with no meaning.
    pub(super) fn new(texts: &[VirtualText]) -> Self {
        let mut spans = Vec::new();
        let mut eol = Vec::new();
        for (source, text) in texts.iter().enumerate() {
            let width = text_width(&text.text);
            if width == 0 {
                continue;
            }
            match text.placement {
                VirtualTextPlacement::Inline { col } => spans.push(Span {
                    line: text.line,
                    col,
                    width,
                    source,
                }),
                VirtualTextPlacement::EndOfLine { .. } => eol.push((text.line, source)),
                // A placement newer than this renderer: painting it somewhere
                // it was not meant to go would be worse than leaving it out.
                _ => {},
            }
        }
        // Sorted by line then column so a line is a contiguous slice and the
        // slicing below is sound. `sort_by_key` is stable, so two texts at one
        // column keep the order they were supplied in, which is the order they
        // are painted in.
        spans.sort_by_key(|span| (span.line, span.col));
        eol.sort_by_key(|&(line, _)| line);
        Self { spans, eol }
    }

    /// The inline text on `line`.
    pub(super) fn line(&self, line: u32) -> LineHints<'_> {
        let start = self.spans.partition_point(|span| span.line < line);
        let end = self.spans.partition_point(|span| span.line <= line);
        LineHints {
            spans: self.spans.get(start..end).unwrap_or_default(),
        }
    }

    /// The source indices of `line`'s end-of-line text, in supply order.
    pub(super) fn eol_on(&self, line: u32) -> impl Iterator<Item = usize> {
        let start = self.eol.partition_point(|&(at, _)| at < line);
        let end = self.eol.partition_point(|&(at, _)| at <= line);
        self.eol
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .map(|&(_, source)| source)
    }
}

/// The screen cells `text` consumes when painted.
pub(super) fn text_width(text: &str) -> u32 {
    u32::try_from(unicode_width::UnicodeWidthStr::width(text)).unwrap_or(u32::MAX)
}

/// One end-of-line text waiting for a place in its line's slot.
#[derive(Clone, Copy, Debug)]
pub(super) struct EolItem<'a> {
    /// Its index in the slice the index was built from.
    pub(super) source: usize,
    /// What it says.
    pub(super) text: &'a str,
    /// Which edge of the slot it keeps to.
    pub(super) align: EolAlign,
    /// Higher keeps its cells first.
    pub(super) priority: u8,
}

/// One end-of-line text as [`place_eol`] fitted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EolPlaced {
    /// Its index in the slice the index was built from.
    pub(super) source: usize,
    /// Cells from the start of the slot to its first cell.
    pub(super) offset: u32,
    /// What is painted: the text, or its head ending in `…` when cut short.
    pub(super) text: String,
}

/// Fit `items` into an end-of-line slot `room` cells wide.
///
/// Cells are handed out in priority order, highest first and, among equals,
/// first-supplied first; whatever does not get its full width is cut short
/// with `…`, and whatever gets none is left out. Left-aligned text then packs
/// from the slot's start in supply order, and right-aligned text packs against
/// its end, also in supply order — which can never overlap, since the cells
/// handed out sum to at most `room`. Returned in paint order, left to right.
pub(super) fn place_eol(items: &[EolItem<'_>], room: u32) -> Vec<EolPlaced> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(items.get(index).map_or(0, |item| item.priority)));
    let mut allotted = vec![0_u32; items.len()];
    let mut left = room;
    for index in order {
        let (Some(item), Some(cells)) = (items.get(index), allotted.get_mut(index)) else {
            continue;
        };
        *cells = text_width(item.text).min(left);
        left -= *cells;
    }
    let fitted = |align: EolAlign| {
        items
            .iter()
            .zip(&allotted)
            .filter(move |(item, cells)| item.align == align && **cells > 0)
    };
    let mut placed = Vec::new();
    let mut offset = 0_u32;
    for (item, &cells) in fitted(EolAlign::Left) {
        placed.push(EolPlaced {
            source: item.source,
            offset,
            text: truncate(item.text, cells),
        });
        offset = offset.saturating_add(cells);
    }
    let right: u32 = fitted(EolAlign::Right).map(|(_, &cells)| cells).sum();
    offset = room.saturating_sub(right).max(offset);
    for (item, &cells) in fitted(EolAlign::Right) {
        placed.push(EolPlaced {
            source: item.source,
            offset,
            text: truncate(item.text, cells),
        });
        offset = offset.saturating_add(cells);
    }
    placed
}

/// The cells of `area`'s width a row's `spans` (gutter included) leave free:
/// the room its end-of-line slot has.
pub(super) fn row_room(area: Rect, spans: &[RowSpan<'_>]) -> u32 {
    let used = spans
        .iter()
        .fold(0_usize, |used, span| used.saturating_add(span.width()));
    u32::try_from(usize::from(area.width).saturating_sub(used)).unwrap_or(0)
}

/// Record where the row at `y` paints virtual text: `painted` pairs each
/// text's index with the position of its span in `spans`. Measured from the
/// spans themselves, so the rect is exactly the cells the text went to,
/// clipped to `area`.
pub(super) fn record_virtual_text(
    rects: &mut Vec<(usize, Rect)>,
    area: Rect,
    y: u16,
    spans: &[RowSpan<'_>],
    painted: &[(usize, usize)],
) {
    for &(index, position) in painted {
        let (Some(before), Some(span)) = (spans.get(..position), spans.get(position)) else {
            continue;
        };
        let x = before
            .iter()
            .fold(0_usize, |x, span| x.saturating_add(span.width()));
        let start = u16::try_from(x).unwrap_or(u16::MAX);
        let width = u16::try_from(span.width()).unwrap_or(u16::MAX);
        let visible = width.min(area.width.saturating_sub(start));
        if visible > 0 {
            rects.push((
                index,
                Rect::new(area.x.saturating_add(start), y, visible, 1),
            ));
        }
    }
}

/// `text` cut to at most `cells` screen cells, ending in `…` when it had to be
/// cut. A wide character that would straddle the cut is dropped whole.
fn truncate(text: &str, cells: u32) -> String {
    if text_width(text) <= cells {
        return text.to_owned();
    }
    let budget = cells.saturating_sub(1);
    let mut used = 0_u32;
    let mut out = String::new();
    for ch in text.chars() {
        let width =
            u32::try_from(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(0);
        if used.saturating_add(width) > budget {
            break;
        }
        used = used.saturating_add(width);
        out.push(ch);
    }
    if cells > 0 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use karet_core::ThemeRole;

    use super::*;

    fn inline(line: u32, col: u32, label: &str) -> VirtualText {
        VirtualText::inline(line, col, label, ThemeRole::InlayHint)
    }

    fn item(source: usize, text: &str, align: EolAlign, priority: u8) -> EolItem<'_> {
        EolItem {
            source,
            text,
            align,
            priority,
        }
    }

    fn placed(source: usize, offset: u32, text: &str) -> EolPlaced {
        EolPlaced {
            source,
            offset,
            text: text.to_owned(),
        }
    }

    #[test]
    fn an_empty_index_reports_nothing_on_every_line() {
        let index = HintIndex::new(&[]);
        assert!(index.line(0).is_empty());
        assert_eq!(index.line(0).width_at(0), 0);
        assert_eq!(index.eol_on(0).count(), 0);
    }

    #[test]
    fn inline_text_groups_by_line_whatever_order_it_arrives_in() {
        let index = HintIndex::new(&[
            inline(4, 2, "b"),
            inline(0, 9, ": i32"),
            inline(4, 0, "a"),
            inline(2, 1, "mid"),
        ]);
        assert_eq!(index.line(0).width_at(9), 5);
        assert!(index.line(1).is_empty());
        assert_eq!(index.line(2).width_at(1), 3);
        // Both of line 4's, each found at its own column.
        assert_eq!(index.line(4).width_at(0), 1);
        assert_eq!(index.line(4).width_at(2), 1);
    }

    #[test]
    fn inline_text_is_found_only_at_the_column_it_annotates() {
        // Found only at its own anchor, never at its neighbours: the mappings
        // decide which side of the caret it falls on, not the index.
        let index = HintIndex::new(&[inline(0, 9, ": i32")]);
        let line = index.line(0);
        assert_eq!(line.width_at(9), 5);
        assert_eq!(line.width_at(8), 0);
        assert_eq!(line.width_at(10), 0);
    }

    #[test]
    fn text_that_would_render_as_nothing_is_dropped() {
        // A zero-width span is a mapping boundary where nothing shifts, which
        // would make `source_col_at_display_offset` able to return a column
        // that occupies no cells.
        let index = HintIndex::new(&[
            inline(0, 3, ""),
            VirtualText::end_of_line(0, "", ThemeRole::Muted, 0),
        ]);
        assert!(index.line(0).is_empty());
        assert_eq!(index.line(0).width_at(3), 0);
        assert_eq!(index.eol_on(0).count(), 0);
    }

    #[test]
    fn a_wide_label_counts_its_display_width_not_its_bytes() {
        let index = HintIndex::new(&[inline(0, 0, "日本")]);
        assert_eq!(index.line(0).width_at(0), 4);
    }

    #[test]
    fn two_texts_at_one_column_keep_the_order_they_arrived_in() {
        let index = HintIndex::new(&[inline(0, 2, "first"), inline(0, 2, "second")]);
        let line = index.line(0);
        assert_eq!(line.width_at(2), 11);
        assert_eq!(line.at(2).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn end_of_line_text_stays_out_of_the_layout_and_is_found_by_line() {
        let index = HintIndex::new(&[
            VirtualText::end_of_line(3, "b", ThemeRole::Muted, 0),
            inline(3, 0, "x"),
            VirtualText::end_of_line(1, "a", ThemeRole::Muted, 0),
            VirtualText::end_of_line(3, "c", ThemeRole::Muted, 0),
        ]);
        // Only the inline text shifts line 3's mapping.
        assert_eq!(index.line(3).width_at(0), 1);
        assert_eq!(index.line(1).width_at(0), 0);
        assert_eq!(index.eol_on(3).collect::<Vec<_>>(), vec![0, 3]);
        assert_eq!(index.eol_on(1).collect::<Vec<_>>(), vec![2]);
        assert_eq!(index.eol_on(2).count(), 0);
    }

    #[test]
    fn everything_fits_and_packs_left_then_flush_right() {
        let items = [
            item(0, "  Ada", EolAlign::Left, 0),
            item(1, "R", EolAlign::Right, 0),
            item(2, " v1", EolAlign::Left, 0),
        ];
        assert_eq!(
            place_eol(&items, 12),
            vec![
                placed(0, 0, "  Ada"),
                placed(2, 5, " v1"),
                placed(1, 11, "R")
            ]
        );
    }

    #[test]
    fn a_narrow_slot_cuts_the_lowest_priority_first() {
        let items = [
            item(0, "  blame", EolAlign::Left, 0),
            item(1, "  ✗ 0.3.55", EolAlign::Left, 5),
        ];
        // The higher priority keeps all ten cells; the blame gets the last
        // three, cut short.
        assert_eq!(
            place_eol(&items, 13),
            vec![placed(0, 0, "  …"), placed(1, 3, "  ✗ 0.3.55")]
        );
        // With no cells left over it is left out altogether.
        assert_eq!(place_eol(&items, 10), vec![placed(1, 0, "  ✗ 0.3.55")]);
    }

    #[test]
    fn equal_priorities_are_cut_last_supplied_first() {
        let items = [
            item(0, "abcd", EolAlign::Left, 1),
            item(1, "efgh", EolAlign::Left, 1),
        ];
        assert_eq!(
            place_eol(&items, 6),
            vec![placed(0, 0, "abcd"), placed(1, 4, "e…")]
        );
    }

    #[test]
    fn a_full_slot_puts_right_aligned_text_straight_after_the_left() {
        let items = [
            item(0, "left", EolAlign::Left, 0),
            item(1, "right", EolAlign::Right, 9),
        ];
        // `right` outranks `left`, so `left` keeps the two cells it leaves,
        // and `right` sits flush against both its neighbour and the edge.
        assert_eq!(
            place_eol(&items, 7),
            vec![placed(0, 0, "l…"), placed(1, 2, "right")]
        );
    }

    #[test]
    fn no_room_places_nothing() {
        assert_eq!(
            place_eol(&[item(0, "x", EolAlign::Right, 0)], 0),
            Vec::new()
        );
    }

    #[test]
    fn truncation_never_splits_a_wide_character() {
        // Four cells for `日本語`: `日` and the ellipsis, since `本` would
        // straddle the cut.
        assert_eq!(truncate("日本語", 4), "日…");
        assert_eq!(truncate("日本語", 1), "…");
        assert_eq!(truncate("日本語", 0), "");
        assert_eq!(truncate("ab", 2), "ab");
    }
}
