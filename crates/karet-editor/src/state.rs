use std::ops::RangeInclusive;

use super::text::*;
use super::visual::*;
use super::*;

/// A fold region resolved for rendering: an inclusive line range plus whether it is
/// currently collapsed. When collapsed, the interior lines `start + 1 ..= end` are
/// hidden and the `start` line shows a fold marker and a `⋯` affordance.
///
/// The application resolves these from `karet_syntax::FoldRegions` plus its own
/// per-view "which folds are collapsed" set, keeping fold *policy* out of the widget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    /// The 0-based header line (always visible; carries the fold marker).
    pub start: u32,
    /// The 0-based last line of the region, inclusive.
    pub end: u32,
    /// Whether the region is currently collapsed.
    pub collapsed: bool,
}

/// The persistent, per-view editor state: scroll position and the cursor/selection
/// set.
///
/// Cursors live in a [`CursorState`] with the invariant that it always holds at least
/// one selection; single-cursor editing is simply `selections.len() == 1` and remains
/// the common fast path, while secondary carets are additive (multi-cursor).
///
/// The viewport height is cached at each render so motions and
/// [`scroll_to`](Self::scroll_to) know how far a page is without re-deriving it.
#[derive(Clone, Debug)]
pub struct EditorState {
    /// The first visible buffer line (top of the viewport).
    pub scroll_line: u32,
    /// The first visible column (horizontal scroll, counted in `char`s).
    pub scroll_col: u32,
    /// The cursor/selection set (never empty). The moving end of each selection is its
    /// `head`; the primary selection's head is the main caret.
    pub(super) cursors: CursorState,
    /// The viewport height captured at the last render.
    pub(super) last_height: u16,
    /// The continuation row within [`scroll_line`](Self::scroll_line) at the top of
    /// a soft-wrapped viewport.
    pub(super) scroll_subrow: u32,
    /// The content width (after the gutter) captured at the last render.
    pub(super) last_content_width: u16,
    /// Whether the last render used soft wrapping.
    pub(super) last_word_wrap: bool,
    /// Hard-tab width captured at the last render.
    pub(super) last_tab_width: u16,
    /// Logical-line ranges exempted from soft wrapping at the last render.
    pub(super) last_unwrapped_lines: Vec<RangeInclusive<u32>>,
    /// Whether the next wrapped render should reveal a cursor moved by an editor
    /// command rather than preserve a manually-scrolled viewport.
    pub(super) follow_cursor: bool,
    /// Source lines currently occupying the sticky-scroll rows, outermost first.
    pub(super) sticky_rows: Vec<u32>,
    /// Rows reserved above the live document viewport by the last render.
    pub(super) sticky_height: u16,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            scroll_line: 0,
            scroll_col: 0,
            cursors: CursorState::single(Selection::caret(LineCol::new(0, 0))),
            last_height: 0,
            scroll_subrow: 0,
            last_content_width: 0,
            last_word_wrap: false,
            last_tab_width: 4,
            last_unwrapped_lines: Vec::new(),
            follow_cursor: false,
            sticky_rows: Vec::new(),
            sticky_height: 0,
        }
    }
}

impl EditorState {
    /// Create a fresh editor state scrolled to the top.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The primary caret (the moving head of the primary selection).
    #[must_use]
    pub fn cursor(&self) -> LineCol {
        self.cursors.primary().head
    }

    /// The full cursor/selection set, for rendering every caret and selection.
    #[must_use]
    pub fn cursors(&self) -> &CursorState {
        &self.cursors
    }

    /// Whether more than one caret is currently active.
    #[must_use]
    pub fn has_multiple_cursors(&self) -> bool {
        self.cursors.selections.len() > 1
    }

    /// Scroll vertically so that `pos` is within the viewport.
    pub fn scroll_to(&mut self, pos: LineCol) {
        self.follow_cursor = true;
        let height = u32::from(self.last_height.max(1));
        if pos.line < self.scroll_line {
            self.scroll_line = pos.line;
            self.scroll_subrow = 0;
        } else if pos.line >= self.scroll_line + height {
            self.scroll_line = pos.line + 1 - height;
            self.scroll_subrow = 0;
        }

        if !self.last_word_wrap && self.last_content_width > 0 {
            let width = u32::from(self.last_content_width);
            let margin = 10_u32.min(width.saturating_sub(1) / 2);
            let left_guard = self.scroll_col.saturating_add(margin);
            let right_guard = self.scroll_col.saturating_add(width.saturating_sub(margin));
            if pos.col < left_guard {
                self.scroll_col = pos.col.saturating_sub(margin);
            } else if pos.col >= right_guard {
                self.scroll_col = pos
                    .col
                    .saturating_add(margin)
                    .saturating_add(1)
                    .saturating_sub(width);
            }
        }
    }

    /// Scroll the viewport vertically by display rows.
    ///
    /// In overflow mode a display row is one buffer line. In soft-wrap mode this
    /// walks continuation rows before advancing to the next visible buffer line.
    pub fn scroll_rows(
        &mut self,
        buffer: &TextBuffer,
        folds: &[Fold],
        word_wrap: bool,
        delta: i32,
    ) {
        self.follow_cursor = false;
        let width = u32::from(self.last_content_width.max(1));
        if !word_wrap {
            let max = i64::from(last_line(buffer));
            self.scroll_line =
                (i64::from(self.scroll_line) + i64::from(delta)).clamp(0, max) as u32;
            self.scroll_subrow = 0;
            return;
        }
        let mut anchor = VisualAnchor {
            line: self.scroll_line.min(last_line(buffer)),
            subrow: self.scroll_subrow,
        };
        let steps = delta.unsigned_abs();
        for _ in 0..steps {
            anchor = if delta.is_negative() {
                previous_visual_anchor(
                    buffer,
                    folds,
                    width,
                    self.last_tab_width,
                    &self.last_unwrapped_lines,
                    anchor,
                )
            } else {
                next_visual_anchor(
                    buffer,
                    folds,
                    width,
                    self.last_tab_width,
                    &self.last_unwrapped_lines,
                    anchor,
                )
            };
        }
        self.scroll_line = anchor.line;
        self.scroll_subrow = anchor.subrow;
    }

    /// Scroll an overflow-mode viewport horizontally without moving the caret.
    /// Soft-wrapped views always remain at column zero.
    pub fn scroll_columns(&mut self, buffer: &TextBuffer, delta: i32) {
        self.follow_cursor = false;
        if self.last_word_wrap {
            self.scroll_col = 0;
            return;
        }
        let longest = (0..buffer.line_count())
            .map(|line| line_len(buffer, line as u32))
            .max()
            .unwrap_or(0);
        let width = u32::from(self.last_content_width.max(1));
        let max = longest.saturating_add(1).saturating_sub(width);
        self.scroll_col =
            (i64::from(self.scroll_col) + i64::from(delta)).clamp(0, i64::from(max)) as u32;
    }

    /// The currently-visible line range `[top, top + height)`.
    #[must_use]
    pub fn viewport(&self) -> Range {
        let height = u32::from(self.last_height.max(1));
        Range {
            start: LineCol::new(self.scroll_line, 0),
            end: LineCol::new(self.scroll_line + height, 0),
        }
    }

    /// The screen cell of the primary caret within `area`, if it is visible.
    ///
    /// This mirrors the widget's own caret placement, including the line-number
    /// gutter, horizontal scroll, and collapsed folds. Applications that render a
    /// terminal-native caret outside the ratatui buffer can use this after a frame's
    /// layout has recorded the editor area.
    #[must_use]
    pub fn primary_caret_cell(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
    ) -> Option<(u16, u16)> {
        caret_cell(area, buffer, folds, self, self.cursor())
    }

    /// The screen cell of an arbitrary buffer position within `area`, if visible.
    ///
    /// This uses the same gutter, scrolling, wrapping, sticky-row, and fold geometry
    /// as the editor widget. It is useful for positioning application-owned inline
    /// affordances without changing document coordinates.
    #[must_use]
    pub fn screen_cell(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        position: LineCol,
    ) -> Option<(u16, u16)> {
        caret_cell(area, buffer, folds, self, position)
    }

    /// Scroll so `line` sits at the vertical center of the viewport. Handy for a
    /// read-only viewer centering a search match. The scroll is clamped to the
    /// buffer at render time.
    pub fn center_on(&mut self, line: u32) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = line.saturating_sub(height / 2);
    }

    /// Scroll the viewport down one page **without moving the cursor** — read-only
    /// paging for a pager/viewer. The scroll is clamped to the buffer at render
    /// time.
    pub fn scroll_page_down(&mut self) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = self.scroll_line.saturating_add(height);
    }

    /// Scroll the viewport up one page without moving the cursor (read-only paging).
    pub fn scroll_page_up(&mut self) {
        let height = u32::from(self.last_height.max(1));
        self.scroll_line = self.scroll_line.saturating_sub(height);
    }

    /// The primary selection as a normalized range, or `None` when it is a bare
    /// caret. This reports the *primary* selection, matching the pre-multi-cursor API.
    #[must_use]
    pub fn selection_range(&self) -> Option<Range> {
        let p = self.cursors.primary();
        (!p.is_empty()).then(|| p.range())
    }

    /// Every non-empty selection as a normalized range, in the order stored, for
    /// painting all selections.
    #[must_use]
    pub fn selection_ranges(&self) -> Vec<Range> {
        self.cursors
            .selections
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.range())
            .collect()
    }

    /// Move every caret's head with `motion`, then merge coincident carets and keep
    /// the primary head in view. This is how multi-caret motions stay consistent.
    fn map_heads(&mut self, motion: impl Fn(LineCol) -> LineCol) {
        for s in &mut self.cursors.selections {
            s.head = motion(s.head);
        }
        self.after_motion();
    }

    /// Normalize the cursor set after a motion and scroll to the primary head.
    fn after_motion(&mut self) {
        self.cursors.normalize();
        let head = self.cursor();
        self.scroll_to(head);
    }

    /// Move every caret down one line, clamping to the buffer and keeping the primary
    /// in view.
    pub fn move_down(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| {
            let line = (h.line + 1).min(last_line(buffer));
            LineCol::new(line, h.col.min(line_len(buffer, line)))
        });
    }

    /// Move every caret up one line.
    pub fn move_up(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| {
            let line = h.line.saturating_sub(1);
            LineCol::new(line, h.col.min(line_len(buffer, line)))
        });
    }

    /// Move every caret left one column, wrapping to the previous line's end.
    pub fn move_left(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| {
            if h.col > 0 {
                LineCol::new(h.line, h.col - 1)
            } else if h.line > 0 {
                let line = h.line - 1;
                LineCol::new(line, line_len(buffer, line))
            } else {
                h
            }
        });
    }

    /// Move every caret right one column, wrapping to the next line's start.
    pub fn move_right(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| {
            if h.col < line_len(buffer, h.line) {
                LineCol::new(h.line, h.col + 1)
            } else if h.line < last_line(buffer) {
                LineCol::new(h.line + 1, 0)
            } else {
                h
            }
        });
    }

    /// Move every caret down one page.
    pub fn page_down(&mut self, buffer: &TextBuffer) {
        let height = u32::from(self.last_height.max(1));
        self.map_heads(|h| {
            let line = (h.line + height).min(last_line(buffer));
            LineCol::new(line, h.col.min(line_len(buffer, line)))
        });
    }

    /// Move every caret up one page.
    pub fn page_up(&mut self, buffer: &TextBuffer) {
        let height = u32::from(self.last_height.max(1));
        self.map_heads(|h| {
            let line = h.line.saturating_sub(height);
            LineCol::new(line, h.col.min(line_len(buffer, line)))
        });
    }

    /// Move every caret to the start of its line (column 0).
    pub fn move_line_start(&mut self, _buffer: &TextBuffer) {
        self.map_heads(|h| LineCol::new(h.line, 0));
    }

    /// Move every caret to the end of its line.
    pub fn move_line_end(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| LineCol::new(h.line, line_len(buffer, h.line)));
    }

    /// Move every caret to the start of the document.
    pub fn move_doc_start(&mut self, _buffer: &TextBuffer) {
        self.map_heads(|_| LineCol::new(0, 0));
    }

    /// Move every caret to the end of the document.
    pub fn move_doc_end(&mut self, buffer: &TextBuffer) {
        let last = last_line(buffer);
        let end = LineCol::new(last, line_len(buffer, last));
        self.map_heads(move |_| end);
    }

    /// Move every caret to the start of the previous word (wrapping across lines).
    pub fn move_word_left(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| prev_word_boundary(buffer, h));
    }

    /// Move every caret to the end of the next word (wrapping across lines).
    pub fn move_word_right(&mut self, buffer: &TextBuffer) {
        self.map_heads(|h| next_word_boundary(buffer, h));
    }

    /// Select the entire buffer as a single selection, caret at the end (Ctrl+A).
    pub fn select_all(&mut self, buffer: &TextBuffer) {
        let last = last_line(buffer);
        let end = LineCol::new(last, line_len(buffer, last));
        self.cursors = CursorState::single(Selection {
            anchor: LineCol::new(0, 0),
            head: end,
        });
        self.scroll_to(end);
    }

    /// Jump the caret to `pos` (clamped), collapsing to a single bare caret there.
    /// Used to place the caret at a target (search match, go-to-line).
    pub fn goto(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        self.cursors = CursorState::single(Selection::caret(p));
        self.scroll_to(p);
    }

    /// Collapse every selection to a bare caret at its head (a non-extending motion).
    pub fn clear_selection(&mut self) {
        for s in &mut self.cursors.selections {
            s.anchor = s.head;
        }
        self.cursors.normalize();
    }

    /// Place the caret at `pos` (clamped), collapsing to a single bare caret there and
    /// clearing any selection or secondary carets.
    pub fn set_caret(&mut self, buffer: &TextBuffer, pos: LineCol) {
        self.goto(buffer, pos);
    }

    /// Replace the cursor set with a single bare caret at exactly `pos` (no clamping);
    /// used after applying an edit and by tests.
    pub fn place_caret(&mut self, pos: LineCol) {
        self.cursors = CursorState::single(Selection::caret(pos));
    }

    /// Replace the cursor set with a bare caret at each `position` (post-edit),
    /// merging any that coincide and keeping the primary index in range. A no-op when
    /// `positions` is empty (the invariant of a non-empty set is preserved).
    pub fn set_carets(&mut self, positions: &[LineCol]) {
        if positions.is_empty() {
            return;
        }
        let selections: Vec<Selection> = positions.iter().copied().map(Selection::caret).collect();
        let primary = self.cursors.primary.min(selections.len() - 1);
        self.cursors = CursorState {
            selections,
            primary,
        };
        self.cursors.normalize();
    }

    /// Restore a complete cursor/selection set, clamping every endpoint to `buffer`.
    /// An empty set becomes one caret at the origin.
    pub fn set_cursor_state(&mut self, buffer: &TextBuffer, mut cursors: CursorState) {
        if cursors.selections.is_empty() {
            cursors = CursorState::single(Selection::caret(LineCol::new(0, 0)));
        }
        for selection in &mut cursors.selections {
            selection.anchor = clamp_to_buffer(buffer, selection.anchor);
            selection.head = clamp_to_buffer(buffer, selection.head);
        }
        cursors.primary = cursors
            .primary
            .min(cursors.selections.len().saturating_sub(1));
        cursors.normalize();
        let head = cursors.primary().head;
        self.cursors = cursors;
        self.scroll_to(head);
    }

    /// Extend the primary selection so its moving end is `pos` (clamped), keeping the
    /// primary anchor fixed and leaving any secondary carets in place.
    pub fn extend_to(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        if let Some(s) = self.cursors.selections.get_mut(self.cursors.primary) {
            s.head = p;
        }
        self.after_motion();
    }

    /// Collapse to a single selection spanning `anchor`..`head` (both clamped), with
    /// the caret at `head`. Used by double/triple-click word/line selection.
    pub fn set_selection(&mut self, buffer: &TextBuffer, anchor: LineCol, head: LineCol) {
        let anchor = clamp_to_buffer(buffer, anchor);
        let head = clamp_to_buffer(buffer, head);
        self.cursors = CursorState::single(Selection { anchor, head });
        self.scroll_to(head);
    }

    /// Collapse the cursor set to just the primary selection (the `Esc` fold-back).
    pub fn collapse_to_primary(&mut self) {
        self.cursors.collapse_to_primary();
    }

    /// Add a bare caret one line above the primary (column clamped to the shorter
    /// line), merging if it collides. A no-op when the primary is already on line 0.
    pub fn add_caret_above(&mut self, buffer: &TextBuffer) {
        let h = self.cursor();
        if h.line == 0 {
            return;
        }
        let p = clamp_to_buffer(buffer, LineCol::new(h.line - 1, h.col));
        self.cursors.push(Selection::caret(p));
        self.scroll_to(self.cursor());
    }

    /// Add a bare caret one line below the primary. A no-op on the last line.
    pub fn add_caret_below(&mut self, buffer: &TextBuffer) {
        let h = self.cursor();
        if h.line >= last_line(buffer) {
            return;
        }
        let p = clamp_to_buffer(buffer, LineCol::new(h.line + 1, h.col));
        self.cursors.push(Selection::caret(p));
        self.scroll_to(self.cursor());
    }

    /// Toggle a caret at `pos` (Alt+click): remove a coincident bare caret unless it is
    /// the only one, otherwise add one as the new primary.
    pub fn add_caret(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        if let Some(i) = self
            .cursors
            .selections
            .iter()
            .position(|s| s.is_empty() && s.head == p)
            && self.cursors.selections.len() > 1
        {
            self.cursors.selections.remove(i);
            self.cursors.primary = self.cursors.primary.min(self.cursors.selections.len() - 1);
            return;
        }
        self.cursors.push(Selection::caret(p));
        self.scroll_to(p);
    }

    /// `Ctrl+D`: if the primary is a bare caret, select the word under it; otherwise
    /// add a caret selecting the next occurrence (wrapping) of the primary selection's
    /// text.
    pub fn add_next_occurrence(&mut self, buffer: &TextBuffer) {
        let primary = self.cursors.primary();
        if primary.is_empty() {
            let (anchor, head) = word_bounds(buffer, primary.head);
            if let Some(s) = self.cursors.selections.get_mut(self.cursors.primary) {
                *s = Selection { anchor, head };
            }
            self.scroll_to(self.cursor());
            return;
        }
        let Some(needle) = slice_text(buffer, primary.range()) else {
            return;
        };
        if needle.is_empty() {
            return;
        }
        let hay = buffer.text();
        let from = self
            .cursors
            .selections
            .iter()
            .filter_map(|s| buffer.line_col_to_byte(s.range().end).ok())
            .map(|b| b.0)
            .max()
            .unwrap_or(0);
        if let Some(byte) = find_next(&hay, &needle, from) {
            let start = buffer.byte_to_line_col(BytePos(byte));
            let end = buffer.byte_to_line_col(BytePos(byte + needle.len()));
            self.cursors.push(Selection {
                anchor: start,
                head: end,
            });
            self.scroll_to(self.cursor());
        }
    }

    /// The buffer position under the screen cell `(col, row)`, given the editor's
    /// render `area` and the `folds` in effect. Accounts for the gutter width, the
    /// scroll offsets, and any collapsed folds that hide lines between the viewport
    /// top and the click.
    #[must_use]
    pub fn pos_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> LineCol {
        let line_count = buffer.line_count().max(1) as u32;
        let mut rel_row = u32::from(row.saturating_sub(area.y));
        let gutter = 1 + digit_count(line_count) as u16 + 1;
        let content_x = area.x.saturating_add(gutter);
        let rel_col = u32::from(col.saturating_sub(content_x));
        if rel_row < u32::from(self.sticky_height) {
            let line = self
                .sticky_rows
                .get(rel_row as usize)
                .copied()
                .unwrap_or(self.scroll_line)
                .min(line_count - 1);
            let want = self.scroll_col.saturating_add(rel_col);
            return LineCol::new(line, want.min(line_len(buffer, line)));
        }
        rel_row = rel_row.saturating_sub(u32::from(self.sticky_height));
        if self.last_word_wrap {
            let width = u32::from(area.width.saturating_sub(gutter).max(1));
            let anchor = visual_anchor_at_row(
                buffer,
                folds,
                width,
                self.last_tab_width,
                &self.last_unwrapped_lines,
                VisualAnchor {
                    line: self.scroll_line,
                    subrow: self.scroll_subrow,
                },
                rel_row,
            );
            let ranges = visual_ranges(
                buffer,
                anchor.line,
                width,
                self.last_tab_width,
                &self.last_unwrapped_lines,
            );
            let range = ranges
                .get(anchor.subrow as usize)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(line_len(buffer, anchor.line)));
            let chars: Vec<char> = buffer
                .line(anchor.line as usize)
                .unwrap_or_default()
                .chars()
                .collect();
            return LineCol::new(
                anchor.line,
                source_col_at_display_offset(
                    &chars,
                    range.start,
                    range.end,
                    rel_col,
                    self.last_tab_width,
                ),
            );
        }
        // Walk visible lines from the (clamped) viewport top to the clicked row.
        let mut line = self.scroll_line;
        while line < line_count && hidden_in(folds, line) {
            line += 1;
        }
        for _ in 0..rel_row {
            let mut next = line + 1;
            while next < line_count && hidden_in(folds, next) {
                next += 1;
            }
            if next >= line_count {
                break;
            }
            line = next;
        }
        let line = line.min(line_count - 1);
        let chars: Vec<char> = buffer
            .line(line as usize)
            .unwrap_or_default()
            .chars()
            .collect();
        LineCol::new(
            line,
            source_col_at_display_offset(
                &chars,
                self.scroll_col,
                chars.len() as u32,
                rel_col,
                self.last_tab_width,
            ),
        )
    }
}
