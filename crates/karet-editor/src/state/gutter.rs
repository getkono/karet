//! The gutter as pointer targets: which lane a cell lies in and the line its
//! row shows, the collapsed-fold `⋯` a render painted, and the whole-line
//! selection a line-number click makes.

use super::*;

/// One of the gutter's columns, left to right (see
/// [`EditorState::gutter_hit_at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GutterLane {
    /// The leading marker column: breakpoints and diagnostic signs are drawn
    /// here (and change markers, when no [`Change`](Self::Change) lane is
    /// reserved); a click is the breakpoint toggle.
    Marker,
    /// The line numbers. Absent when [`LineNumbers::Off`] hides them.
    LineNumber,
    /// The change column after the line numbers, where
    /// [`DecorationKind::ChangeMarker`](karet_core::DecorationKind::ChangeMarker)s
    /// are drawn. Present only when [`Editor::change_lane`](crate::Editor::change_lane)
    /// reserves it.
    Change,
    /// The trailing column next to the text, where a fold header draws its
    /// collapse/expand chevron.
    Fold,
}

/// A gutter cell under the pointer: its lane and the buffer line its row
/// shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GutterHit {
    /// The lane the cell lies in.
    pub lane: GutterLane,
    /// The 0-based buffer line of the cell's row.
    pub line: u32,
}

impl EditorState {
    /// The gutter lane and buffer line under the screen cell `(col, row)`,
    /// given the editor's render `area` and the `folds` in effect.
    ///
    /// `None` when the cell is outside the gutter, or on a row below the
    /// document's last row, which shows no line. Rows resolve as
    /// [`pos_at`](Self::pos_at) resolves them, so folds, word wrap and sticky
    /// headers agree with the text beside them; a soft-wrap continuation row
    /// belongs to the line it continues.
    #[must_use]
    pub fn gutter_hit_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> Option<GutterHit> {
        if col < area.x || row < area.y || row >= area.bottom() {
            return None;
        }
        let line_count = buffer.line_count().max(1) as u32;
        let gutter = gutter_width(line_count, self.last_line_numbers, self.last_change_lane);
        let offset = col - area.x;
        if offset >= gutter.min(area.width) {
            return None;
        }
        let lane = if offset == 0 {
            GutterLane::Marker
        } else if offset + 1 == gutter {
            GutterLane::Fold
        } else if self.last_change_lane && offset + 2 == gutter {
            GutterLane::Change
        } else {
            GutterLane::LineNumber
        };
        let mut rel_row = u32::from(row - area.y);
        let sticky = u32::from(self.sticky_height);
        let line = if rel_row < sticky {
            self.sticky_rows
                .get(rel_row as usize)
                .copied()
                .unwrap_or(self.scroll_line)
                .min(line_count - 1)
        } else if self.last_word_wrap {
            rel_row -= sticky;
            let width = u32::from(area.width.saturating_sub(gutter).max(1));
            let top = VisualAnchor {
                line: self.scroll_line,
                subrow: self.scroll_subrow,
            };
            let (anchor, below_end) =
                visual_anchor_at_row(buffer, folds, self.layout(width), top, rel_row);
            (!below_end).then_some(anchor.line)?
        } else {
            self.unwrapped_row_line(line_count, folds, rel_row - sticky)
                .ok()?
        };
        Some(GutterHit { lane, line })
    }

    /// The buffer line under `(col, row)` when the cell lies in the gutter's
    /// leading marker column (the breakpoint lane); `None` anywhere else,
    /// including below the document's last row. The
    /// [`GutterLane::Marker`] case of [`gutter_hit_at`](Self::gutter_hit_at).
    #[must_use]
    pub fn gutter_marker_line_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> Option<u32> {
        self.gutter_hit_at(area, buffer, folds, col, row)
            .filter(|hit| hit.lane == GutterLane::Marker)
            .map(|hit| hit.line)
    }

    /// The header line of the collapsed fold whose ` ⋯` the last render
    /// painted over the screen cell `(col, row)`, if any.
    #[must_use]
    pub fn fold_ellipsis_at(&self, col: u16, row: u16) -> Option<u32> {
        self.last_fold_ellipses
            .iter()
            .find(|(rect, _)| rect.contains(ratatui::layout::Position { x: col, y: row }))
            .map(|&(_, line)| line)
    }

    /// Select whole lines from `from` through `to`, line breaks included,
    /// with the caret at the `to` end: after `to`'s line break when `to` is at
    /// or below `from`, at the start of `to` when above it. Both lines are
    /// clamped to the buffer, and every other caret is dropped.
    ///
    /// This is the line-number click (and Shift+click), made on a row the
    /// user can see, so it never scrolls — not even when the line break it
    /// selects ends on the row below the viewport.
    pub fn select_lines(&mut self, buffer: &TextBuffer, from: u32, to: u32) {
        let last = last_line(buffer);
        let (from, to) = (from.min(last), to.min(last));
        let (from_start, from_end) = line_span(buffer, from);
        let (to_start, to_end) = line_span(buffer, to);
        let (anchor, head) = if to >= from {
            (from_start, to_end)
        } else {
            (from_end, to_start)
        };
        self.set_single(Selection { anchor, head });
        self.upstream.clear();
        self.follow_cursor = false;
        self.follow_col = None;
    }
}
