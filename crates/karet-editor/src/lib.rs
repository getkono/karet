//! `karet-editor` — the composable code-editor widget for karet.
//!
//! Combines the text engines (`karet-text`, `karet-syntax`, `karet-theme`) into a
//! ratatui editor widget. By design it depends on **none** of the feature
//! producers (`karet-lsp`/`karet-vcs`/`karet-dap`/`karet-search`): diagnostics,
//! git markers, breakpoints, inlay hints and code lenses arrive as `karet-core`
//! decorations supplied by the application from the backend's event stream.
//!
//! This is the implementation *skeleton*: the [`Editor`] builder (the data joint)
//! and [`EditorState`] are defined; the painting/input logic is filled in
//! separately.

use karet_core::BytePos;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::DecorationKind;
use karet_core::Diagnostic;
use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::ThemeRole;
use karet_core::TokenId;
use karet_syntax::HighlightSpan;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use karet_theme::Rgba;
use karet_theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::StatefulWidget;

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
    cursors: CursorState,
    /// The viewport height captured at the last render.
    last_height: u16,
    /// The continuation row within [`scroll_line`](Self::scroll_line) at the top of
    /// a soft-wrapped viewport.
    scroll_subrow: u32,
    /// The content width (after the gutter) captured at the last render.
    last_content_width: u16,
    /// Whether the last render used soft wrapping.
    last_word_wrap: bool,
    /// Whether the next wrapped render should reveal a cursor moved by an editor
    /// command rather than preserve a manually-scrolled viewport.
    follow_cursor: bool,
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
            follow_cursor: false,
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
                previous_visual_anchor(buffer, folds, width, anchor)
            } else {
                next_visual_anchor(buffer, folds, width, anchor)
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
        let rel_row = u32::from(row.saturating_sub(area.y));
        let gutter = 1 + digit_count(line_count) as u16 + 1;
        let content_x = area.x.saturating_add(gutter);
        let rel_col = u32::from(col.saturating_sub(content_x));
        if self.last_word_wrap {
            let width = u32::from(area.width.saturating_sub(gutter).max(1));
            let anchor = visual_anchor_at_row(
                buffer,
                folds,
                width,
                VisualAnchor {
                    line: self.scroll_line,
                    subrow: self.scroll_subrow,
                },
                rel_row,
            );
            let ranges = visual_ranges(buffer, anchor.line, width);
            let range = ranges
                .get(anchor.subrow as usize)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(line_len(buffer, anchor.line)));
            return LineCol::new(
                anchor.line,
                range.start.saturating_add(rel_col).min(range.end),
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
        let want = self.scroll_col + rel_col;
        LineCol::new(line, want.min(line_len(buffer, line)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct VisualAnchor {
    line: u32,
    subrow: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VisualRange {
    start: u32,
    end: u32,
}

impl VisualRange {
    const fn empty(at: u32) -> Self {
        Self { start: at, end: at }
    }
}

/// Split one logical line into source-column ranges for soft wrapping. Whitespace is
/// kept in the range before the break so every source column maps to exactly one row;
/// words wider than the viewport are split at the hard width.
fn visual_ranges(buffer: &TextBuffer, line: u32, width: u32) -> Vec<VisualRange> {
    let chars: Vec<char> = buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let len = chars.len() as u32;
    if len == 0 {
        return vec![VisualRange::empty(0)];
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0_u32;
    while start < len {
        let hard_end = start.saturating_add(width).min(len);
        let end = if hard_end < len {
            chars[start as usize..hard_end as usize]
                .iter()
                .rposition(|ch| ch.is_whitespace())
                .map_or(hard_end, |index| start + index as u32 + 1)
        } else {
            hard_end
        };
        let end = end.max(start.saturating_add(1)).min(len);
        ranges.push(VisualRange { start, end });
        start = end;
    }
    ranges
}

fn normalize_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let last = last_line(buffer);
    let mut line = anchor.line.min(last);
    while line < last && hidden_in(folds, line) {
        line += 1;
    }
    while line > 0 && hidden_in(folds, line) {
        line -= 1;
    }
    let rows = visual_ranges(buffer, line, width).len().max(1) as u32;
    VisualAnchor {
        line,
        subrow: anchor.subrow.min(rows - 1),
    }
}

fn next_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, anchor);
    let rows = visual_ranges(buffer, anchor.line, width).len() as u32;
    if anchor.subrow + 1 < rows {
        return VisualAnchor {
            subrow: anchor.subrow + 1,
            ..anchor
        };
    }
    let last = last_line(buffer);
    let mut line = anchor.line.saturating_add(1);
    while line <= last && hidden_in(folds, line) {
        line += 1;
    }
    if line > last {
        anchor
    } else {
        VisualAnchor { line, subrow: 0 }
    }
}

fn previous_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, anchor);
    if anchor.subrow > 0 {
        return VisualAnchor {
            subrow: anchor.subrow - 1,
            ..anchor
        };
    }
    let mut line = anchor.line;
    while line > 0 {
        line -= 1;
        if !hidden_in(folds, line) {
            let rows = visual_ranges(buffer, line, width).len().max(1) as u32;
            return VisualAnchor {
                line,
                subrow: rows - 1,
            };
        }
    }
    anchor
}

fn next_line_anchor(folds: &[Fold], line_count: u32, anchor: VisualAnchor) -> VisualAnchor {
    let mut line = anchor.line.saturating_add(1);
    while line < line_count && hidden_in(folds, line) {
        line += 1;
    }
    if line >= line_count {
        anchor
    } else {
        VisualAnchor { line, subrow: 0 }
    }
}

fn visual_anchor_at_row(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    start: VisualAnchor,
    row: u32,
) -> VisualAnchor {
    let mut anchor = normalize_visual_anchor(buffer, folds, width, start);
    for _ in 0..row {
        let next = next_visual_anchor(buffer, folds, width, anchor);
        if next == anchor {
            break;
        }
        anchor = next;
    }
    anchor
}

fn visual_anchor_for_position(buffer: &TextBuffer, width: u32, pos: LineCol) -> VisualAnchor {
    let line = pos.line.min(last_line(buffer));
    let ranges = visual_ranges(buffer, line, width);
    let last = ranges.len().saturating_sub(1);
    let subrow = ranges
        .iter()
        .enumerate()
        .find_map(|(index, range)| {
            (range.start <= pos.col
                && (pos.col < range.end || (index == last && pos.col == range.end)))
                .then_some(index as u32)
        })
        .unwrap_or(last as u32);
    VisualAnchor { line, subrow }
}

fn reveal_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    height: u16,
    current: VisualAnchor,
    cursor: LineCol,
) -> VisualAnchor {
    let current = normalize_visual_anchor(buffer, folds, width, current);
    let target = visual_anchor_for_position(buffer, width, cursor);
    let mut probe = current;
    for _ in 0..height.max(1) {
        if probe == target {
            return current;
        }
        let next = next_visual_anchor(buffer, folds, width, probe);
        if next == probe {
            break;
        }
        probe = next;
    }
    if target < current {
        return target;
    }
    let mut revealed = target;
    for _ in 1..height.max(1) {
        let previous = previous_visual_anchor(buffer, folds, width, revealed);
        if previous == revealed {
            break;
        }
        revealed = previous;
    }
    revealed
}

fn caret_cell(
    area: Rect,
    buffer: &TextBuffer,
    folds: &[Fold],
    state: &EditorState,
    at: LineCol,
) -> Option<(u16, u16)> {
    let line_count = buffer.line_count() as u32;
    let gutter = 1 + digit_count(line_count.max(1)) as u16 + 1;
    let content_x = area.x.saturating_add(gutter);
    if content_x >= area.right() || area.height == 0 {
        return None;
    }
    let content_width = area.right().saturating_sub(content_x);

    if state.last_word_wrap {
        let width = u32::from(content_width.max(1));
        let mut anchor = normalize_visual_anchor(
            buffer,
            folds,
            width,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        for row in 0..area.height {
            let ranges = visual_ranges(buffer, anchor.line, width);
            let index = (anchor.subrow as usize).min(ranges.len().saturating_sub(1));
            let range = ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            let last = index + 1 == ranges.len();
            if anchor.line == at.line
                && (range.start <= at.col)
                && (at.col < range.end || (last && at.col == range.end))
            {
                let rel = at.col.saturating_sub(range.start);
                let x = content_x.saturating_add(
                    u16::try_from(rel.min(u32::from(content_width.saturating_sub(1))))
                        .unwrap_or(u16::MAX),
                );
                return Some((x, area.y.saturating_add(row)));
            }
            anchor = next_visual_anchor(buffer, folds, width, anchor);
        }
        return None;
    }

    let top = first_visible(
        folds,
        state.scroll_line.min(line_count.saturating_sub(1)),
        line_count,
    );
    if at.line < top || hidden_in(folds, at.line) {
        return None;
    }
    let mut vis_row: u16 = 0;
    let mut ll = top;
    while ll < at.line {
        if !hidden_in(folds, ll) {
            vis_row = vis_row.saturating_add(1);
        }
        ll += 1;
    }
    if vis_row >= area.height {
        return None;
    }
    let rel = i64::from(at.col) - i64::from(state.scroll_col);
    let max_rel = i64::from(content_width.saturating_sub(1));
    let cx = content_x.saturating_add(u16::try_from(rel.clamp(0, max_rel)).unwrap_or(0));
    let cy = area.y.saturating_add(vis_row);
    (cy < area.bottom()).then_some((cx, cy))
}

fn first_visible(folds: &[Fold], mut line: u32, line_count: u32) -> u32 {
    while line < line_count && hidden_in(folds, line) {
        line += 1;
    }
    line
}

/// Clamp `pos` to a valid position within `buffer` (line, then column).
fn clamp_to_buffer(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let line = pos.line.min(last_line(buffer));
    LineCol::new(line, pos.col.min(line_len(buffer, line)))
}

/// The editor widget: a builder over the buffer and the (borrowed) data layers
/// the application supplies. Render it as a ratatui [`StatefulWidget`] with an
/// [`EditorState`].
///
/// [`StatefulWidget`]: ratatui::widgets::StatefulWidget
pub struct Editor<'a> {
    buffer: &'a TextBuffer,
    highlights: Option<&'a Highlights>,
    theme: Option<&'a Theme>,
    decorations: &'a [Decoration],
    diagnostics: &'a [Diagnostic],
    inlay_hints: &'a [InlayHint],
    folds: &'a [Fold],
    focused: bool,
    cell_caret: bool,
    read_only: bool,
    word_wrap: bool,
}

impl<'a> Editor<'a> {
    /// Start building an editor over `buffer`.
    #[must_use]
    pub fn new(buffer: &'a TextBuffer) -> Self {
        Self {
            buffer,
            highlights: None,
            theme: None,
            decorations: &[],
            diagnostics: &[],
            inlay_hints: &[],
            folds: &[],
            focused: false,
            cell_caret: true,
            read_only: false,
            word_wrap: false,
        }
    }

    /// Mark the editor focused, so the caret cell is drawn.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Choose whether the focused editor paints its built-in reversed-cell caret.
    ///
    /// Applications that draw a separate terminal-native or pixel-graphics caret can
    /// disable this while keeping focus-dependent cursor-line highlighting.
    #[must_use]
    pub fn cell_caret(mut self, visible: bool) -> Self {
        self.cell_caret = visible;
        self
    }

    /// Render in read-only (pager) mode: never draw the caret and don't highlight
    /// the cursor's line, even when [`focused`](Self::focused). Pair with
    /// [`EditorState::scroll_page_down`]/[`center_on`](EditorState::center_on) to
    /// page through a document without an editable cursor.
    #[must_use]
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Soft-wrap long buffer lines to the available content width.
    #[must_use]
    pub fn word_wrap(mut self, word_wrap: bool) -> Self {
        self.word_wrap = word_wrap;
        self
    }

    /// Supply syntax highlight spans.
    #[must_use]
    pub fn highlights(mut self, highlights: &'a Highlights) -> Self {
        self.highlights = Some(highlights);
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Supply decorations (VCS markers, breakpoints, search highlights, …).
    #[must_use]
    pub fn decorations(mut self, decorations: &'a [Decoration]) -> Self {
        self.decorations = decorations;
        self
    }

    /// Supply diagnostics (from LSP, spell-check, …).
    #[must_use]
    pub fn diagnostics(mut self, diagnostics: &'a [Diagnostic]) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// Supply inlay hints.
    #[must_use]
    pub fn inlay_hints(mut self, inlay_hints: &'a [InlayHint]) -> Self {
        self.inlay_hints = inlay_hints;
        self
    }

    /// Supply the resolved fold regions to render (collapsed folds hide their
    /// interior lines and mark their header).
    #[must_use]
    pub fn folds(mut self, folds: &'a [Fold]) -> Self {
        self.folds = folds;
        self
    }

    /// The fold whose header is line `l`, if any.
    fn fold_at(&self, l: u32) -> Option<Fold> {
        self.folds.iter().copied().find(|f| f.start == l)
    }
}

impl Editor<'_> {
    /// The gutter marker glyph + color for line `l`, if a decoration places one.
    fn gutter_marker(&self, l: u32, theme: &Theme, default_fg: Rgba) -> Option<(char, Rgba)> {
        for d in self.decorations {
            if let DecorationKind::GutterMarker { glyph } = &d.kind
                && line_in_range(l, d.range)
            {
                let color = d.role.map_or(default_fg, |r| theme.role(r));
                return Some((*glyph, color));
            }
        }
        None
    }

    /// The background color for column `col` on line `l`, from line/text-background
    /// decorations (e.g. search matches).
    fn decoration_bg(&self, l: u32, col: u32, theme: &Theme) -> Option<Rgba> {
        for d in self.decorations {
            let Some(role) = d.role else { continue };
            let covers = match &d.kind {
                DecorationKind::LineBackground => line_in_range(l, d.range),
                DecorationKind::TextBackground => col_in_range(l, col, d.range),
                _ => false,
            };
            if covers {
                return Some(theme.role(role));
            }
        }
        None
    }

    /// Append the syntax-colored content spans for line `l`, honoring horizontal
    /// scroll, active selections, and text-background decorations.
    fn push_content_spans(
        &self,
        spans: &mut Vec<Span<'static>>,
        l: u32,
        theme: &Theme,
        default_fg: Rgba,
        range: VisualRange,
        selections: &[Range],
    ) {
        let Some(content) = self.buffer.line(l as usize) else {
            return;
        };
        let Some(line_span) = self.buffer.line_to_byte_range(l as usize) else {
            return;
        };
        let line_start = line_span.start.0;
        let hl: &[HighlightSpan] = self.highlights.map_or(&[], |h| h.spans_in(line_span));

        let mut run = String::new();
        let mut run_style: Option<Style> = None;
        let mut col: u32 = 0;
        for (boff, ch) in content.char_indices() {
            if col < range.start {
                col += 1;
                continue;
            }
            if col >= range.end {
                break;
            }
            let mut style = token_style(line_start + boff, hl, theme, default_fg);
            let bg = if in_any(selections, l, col) {
                Some(theme.role(ThemeRole::Selection))
            } else {
                self.decoration_bg(l, col, theme)
            };
            if let Some(bg) = bg {
                style = style.bg(bg.to_ratatui());
            }
            if run_style == Some(style) {
                run.push(ch);
            } else {
                if let Some(prev) = run_style {
                    spans.push(Span::styled(std::mem::take(&mut run), prev));
                }
                run.push(ch);
                run_style = Some(style);
            }
            col += 1;
        }
        if let Some(prev) = run_style {
            spans.push(Span::styled(run, prev));
        }
    }

    /// Draw the caret at buffer position `at` as a reversed cell, when it falls within
    /// the visible, non-folded region of `area`. Called once per caret so multi-cursor
    /// renders every head.
    fn draw_caret(&self, buf: &mut Buffer, area: Rect, state: &EditorState, at: LineCol) {
        let Some((cx, cy)) = caret_cell(area, self.buffer, self.folds, state, at) else {
            return;
        };
        buf.set_style(
            Rect {
                x: cx,
                y: cy,
                width: 1,
                height: 1,
            },
            Style::default().add_modifier(Modifier::REVERSED),
        );
    }
}

impl StatefulWidget for Editor<'_> {
    type State = EditorState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut EditorState) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let fallback;
        let theme = match self.theme {
            Some(theme) => theme,
            None => {
                fallback = Theme::dark();
                &fallback
            },
        };

        let line_count = self.buffer.line_count() as u32;
        let background = theme.role(ThemeRole::Background);
        let cursor_line_bg = theme.role(ThemeRole::CursorLine);
        let default_fg = theme.role(ThemeRole::Foreground);
        let digits = digit_count(line_count.max(1));

        let gutter = 1 + digits as u16 + 1;
        let content_width = area.width.saturating_sub(gutter).max(1);

        // Clamp scroll to the buffer and record viewport geometry for motions and
        // mouse scrolling between frames.
        state.last_height = area.height;
        state.last_content_width = content_width;
        state.last_word_wrap = self.word_wrap;
        state.scroll_line = state.scroll_line.min(line_count.saturating_sub(1));
        if self.word_wrap {
            state.scroll_col = 0;
        } else {
            state.scroll_subrow = 0;
        }

        let width = u32::from(content_width);
        let mut anchor = normalize_visual_anchor(
            self.buffer,
            self.folds,
            width,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        if self.word_wrap && state.follow_cursor {
            anchor = reveal_visual_anchor(
                self.buffer,
                self.folds,
                width,
                area.height,
                anchor,
                state.cursor(),
            );
        }
        state.scroll_line = anchor.line;
        state.scroll_subrow = if self.word_wrap { anchor.subrow } else { 0 };
        state.follow_cursor = false;

        // Snapshot the cursor set for painting: every non-empty selection's range and
        // the line of every caret (each caret's line gets the cursor-line emphasis).
        let selections = state.selection_ranges();
        let caret_lines: Vec<u32> = state
            .cursors()
            .selections
            .iter()
            .map(|s| s.head.line)
            .collect();

        // Base background for the whole editor area (covers rows past end-of-file).
        buf.set_style(area, Style::default().bg(background.to_ratatui()));

        for row in 0..area.height {
            if anchor.line >= line_count {
                break;
            }
            let l = anchor.line;
            let y = area.y + row;
            // In read-only (pager) mode there is no active cursor line to emphasize.
            let is_cursor = !self.read_only && caret_lines.contains(&l);
            let row_bg = if is_cursor {
                cursor_line_bg
            } else {
                background
            };
            buf.set_style(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                Style::default().bg(row_bg.to_ratatui()),
            );

            // A fold header shows a collapse/expand chevron in the marker column;
            // other lines show their usual decoration marker (git/diagnostic).
            let first_row = !self.word_wrap || anchor.subrow == 0;
            let fold = self.fold_at(l);
            let (marker_ch, marker_color) = match (first_row, fold) {
                (true, Some(f)) => (
                    if f.collapsed { '\u{25b8}' } else { '\u{25be}' },
                    theme.role(ThemeRole::LineNumberActive),
                ),
                (true, None) => self
                    .gutter_marker(l, theme, default_fg)
                    .unwrap_or((' ', default_fg)),
                (false, _) => (' ', default_fg),
            };
            let number_color = if is_cursor {
                theme.role(ThemeRole::LineNumberActive)
            } else {
                theme.role(ThemeRole::LineNumber)
            };
            let mut spans = vec![
                Span::styled(
                    marker_ch.to_string(),
                    Style::default().fg(marker_color.to_ratatui()),
                ),
                Span::styled(
                    if first_row {
                        format!("{:>width$} ", l + 1, width = digits)
                    } else {
                        " ".repeat(digits + 1)
                    },
                    Style::default().fg(number_color.to_ratatui()),
                ),
            ];
            let ranges = if self.word_wrap {
                visual_ranges(self.buffer, l, width)
            } else {
                vec![VisualRange {
                    start: state.scroll_col,
                    end: u32::MAX,
                }]
            };
            let range_index = if self.word_wrap {
                (anchor.subrow as usize).min(ranges.len().saturating_sub(1))
            } else {
                0
            };
            let range = ranges
                .get(range_index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            self.push_content_spans(&mut spans, l, theme, default_fg, range, &selections);
            // A collapsed header hints at the hidden lines it conceals.
            if fold.is_some_and(|f| f.collapsed) && range_index + 1 == ranges.len() {
                spans.push(Span::styled(
                    " \u{22ef}", // ⋯
                    Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui()),
                ));
            }
            buf.set_line(area.x, y, &Line::from(spans), area.width);

            let next = if self.word_wrap {
                next_visual_anchor(self.buffer, self.folds, width, anchor)
            } else {
                next_line_anchor(self.folds, line_count, anchor)
            };
            if next == anchor {
                break;
            }
            anchor = next;
        }

        // Draw a reversed caret cell for every head when focused and editable.
        if self.focused && self.cell_caret && !self.read_only {
            for sel in &state.cursors().selections {
                self.draw_caret(buf, area, state, sel.head);
            }
        }
    }
}

/// Whether the cell at line `l`, column `col` lies within any of `selections`.
fn in_any(selections: &[Range], l: u32, col: u32) -> bool {
    selections.iter().any(|r| col_in_range(l, col, *r))
}

/// Whether `c` is part of a word (alphanumeric or underscore), for word motions.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The `(start, end)` of the word (alphanumeric + `_`) around `pos` on its line, or a
/// single-character span when `pos` is not on a word character. Mirrors the span a
/// double-click selects; reused by the app's click handling.
#[must_use]
pub fn word_bounds(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let n = chars.len() as u32;
    let col = pos.col.min(n);
    let mut start = col;
    while start > 0 && is_word_char(chars[start as usize - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < n && is_word_char(chars[end as usize]) {
        end += 1;
    }
    if start == end {
        (
            LineCol::new(pos.line, col),
            LineCol::new(pos.line, (col + 1).min(n)),
        )
    } else {
        (LineCol::new(pos.line, start), LineCol::new(pos.line, end))
    }
}

/// The text within `range`, or `None` if either end can't be resolved to a byte.
fn slice_text(buffer: &TextBuffer, range: Range) -> Option<String> {
    let start = buffer.line_col_to_byte(range.start).ok()?.0;
    let end = buffer.line_col_to_byte(range.end).ok()?.0;
    buffer.text().get(start..end).map(str::to_string)
}

/// The byte offset of the next occurrence of `needle` in `hay` at or after `from`,
/// wrapping around to the start of the buffer.
fn find_next(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let from = from.min(hay.len());
    hay.get(from..)
        .and_then(|tail| tail.find(needle).map(|i| from + i))
        .or_else(|| hay.get(..from).and_then(|head| head.find(needle)))
}

/// The start of the word before `pos`, wrapping to the previous line's end when at
/// column 0. Skips trailing whitespace, then a single word/punctuation run.
fn prev_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    if pos.col == 0 {
        return if pos.line > 0 {
            let line = pos.line - 1;
            LineCol::new(line, line_len(buffer, line))
        } else {
            pos
        };
    }
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let mut i = (pos.col as usize).min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i > 0 {
        let word = is_word_char(chars[i - 1]);
        while i > 0 && !chars[i - 1].is_whitespace() && is_word_char(chars[i - 1]) == word {
            i -= 1;
        }
    }
    LineCol::new(pos.line, i as u32)
}

/// The end of the word after `pos`, wrapping to the next line's start at end of line.
/// Skips leading whitespace, then a single word/punctuation run.
fn next_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let n = chars.len();
    if pos.col as usize >= n {
        return if pos.line < last_line(buffer) {
            LineCol::new(pos.line + 1, 0)
        } else {
            pos
        };
    }
    let mut i = pos.col as usize;
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    if i < n {
        let word = is_word_char(chars[i]);
        while i < n && !chars[i].is_whitespace() && is_word_char(chars[i]) == word {
            i += 1;
        }
    }
    LineCol::new(pos.line, i as u32)
}

/// Whether line `l` is hidden inside the interior of a collapsed fold in `folds`.
fn hidden_in(folds: &[Fold], l: u32) -> bool {
    folds
        .iter()
        .any(|f| f.collapsed && l > f.start && l <= f.end)
}

/// The index of the last line in `buffer` (0 for an empty buffer).
fn last_line(buffer: &TextBuffer) -> u32 {
    (buffer.line_count().max(1) - 1) as u32
}

/// The length (in `char`s) of line `line` in `buffer`.
fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    buffer
        .line(line as usize)
        .map_or(0, |s| s.chars().count() as u32)
}

/// The number of decimal digits needed to print `n`.
fn digit_count(n: u32) -> usize {
    if n < 10 { 1 } else { (n.ilog10() + 1) as usize }
}

/// Whether line `l` falls within the line span of `range`.
fn line_in_range(l: u32, range: Range) -> bool {
    l >= range.start.line && l <= range.end.line
}

/// Whether column `col` on line `l` falls within `range`.
fn col_in_range(l: u32, col: u32, range: Range) -> bool {
    if !line_in_range(l, range) {
        return false;
    }
    let lo = if l == range.start.line {
        range.start.col
    } else {
        0
    };
    let hi = if l == range.end.line {
        range.end.col
    } else {
        u32::MAX
    };
    col >= lo && col < hi
}

/// The semantic token covering absolute byte `abs`, if any highlight span claims it.
fn token_at(abs: usize, hl: &[HighlightSpan]) -> Option<TokenId> {
    hl.iter()
        .find(|s| s.span.start.0 <= abs && abs < s.span.end.0)
        .map(|s| s.token)
}

/// The style (foreground + emphasis) for the char at absolute byte `abs`. Markup
/// tokens carry bold/italic, so color alone is not enough.
fn token_style(abs: usize, hl: &[HighlightSpan], theme: &Theme, default_fg: Rgba) -> Style {
    match token_at(abs, hl) {
        Some(token) => Style::default()
            .fg(theme.color(token).to_ratatui())
            .add_modifier(theme.emphasis(token).to_ratatui()),
        None => Style::default().fg(default_fg.to_ratatui()),
    }
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;
    use karet_core::TokenId;

    use super::*;

    #[test]
    fn editor_builder_collects_layers() {
        let buffer = TextBuffer::from_text("fn main() {}");
        let _editor = Editor::new(&buffer).diagnostics(&[]).decorations(&[]);
        assert_eq!(EditorState::new().scroll_line, 0);
    }

    #[test]
    fn token_style_uses_highlight_then_default() {
        let theme = Theme::dark();
        let default_fg = theme.role(ThemeRole::Foreground);
        let hl = [HighlightSpan {
            span: karet_core::Span {
                start: BytePos(0),
                end: BytePos(2),
            },
            token: TokenId(0),
        }];
        assert_eq!(
            token_style(1, &hl, &theme, default_fg).fg,
            Some(theme.color(TokenId(0)).to_ratatui())
        );
        assert_eq!(
            token_style(5, &hl, &theme, default_fg).fg,
            Some(default_fg.to_ratatui())
        );
    }

    #[test]
    fn token_style_applies_markup_emphasis() {
        use karet_core::StandardToken;
        let theme = Theme::dark();
        let default_fg = theme.role(ThemeRole::Foreground);
        let hl = [HighlightSpan {
            span: karet_core::Span {
                start: BytePos(0),
                end: BytePos(4),
            },
            token: StandardToken::MarkupBold.id(),
        }];
        // A bold markup span renders bold, not merely recolored.
        let style = token_style(2, &hl, &theme, default_fg);
        assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
        // Unhighlighted text carries no modifier.
        let plain = token_style(9, &hl, &theme, default_fg);
        assert!(plain.add_modifier.is_empty());
    }

    #[test]
    fn scroll_to_keeps_cursor_in_view() {
        let mut state = EditorState::new();
        state.last_height = 10;
        state.scroll_to(LineCol::new(25, 0));
        let vp = state.viewport();
        assert!(vp.start.line <= 25 && 25 < vp.end.line);
        state.scroll_to(LineCol::new(0, 0));
        assert_eq!(state.scroll_line, 0);
    }

    #[test]
    fn motions_clamp_to_buffer() {
        let buffer = TextBuffer::from_text("ab\ncde\nf");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.move_down(&buffer);
        state.move_down(&buffer);
        state.move_down(&buffer); // past the end clamps to the last line
        assert_eq!(state.cursor().line, 2);
        state.goto(&buffer, LineCol::new(1, 99)); // col clamps to the line length
        assert_eq!(state.cursor(), LineCol::new(1, 3));
    }

    #[test]
    fn pos_at_accounts_for_gutter_and_scroll() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let mut state = EditorState::new();
        state.last_height = 3;
        let area = Rect::new(0, 0, 20, 3);
        // gutter = marker(1) + 1 digit + space = 3; column 5 -> content col 2.
        assert_eq!(state.pos_at(area, &buffer, &[], 5, 0), LineCol::new(0, 2));
        // A click past the line end clamps to the line length.
        assert_eq!(state.pos_at(area, &buffer, &[], 100, 0), LineCol::new(0, 5));
        // Vertical scroll shifts the mapped line.
        state.scroll_line = 1;
        assert_eq!(state.pos_at(area, &buffer, &[], 3, 0), LineCol::new(1, 0));
    }

    #[test]
    fn pos_at_skips_collapsed_fold_interiors() {
        let buffer = TextBuffer::from_text("l0\nl1\nl2\nl3\nl4");
        let mut state = EditorState::new();
        state.last_height = 5;
        let area = Rect::new(0, 0, 20, 5);
        // Collapse lines 1..=3 under a fold headered on line 0. Visible order is now
        // l0, l4 — so screen row 1 maps to buffer line 4, not line 1.
        let folds = [Fold {
            start: 0,
            end: 3,
            collapsed: true,
        }];
        assert_eq!(
            state.pos_at(area, &buffer, &folds, 3, 0),
            LineCol::new(0, 0)
        );
        assert_eq!(
            state.pos_at(area, &buffer, &folds, 3, 1),
            LineCol::new(4, 0)
        );
    }

    #[test]
    fn word_wrap_renders_continuations_and_maps_clicks() {
        let buffer = TextBuffer::from_text("alpha beta gamma");
        let mut state = EditorState::new();
        let area = Rect::new(0, 0, 10, 3); // 3-cell gutter, 7 content cells.
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .word_wrap(true)
            .focused(true)
            .render(area, &mut buf, &mut state);

        let painted = |row| {
            (0..area.width)
                .map(|x| buf[(x, row)].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
        };
        assert!(painted(0).contains("alpha"));
        assert!(painted(1).contains("beta"));
        assert!(painted(2).contains("gamma"));
        assert_eq!(state.pos_at(area, &buffer, &[], 4, 1), LineCol::new(0, 7));

        state.place_caret(LineCol::new(0, 8));
        assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((5, 1)));
    }

    #[test]
    fn wrapped_row_scrolling_walks_continuations_before_lines() {
        let buffer = TextBuffer::from_text("alpha beta gamma\ntail");
        let mut state = EditorState::new();
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .word_wrap(true)
            .render(area, &mut buf, &mut state);

        state.scroll_rows(&buffer, &[], true, 1);
        assert_eq!((state.scroll_line, state.scroll_subrow), (0, 1));
        state.scroll_rows(&buffer, &[], true, 2);
        assert_eq!((state.scroll_line, state.scroll_subrow), (1, 0));
        state.scroll_rows(&buffer, &[], true, -1);
        assert_eq!((state.scroll_line, state.scroll_subrow), (0, 2));
    }

    #[test]
    fn overflow_scrolling_and_cursor_margin_are_clamped() {
        let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
        let mut state = EditorState::new();
        let area = Rect::new(0, 0, 25, 1); // 22 content cells, 10-cell margin.
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer).render(area, &mut buf, &mut state);

        state.goto(&buffer, LineCol::new(0, 15));
        assert_eq!(state.scroll_col, 4);
        state.goto(&buffer, LineCol::new(0, 2));
        assert_eq!(state.scroll_col, 0);

        state.scroll_columns(&buffer, 3);
        assert_eq!(state.scroll_col, 3);
        state.scroll_columns(&buffer, i32::MAX);
        assert_eq!(state.scroll_col, 5, "longest line clamps manual scrolling");
    }

    #[test]
    fn overflow_caret_clamps_to_horizontal_edges() {
        let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
        let mut state = EditorState::new();
        let area = Rect::new(0, 0, 10, 1); // 3-cell gutter, 7 content cells.
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer).render(area, &mut buf, &mut state);
        state.scroll_col = 10;

        state.place_caret(LineCol::new(0, 2));
        assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((3, 0)));
        state.place_caret(LineCol::new(0, 22));
        assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((9, 0)));

        Editor::new(&buffer)
            .word_wrap(true)
            .render(area, &mut buf, &mut state);
        state.scroll_columns(&buffer, 3);
        assert_eq!(
            state.scroll_col, 0,
            "wrapped views never scroll horizontally"
        );
    }

    #[test]
    fn selection_range_normalizes_and_clears() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let mut state = EditorState::new();
        state.last_height = 3;
        assert_eq!(state.selection_range(), None);
        state.set_caret(&buffer, LineCol::new(0, 2));
        assert_eq!(
            state.selection_range(),
            None,
            "a bare caret is not a selection"
        );
        state.extend_to(&buffer, LineCol::new(1, 1));
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 2),
                end: LineCol::new(1, 1),
            })
        );
        // Dragging back above the anchor normalizes start <= end.
        state.extend_to(&buffer, LineCol::new(0, 0));
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 2),
            })
        );
        state.clear_selection();
        assert_eq!(state.selection_range(), None);
    }

    #[test]
    fn render_draws_gutter_and_cursor_line() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let theme = Theme::dark();
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(1, 0));
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .render(area, &mut buf, &mut state);

        let row0: String = (0..20)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row0.contains('1'), "gutter line number missing: {row0:?}");
        assert!(row0.contains("alpha"), "content missing: {row0:?}");
        // The cursor row (line 1) carries the cursor-line background.
        assert_eq!(
            buf[(0, 1)].bg,
            theme.role(ThemeRole::CursorLine).to_ratatui()
        );
        // A non-cursor row uses the editor background.
        assert_eq!(
            buf[(0, 0)].bg,
            theme.role(ThemeRole::Background).to_ratatui()
        );
    }

    #[test]
    fn line_word_and_doc_motions() {
        let buffer = TextBuffer::from_text("foo bar\nbaz");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.move_line_end(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 7));
        state.move_line_start(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 0));
        // Word-right lands at the end of each word, then wraps to the next line.
        state.move_word_right(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 3));
        state.move_word_right(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 7));
        state.move_word_right(&buffer);
        assert_eq!(state.cursor(), LineCol::new(1, 0));
        // Word-left from column 0 wraps to the previous line's end.
        state.move_word_left(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 7));
        state.move_doc_end(&buffer);
        assert_eq!(state.cursor(), LineCol::new(1, 3));
        state.move_doc_start(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 0));
    }

    #[test]
    fn select_all_spans_the_whole_buffer() {
        let buffer = TextBuffer::from_text("ab\ncde");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.select_all(&buffer);
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(1, 3),
            })
        );
    }

    #[test]
    fn render_draws_a_caret_for_every_cursor() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let theme = Theme::dark();
        let mut state = EditorState::new();
        state.set_carets(&[LineCol::new(0, 0), LineCol::new(2, 0)]);
        assert!(state.has_multiple_cursors());
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .focused(true)
            .render(area, &mut buf, &mut state);
        // Gutter is 1 marker + 1 digit + 1 space = 3; both caret rows get a caret cell.
        let gutter = 3;
        assert!(buf[(gutter, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(gutter, 2)].modifier.contains(Modifier::REVERSED));
        // The caret-free middle line has no reversed cell.
        let row1_caret = (0..area.width).any(|x| buf[(x, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!row1_caret, "line 1 has no caret");
    }

    #[test]
    fn cell_caret_can_be_suppressed_while_focused() {
        let buffer = TextBuffer::from_text("abc\n");
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(0, 1));
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .focused(true)
            .cell_caret(false)
            .render(area, &mut buf, &mut state);
        let any_caret = (0..area.width)
            .any(|x| (0..area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED)));
        assert!(!any_caret);
    }

    #[test]
    fn primary_caret_cell_matches_rendered_gutter_geometry() {
        let buffer = TextBuffer::from_text("abc\n");
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(0, 2));
        let area = Rect::new(10, 5, 20, 4);
        assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((15, 5)));
    }

    #[test]
    fn set_carets_preserves_count_and_merges_coincident() {
        let mut state = EditorState::new();
        state.set_carets(&[LineCol::new(0, 0), LineCol::new(1, 2)]);
        assert_eq!(state.cursors().selections.len(), 2);
        // Two carets at the same spot collapse back to one.
        state.set_carets(&[LineCol::new(3, 3), LineCol::new(3, 3)]);
        assert!(!state.has_multiple_cursors());
        assert_eq!(state.cursor(), LineCol::new(3, 3));
    }

    #[test]
    fn add_caret_below_clamps_to_short_line() {
        let buffer = TextBuffer::from_text("longline\nab");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.place_caret(LineCol::new(0, 6));
        state.add_caret_below(&buffer);
        let heads: Vec<LineCol> = state.cursors().selections.iter().map(|s| s.head).collect();
        assert_eq!(heads, vec![LineCol::new(0, 6), LineCol::new(1, 2)]);
    }

    #[test]
    fn add_caret_above_is_noop_on_the_top_line() {
        let buffer = TextBuffer::from_text("ab\ncd");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.place_caret(LineCol::new(0, 1));
        state.add_caret_above(&buffer);
        assert!(!state.has_multiple_cursors());
    }

    #[test]
    fn add_caret_toggles_a_coincident_caret() {
        let buffer = TextBuffer::from_text("abcdef");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.place_caret(LineCol::new(0, 0));
        state.add_caret(&buffer, LineCol::new(0, 3));
        assert_eq!(state.cursors().selections.len(), 2);
        // Alt-adding at the same spot removes it, leaving the original.
        state.add_caret(&buffer, LineCol::new(0, 3));
        assert!(!state.has_multiple_cursors());
        assert_eq!(state.cursor(), LineCol::new(0, 0));
    }

    #[test]
    fn add_next_occurrence_selects_word_then_next_match() {
        let buffer = TextBuffer::from_text("foo bar foo");
        let mut state = EditorState::new();
        state.last_height = 4;
        state.place_caret(LineCol::new(0, 1)); // inside the first "foo"
        state.add_next_occurrence(&buffer);
        assert_eq!(
            state.selection_range(),
            Some(Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 3),
            })
        );
        state.add_next_occurrence(&buffer);
        assert!(state.has_multiple_cursors());
        assert!(state.selection_ranges().contains(&Range {
            start: LineCol::new(0, 8),
            end: LineCol::new(0, 11),
        }));
    }

    #[test]
    fn word_bounds_spans_the_word_under_pos() {
        let buffer = TextBuffer::from_text("foo bar");
        assert_eq!(
            word_bounds(&buffer, LineCol::new(0, 5)),
            (LineCol::new(0, 4), LineCol::new(0, 7))
        );
    }

    #[test]
    fn read_only_suppresses_cursor_line_and_caret() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
        let theme = Theme::dark();
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(1, 0));
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .focused(true) // focused, but read-only must still hide the caret
            .read_only(true)
            .render(area, &mut buf, &mut state);

        // The cursor's line carries the plain background, not the cursor-line color.
        assert_eq!(
            buf[(0, 1)].bg,
            theme.role(ThemeRole::Background).to_ratatui(),
            "read-only mode must not highlight the cursor line"
        );
        // No caret cell is drawn anywhere.
        let any_caret = (0..area.width)
            .any(|x| (0..area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED)));
        assert!(!any_caret, "read-only mode must not draw a caret");
    }

    #[test]
    fn center_on_and_scroll_paging_move_viewport_only() {
        let mut state = EditorState::new();
        state.last_height = 10;
        state.center_on(50);
        assert_eq!(state.scroll_line, 45, "line centered in a 10-row viewport");
        // Scroll-only paging moves the viewport without touching the cursor.
        state.scroll_page_up();
        assert_eq!(state.scroll_line, 35);
        state.scroll_page_down();
        assert_eq!(state.scroll_line, 45);
        assert_eq!(state.cursor().line, 0, "paging never moves the cursor");
        // Centering near the top saturates at 0.
        state.center_on(2);
        assert_eq!(state.scroll_line, 0);
    }
}
