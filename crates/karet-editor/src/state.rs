use std::cell::Cell;
use std::ops::RangeInclusive;

use super::hint::HintIndex;
use super::hint::LineHints;
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

/// Resolve fold regions against the set of collapsed header lines into the
/// editor's per-render [`Fold`] list.
#[must_use]
pub fn resolve_folds(
    folds: &karet_core::FoldRegions,
    folded: &std::collections::BTreeSet<u32>,
) -> Vec<Fold> {
    folds
        .regions()
        .iter()
        .map(|r| Fold {
            start: r.start,
            end: r.end,
            collapsed: folded.contains(&r.start),
        })
        .collect()
}

/// A caret motion, applied to every caret by [`EditorState::move_carets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    /// One line up, column clamped to the shorter line.
    Up,
    /// One line down, column clamped to the shorter line.
    Down,
    /// One character left, wrapping to the previous line's end.
    Left,
    /// One character right, wrapping to the next line's start.
    Right,
    /// One viewport height up.
    PageUp,
    /// One viewport height down.
    PageDown,
    /// To column 0 of the line.
    LineStart,
    /// To the end of the line.
    LineEnd,
    /// To the start of the document.
    DocStart,
    /// To the end of the document.
    DocEnd,
    /// To the start of the previous word, wrapping across lines.
    WordLeft,
    /// To the end of the next word, wrapping across lines.
    WordRight,
}

impl Motion {
    /// Whether the motion heads toward the start of the document, which decides
    /// the selection edge a non-extending motion collapses to.
    #[must_use]
    pub const fn is_backward(self) -> bool {
        matches!(
            self,
            Self::Up
                | Self::Left
                | Self::PageUp
                | Self::LineStart
                | Self::DocStart
                | Self::WordLeft
        )
    }
}

/// Where `motion` takes a caret at `from`, with `page` lines to a page.
fn step(buffer: &TextBuffer, motion: Motion, page: u32, from: LineCol) -> LineCol {
    let to_line = |line: u32| LineCol::new(line, from.col.min(line_len(buffer, line)));
    let last = last_line(buffer);
    match motion {
        Motion::Up => to_line(from.line.saturating_sub(1)),
        Motion::Down => to_line((from.line + 1).min(last)),
        Motion::PageUp => to_line(from.line.saturating_sub(page)),
        Motion::PageDown => to_line(from.line.saturating_add(page).min(last)),
        Motion::Left if from.col > 0 => LineCol::new(from.line, from.col - 1),
        Motion::Left if from.line > 0 => {
            LineCol::new(from.line - 1, line_len(buffer, from.line - 1))
        },
        Motion::Right if from.col < line_len(buffer, from.line) => {
            LineCol::new(from.line, from.col + 1)
        },
        Motion::Right if from.line < last => LineCol::new(from.line + 1, 0),
        Motion::Left | Motion::Right => from,
        Motion::LineStart => LineCol::new(from.line, 0),
        Motion::LineEnd => LineCol::new(from.line, line_len(buffer, from.line)),
        Motion::DocStart => LineCol::new(0, 0),
        Motion::DocEnd => LineCol::new(last, line_len(buffer, last)),
        Motion::WordLeft => previous_word_boundary(buffer, from),
        Motion::WordRight => next_word_boundary(buffer, from),
    }
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
    /// The first visible column (horizontal scroll), as a **buffer** column
    /// counted in `char`s: an unwrapped row starts painting at this column.
    ///
    /// Screen positions within the row are then measured in display cells
    /// from there — tabs, wide characters and inlay hints included — which is
    /// how the caret, mouse clicks, cursor reveal and
    /// [`longest_col`](Self::longest_col) all read it.
    pub scroll_col: u32,
    /// The cursor/selection set (never empty). The moving end of each selection is its
    /// `head`; the primary selection's head is the main caret.
    pub(super) cursors: CursorState,
    /// Index into the cursor set of the first-placed caret, the one `Esc` keeps
    /// ([`collapse_to_original`](Self::collapse_to_original)). Followed through
    /// every sort and merge, and reset whenever the set becomes one caret.
    pub(super) original: usize,
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
    /// Inlay hints captured at the last render, indexed for lookup.
    ///
    /// Cached rather than recomputed because a mouse click resolves outside a
    /// render, and it must land against the geometry actually on screen. A
    /// click resolved against hints the frame did not paint would select the
    /// wrong character.
    pub(super) last_hints: HintIndex,
    /// Whether the next render should reveal a cursor moved by an editor
    /// command rather than preserve a manually-scrolled viewport.
    pub(super) follow_cursor: bool,
    /// A horizontal reveal the next unwrapped render still owes: set only when
    /// the scroll could not be resolved on the spot — [`scroll_to`] was given
    /// no buffer, or the last frame was wrapped or never measured the viewport.
    ///
    /// Paired with the margin that reveal keeps, so the render re-resolves a
    /// pointer-originated move without the keyboard's margin.
    ///
    /// [`scroll_to`]: Self::scroll_to
    pub(super) follow_col: Option<(LineCol, Reveal)>,
    /// The collapsed folds the last render hid lines behind, so a reveal made
    /// between renders counts rows, not buffer lines.
    pub(super) last_folds: Vec<Fold>,
    /// Source lines currently occupying the sticky-scroll rows, outermost first.
    pub(super) sticky_rows: Vec<u32>,
    /// Rows reserved above the live document viewport by the last render.
    pub(super) sticky_height: u16,
    /// Distinct buffer lines the last render painted — see [`visible_lines`].
    ///
    /// [`visible_lines`]: EditorState::visible_lines
    pub(super) last_visible_lines: u32,
    /// High-water mark of the longest line seen — see [`longest_col`].
    ///
    /// [`longest_col`]: EditorState::longest_col
    pub(super) last_longest_col: u32,
    /// The primary head, when a pointer placed it past the end of a wrapped
    /// row: that position is also where the next row starts, and it is drawn
    /// at the end of the row the user clicked rather than the start of the
    /// next. Dropped by any keyboard move, and by the render once the head
    /// has moved elsewhere.
    pub(super) upstream: Option<LineCol>,
    /// What the last [`pos_at`](Self::pos_at) resolved, when it resolved past
    /// the end of a wrapped row, so the pointer move that consumes it
    /// ([`set_caret`](Self::set_caret), [`extend_to`](Self::extend_to)) can
    /// keep the caret on that row. A `Cell` because hit-testing is `&self`.
    pub(super) pointer_upstream: Cell<Option<LineCol>>,
    /// Where the last render painted each inline text decoration, keyed by
    /// its index in the slice given to `Editor::decorations` — see
    /// [`inline_text_rect`](Self::inline_text_rect).
    pub(super) last_inline_text: Vec<(usize, Rect)>,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            scroll_line: 0,
            scroll_col: 0,
            cursors: CursorState::single(Selection::caret(LineCol::new(0, 0))),
            original: 0,
            last_height: 0,
            scroll_subrow: 0,
            last_content_width: 0,
            last_word_wrap: false,
            last_tab_width: 4,
            last_unwrapped_lines: Vec::new(),
            last_hints: HintIndex::default(),
            follow_cursor: false,
            follow_col: None,
            last_folds: Vec::new(),
            sticky_rows: Vec::new(),
            sticky_height: 0,
            last_visible_lines: 0,
            last_longest_col: 0,
            upstream: None,
            pointer_upstream: Cell::new(None),
            last_inline_text: Vec::new(),
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

    /// Scroll so that `pos` is within the viewport, when the buffer is not at
    /// hand.
    ///
    /// Vertical scroll is applied immediately. The horizontal reveal is measured
    /// in the display cells the caret is drawn in — tabs, wide characters and
    /// inlay hints ahead of it included — which needs the line's text, so
    /// without a buffer it is left to the next render and
    /// [`scroll_col`](Self::scroll_col) stays stale until then. Prefer
    /// [`reveal`](Self::reveal), which resolves both axes now.
    pub fn scroll_to(&mut self, pos: LineCol) {
        self.scroll_vertically_to(pos);
        self.follow_col = Some((pos, Reveal::Margin));
    }

    /// Scroll so that `pos` is within the viewport, resolving both axes
    /// immediately and re-checking both at the next render.
    ///
    /// This is the keyboard reveal: in an unwrapped view the horizontal scroll
    /// keeps up to a 10-cell margin either side of the caret, measured in
    /// display cells against the last frame's geometry: its content width, tab
    /// width and inlay hints. So [`scroll_col`](Self::scroll_col) is already
    /// right when read before the next render — as a view that mirrors this
    /// one's offset does. The render then re-resolves it against the geometry
    /// it actually paints, so a pane that changed width since still shows the
    /// caret. Soft-wrapped views have no horizontal axis. Before any render
    /// has measured the viewport the horizontal part is deferred to the first
    /// one.
    ///
    /// Vertically the caret's line is brought onto a visible row, counting
    /// rows rather than buffer lines — collapsed folds take none, wrapped
    /// lines take several, sticky headers shrink the viewport — so a line
    /// already on screen never scrolls. The estimate made here uses the last
    /// frame's folds and height; the render completes it against the ones it
    /// paints.
    ///
    /// The pointer-originated moves — [`set_caret`](Self::set_caret),
    /// [`extend_to`](Self::extend_to), [`set_selection`](Self::set_selection)
    /// and [`add_caret`](Self::add_caret) — keep no margin instead: they
    /// scroll only when the target is off screen.
    pub fn reveal(&mut self, buffer: &TextBuffer, pos: LineCol) {
        // A keyboard-placed caret at a wrap point opens the next row.
        self.upstream = None;
        self.reveal_with(buffer, pos, Reveal::Margin);
    }

    /// [`reveal`](Self::reveal) keeping the horizontal margin `reveal` asks for.
    fn reveal_with(&mut self, buffer: &TextBuffer, pos: LineCol, reveal: Reveal) {
        self.scroll_vertically_to(pos);
        // Re-resolved by the render whatever happens here: the last frame's
        // width may not be the next one's (a resize, a split, a background tab
        // brought forward), and a wrapped frame may be followed by an
        // unwrapped one.
        self.follow_col = Some((pos, reveal));
        if self.last_word_wrap || self.last_content_width == 0 {
            return;
        }
        self.scroll_col = reveal_column(
            &line_chars(buffer, pos.line),
            self.scroll_col,
            pos.col,
            u32::from(self.last_content_width),
            self.last_tab_width,
            self.hints_on(pos.line),
            reveal,
        );
    }

    /// Bring `pos`'s line onto a visible row and have the next render follow
    /// the cursor.
    fn scroll_vertically_to(&mut self, pos: LineCol) {
        self.follow_cursor = true;
        let height = u32::from(self.last_height.max(1));
        let top = reveal_line(&self.last_folds, self.scroll_line, pos.line, height);
        if top != self.scroll_line {
            self.scroll_line = top;
            self.scroll_subrow = 0;
        }
    }

    /// The geometry of the last render, at `width` content columns.
    pub(super) fn layout(&self, width: u32) -> Layout<'_> {
        Layout {
            width,
            tab_width: self.last_tab_width,
            unwrapped_lines: &self.last_unwrapped_lines,
            hints: &self.last_hints,
        }
    }

    /// The inlay hints the last render painted on `line`.
    pub(super) fn hints_on(&self, line: u32) -> LineHints<'_> {
        self.last_hints.line(line)
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
                previous_visual_anchor(buffer, folds, self.layout(width), anchor)
            } else {
                next_visual_anchor(buffer, folds, self.layout(width), anchor)
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
        let width = u32::from(self.last_content_width.max(1));
        let longest = (0..buffer.line_count() as u32)
            .map(|line| {
                horizontal_extent(
                    &line_chars(buffer, line),
                    width,
                    self.last_tab_width,
                    self.hints_on(line),
                )
            })
            .max()
            .unwrap_or(0);
        // The scan is already paid for here, so keep the answer for the horizontal
        // scrollbar rather than making the render pay for it again every frame.
        self.last_longest_col = longest;
        let max = longest.saturating_add(1).saturating_sub(width);
        self.scroll_col =
            (i64::from(self.scroll_col) + i64::from(delta)).clamp(0, i64::from(max)) as u32;
    }

    /// Scroll so `line` is the topmost buffer line, without moving the caret.
    ///
    /// The absolute counterpart to [`scroll_rows`](Self::scroll_rows), for a
    /// scrollbar that hands over a position rather than a delta. It is not the same
    /// as stepping there: in soft-wrap mode `scroll_rows` walks one visual anchor per
    /// row, so dragging a bar down a long file would re-wrap the whole document on
    /// every mouse event. Landing on the line directly is O(1), and it is the unit
    /// [`visible_lines`](Self::visible_lines) already reports the extent in.
    ///
    /// The continuation row is cleared: a position expressed in buffer lines says
    /// nothing about where inside a wrapped line to sit, and keeping a stale subrow
    /// would leave the view a few rows off wherever the drag landed.
    pub fn scroll_to_line(&mut self, line: u32) {
        self.follow_cursor = false;
        self.scroll_line = line;
        self.scroll_subrow = 0;
    }

    /// Scroll so `col` is the leftmost column, without moving the caret.
    ///
    /// Soft-wrapped views have no horizontal axis and stay pinned at zero, matching
    /// [`scroll_columns`](Self::scroll_columns).
    pub fn scroll_to_column(&mut self, col: u32) {
        self.follow_cursor = false;
        self.scroll_col = if self.last_word_wrap { 0 } else { col };
    }

    /// How many distinct buffer lines the last render showed.
    ///
    /// With soft wrapping on this is smaller than the row count, because a wrapped
    /// line occupies several rows. Reporting the vertical scroll extent in buffer
    /// lines — for the total *and* the viewport — is what keeps it O(1): a
    /// visual-row total would mean re-wrapping the whole document every frame, since
    /// the wrap depends on the width and the buffer changes on every keystroke.
    #[must_use]
    pub const fn visible_lines(&self) -> u32 {
        self.last_visible_lines
    }

    /// The text width the last render had after the line-number gutter, in cells.
    ///
    /// This is the horizontal *viewport*, the counterpart to
    /// [`longest_col`](Self::longest_col): the pane's own width includes the gutter,
    /// which never scrolls.
    #[must_use]
    pub const fn content_width(&self) -> u16 {
        self.last_content_width
    }

    /// The longest line seen so far — the horizontal scroll extent, in the
    /// buffer-column units of [`scroll_col`](Self::scroll_col).
    ///
    /// Measured so that `longest_col + 1 - content_width` is the scroll that
    /// shows the widest line's last cell, tabs, wide characters and inlay hints
    /// included; for plain text it is the longest line's length in `char`s.
    ///
    /// A high-water mark rather than a live measurement: an exact answer costs a
    /// full-document scan (every line's width), which is affordable on a scroll
    /// event but not on every frame. It is exact right after
    /// [`scroll_columns`](Self::scroll_columns) and never *under*-reports, so a
    /// scrollbar built on it never claims the whole line is on screen when it is not.
    #[must_use]
    pub const fn longest_col(&self) -> u32 {
        self.last_longest_col
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

    /// The screen cells the last render painted the
    /// [`InlineText`](karet_core::DecorationKind::InlineText) decoration at
    /// `index` of the slice given to [`Editor::decorations`] in, clipped to
    /// the editor area; `None` when it painted none of it (off screen, or not
    /// inline text).
    ///
    /// Read back from the painted spans rather than re-derived, so a hit
    /// region built from it covers exactly the text the user sees — after
    /// any inlay hint or fold marker the line ends with. A decoration over
    /// several lines reports its first painted row.
    #[must_use]
    pub fn inline_text_rect(&self, index: usize) -> Option<Rect> {
        self.last_inline_text
            .iter()
            .find_map(|&(painted, rect)| (painted == index).then_some(rect))
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

    /// Apply `motion` to every caret, then merge carets that collide and keep the
    /// primary head in view.
    ///
    /// With `extend` each head moves and its anchor stays, so the selection grows
    /// or shrinks (Shift + motion). Without it each selection first collapses to
    /// its edge in the motion's direction (its start for a backward motion, its
    /// end for a forward one) and the caret moves from there; [`Motion::Left`]
    /// and [`Motion::Right`] stop at that edge. Carets are moved independently,
    /// so two carets merge only when they end on the same position.
    pub fn move_carets(&mut self, buffer: &TextBuffer, motion: Motion, extend: bool) {
        let page = u32::from(self.last_height.max(1));
        let backward = motion.is_backward();
        for s in &mut self.cursors.selections {
            if extend {
                s.head = step(buffer, motion, page, s.head);
            } else if s.is_empty() {
                *s = Selection::caret(step(buffer, motion, page, s.head));
            } else {
                let range = s.range();
                let edge = if backward { range.start } else { range.end };
                let to = match motion {
                    Motion::Left | Motion::Right => edge,
                    _ => step(buffer, motion, page, edge),
                };
                *s = Selection::caret(to);
            }
        }
        self.normalize();
        let head = self.cursor();
        self.reveal(buffer, head);
    }

    /// Sort and merge the cursor set, following the first-placed caret.
    fn normalize(&mut self) {
        self.original = self.cursors.normalize_tracking(self.original);
    }

    /// Add `sel` as the new primary selection, then merge it into the set.
    fn push(&mut self, sel: Selection) {
        let len = self.cursors.selections.len();
        self.original = self.original.min(len.saturating_sub(1));
        self.cursors.selections.push(sel);
        self.cursors.primary = len;
        self.normalize();
    }

    /// Move every caret down one line, keeping each anchor
    /// (`move_carets(buffer, Motion::Down, true)`).
    pub fn move_down(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::Down, true);
    }

    /// Move every caret up one line, keeping each anchor.
    pub fn move_up(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::Up, true);
    }

    /// Move every caret left one column, wrapping to the previous line's end and
    /// keeping each anchor.
    pub fn move_left(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::Left, true);
    }

    /// Move every caret right one column, wrapping to the next line's start and
    /// keeping each anchor.
    pub fn move_right(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::Right, true);
    }

    /// Move every caret down one page, keeping each anchor.
    pub fn page_down(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::PageDown, true);
    }

    /// Move every caret up one page, keeping each anchor.
    pub fn page_up(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::PageUp, true);
    }

    /// Move every caret to the start of its line (column 0), keeping each anchor.
    pub fn move_line_start(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::LineStart, true);
    }

    /// Move every caret to the end of its line, keeping each anchor.
    pub fn move_line_end(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::LineEnd, true);
    }

    /// Move every caret to the start of the document, keeping each anchor.
    pub fn move_doc_start(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::DocStart, true);
    }

    /// Move every caret to the end of the document, keeping each anchor.
    pub fn move_doc_end(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::DocEnd, true);
    }

    /// Move every caret to the start of the previous word (wrapping across
    /// lines), keeping each anchor.
    pub fn move_word_left(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::WordLeft, true);
    }

    /// Move every caret to the end of the next word (wrapping across lines),
    /// keeping each anchor.
    pub fn move_word_right(&mut self, buffer: &TextBuffer) {
        self.move_carets(buffer, Motion::WordRight, true);
    }

    /// Select the entire buffer as a single selection, caret at the end (Ctrl+A).
    ///
    /// Does not scroll: the selection covers whatever is on screen, so the view
    /// the user selected from stays put, and any reveal still owed by an
    /// earlier move is dropped with the caret it was for.
    pub fn select_all(&mut self, buffer: &TextBuffer) {
        let last = last_line(buffer);
        let end = LineCol::new(last, line_len(buffer, last));
        self.set_single(Selection {
            anchor: LineCol::new(0, 0),
            head: end,
        });
        self.follow_cursor = false;
        self.follow_col = None;
    }

    /// Jump the caret to `pos` (clamped), collapsing to a single bare caret there.
    /// Used to place the caret at a target (search match, go-to-line).
    pub fn goto(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        self.set_single(Selection::caret(p));
        self.reveal(buffer, p);
    }

    /// Collapse every selection to a bare caret at its head (a non-extending motion).
    pub fn clear_selection(&mut self) {
        for s in &mut self.cursors.selections {
            s.anchor = s.head;
        }
        self.normalize();
    }

    /// Place the caret at `pos` (clamped), collapsing to a single bare caret there and
    /// clearing any selection or secondary carets.
    ///
    /// The pointer placement (a click): it scrolls only when `pos` is off
    /// screen, so the text under the pointer never moves. Use
    /// [`goto`](Self::goto) to jump to a target with the keyboard margin.
    pub fn set_caret(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        self.set_single(Selection::caret(p));
        self.upstream = self.pointer_upstream.take().filter(|hit| *hit == p);
        self.reveal_with(buffer, p, Reveal::Minimal);
    }

    /// Replace the cursor set with a single bare caret at exactly `pos` (no clamping);
    /// used after applying an edit and by tests.
    pub fn place_caret(&mut self, pos: LineCol) {
        self.set_single(Selection::caret(pos));
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
        self.normalize();
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
        self.cursors = cursors;
        self.normalize();
        self.reveal(buffer, self.cursor());
    }

    /// Extend the primary selection so its moving end is `pos` (clamped), keeping the
    /// primary anchor fixed and leaving any secondary carets in place.
    ///
    /// A pointer move (Shift+click, drag): it scrolls only when `pos` is off
    /// screen.
    pub fn extend_to(&mut self, buffer: &TextBuffer, pos: LineCol) {
        let p = clamp_to_buffer(buffer, pos);
        if let Some(s) = self.cursors.selections.get_mut(self.cursors.primary) {
            s.head = p;
        }
        self.normalize();
        self.upstream = self.pointer_upstream.take().filter(|hit| *hit == p);
        self.reveal_with(buffer, self.cursor(), Reveal::Minimal);
    }

    /// Collapse to a single selection spanning `anchor`..`head` (both clamped), with
    /// the caret at `head`. Used by double/triple-click word/line selection.
    ///
    /// A pointer move: it scrolls only when `head` is off screen, so a
    /// double-click's second click lands on the word the first one hit.
    pub fn set_selection(&mut self, buffer: &TextBuffer, anchor: LineCol, head: LineCol) {
        let anchor = clamp_to_buffer(buffer, anchor);
        let head = clamp_to_buffer(buffer, head);
        self.set_single(Selection { anchor, head });
        self.upstream = None;
        self.reveal_with(buffer, head, Reveal::Minimal);
    }

    /// Collapse the cursor set to just the primary selection, keeping its
    /// selection (Shift+click extends from it).
    pub fn collapse_to_primary(&mut self) {
        self.cursors.collapse_to_primary();
        self.original = 0;
    }

    /// Collapse the cursor set to the first-placed caret, keeping its selection,
    /// and keep it in view (`Esc` with several carets). That caret becomes the
    /// primary.
    pub fn collapse_to_original(&mut self, buffer: &TextBuffer) {
        let kept = self
            .cursors
            .selections
            .get(self.original)
            .copied()
            .unwrap_or_else(|| self.cursors.primary());
        self.set_single(kept);
        self.reveal(buffer, kept.head);
    }

    /// Replace the cursor set with the one selection `sel`.
    fn set_single(&mut self, sel: Selection) {
        self.cursors = CursorState::single(sel);
        self.original = 0;
    }

    /// Add a bare caret one line above the primary (column clamped to the shorter
    /// line), merging if it collides. A no-op when the primary is already on line 0.
    pub fn add_caret_above(&mut self, buffer: &TextBuffer) {
        let h = self.cursor();
        if h.line == 0 {
            return;
        }
        let p = clamp_to_buffer(buffer, LineCol::new(h.line - 1, h.col));
        self.push(Selection::caret(p));
        self.reveal(buffer, self.cursor());
    }

    /// Add a bare caret one line below the primary. A no-op on the last line.
    pub fn add_caret_below(&mut self, buffer: &TextBuffer) {
        let h = self.cursor();
        if h.line >= last_line(buffer) {
            return;
        }
        let p = clamp_to_buffer(buffer, LineCol::new(h.line + 1, h.col));
        self.push(Selection::caret(p));
        self.reveal(buffer, self.cursor());
    }

    /// Toggle a caret at `pos` (Alt+click): remove a coincident bare caret unless it is
    /// the only one, otherwise add one as the new primary. A pointer move: it
    /// scrolls only when `pos` is off screen.
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
            // Every index past the removed caret shifts down one, so the primary
            // and the first-placed caret stay the carets they were. Removing one of
            // them hands its role to the caret before it (or after it, at the start).
            let shift = |index: usize| {
                if i <= index {
                    index.saturating_sub(1)
                } else {
                    index
                }
            };
            self.cursors.primary = shift(self.cursors.primary);
            self.original = shift(self.original);
            return;
        }
        self.push(Selection::caret(p));
        self.upstream = self.pointer_upstream.take().filter(|hit| *hit == p);
        self.reveal_with(buffer, p, Reveal::Minimal);
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
            self.reveal(buffer, self.cursor());
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
            self.push(Selection {
                anchor: start,
                head: end,
            });
            self.reveal(buffer, self.cursor());
        }
    }

    /// The buffer line under `(col, row)` when the cell lies in the gutter's
    /// leading marker column (the breakpoint lane, left of the line numbers);
    /// `None` anywhere else. Row math matches [`pos_at`](Self::pos_at), so
    /// folds, word wrap, and sticky lines resolve identically.
    #[must_use]
    pub fn gutter_marker_line_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> Option<u32> {
        (col == area.x).then(|| self.pos_at(area, buffer, folds, col, row).line)
    }

    /// The buffer position under the screen cell `(col, row)`, given the editor's
    /// render `area` and the `folds` in effect. Accounts for the gutter width, the
    /// scroll offsets, and any collapsed folds that hide lines between the viewport
    /// top and the click.
    ///
    /// A cell inside a wide character or a tab resolves to the nearer of its
    /// two edges, and a cell below the last row resolves to the end of the
    /// last line shown.
    #[must_use]
    pub fn pos_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> LineCol {
        self.pointer_upstream.set(None);
        self.hit(area, buffer, folds, (col, row), Snap::Nearest)
    }

    /// The buffer position of the character under the screen cell
    /// `(col, row)`: like [`pos_at`](Self::pos_at), except that a cell inside
    /// a wide character or a tab resolves to that character's own column
    /// rather than the nearer of its edges. This is what the pointer is
    /// *over* — the position word selection, a word-drag anchor, and
    /// anything that looks up the symbol under the pointer should use — where
    /// `pos_at` is where a click puts the caret.
    #[must_use]
    pub fn char_pos_at(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        col: u16,
        row: u16,
    ) -> LineCol {
        self.hit(area, buffer, folds, (col, row), Snap::Containing)
    }

    /// The hit-test behind [`pos_at`](Self::pos_at) and
    /// [`char_pos_at`](Self::char_pos_at). Only a [`Snap::Nearest`] hit
    /// records the wrap-row affinity, since only a caret placement consumes it.
    fn hit(
        &self,
        area: Rect,
        buffer: &TextBuffer,
        folds: &[Fold],
        (col, row): (u16, u16),
        snap: Snap,
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
            let chars = line_chars(buffer, line);
            // Measured exactly as `draw_sticky_row` paints it: from
            // `scroll_col`, tabs expanded, and unhinted.
            return LineCol::new(
                line,
                source_col_at_display_offset(
                    &chars,
                    self.scroll_col,
                    chars.len() as u32,
                    rel_col,
                    self.last_tab_width,
                    LineHints::EMPTY,
                    snap,
                ),
            );
        }
        rel_row = rel_row.saturating_sub(u32::from(self.sticky_height));
        if self.last_word_wrap {
            let width = u32::from(area.width.saturating_sub(gutter).max(1));
            let layout = self.layout(width);
            let (anchor, below_end) = visual_anchor_at_row(
                buffer,
                folds,
                layout,
                VisualAnchor {
                    line: self.scroll_line,
                    subrow: self.scroll_subrow,
                },
                rel_row,
            );
            if below_end {
                return LineCol::new(anchor.line, line_len(buffer, anchor.line));
            }
            let ranges = visual_ranges(buffer, anchor.line, layout);
            let range = ranges
                .get(anchor.subrow as usize)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(line_len(buffer, anchor.line)));
            let chars = line_chars(buffer, anchor.line);
            let hints = layout.hints_on(anchor.line);
            let hit = LineCol::new(
                anchor.line,
                source_col_at_display_offset(
                    &chars,
                    range.start,
                    range.end,
                    rel_col.saturating_sub(row_lead(hints, anchor.subrow == 0)),
                    self.last_tab_width,
                    hints,
                    snap,
                ),
            );
            // Past the end of a row the line continues below: the same column
            // starts the next row, so remember which one was clicked.
            let continues = (anchor.subrow as usize).saturating_add(1) < ranges.len();
            if snap == Snap::Nearest {
                self.pointer_upstream
                    .set((continues && hit.col == range.end).then_some(hit));
            }
            return hit;
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
                // Below the last row: the end of the document, as shown.
                let line = line.min(line_count - 1);
                return LineCol::new(line, line_len(buffer, line));
            }
            line = next;
        }
        let line = line.min(line_count - 1);
        let chars = line_chars(buffer, line);
        let hints = self.hints_on(line);
        LineCol::new(
            line,
            source_col_at_display_offset(
                &chars,
                self.scroll_col,
                chars.len() as u32,
                rel_col.saturating_sub(row_lead(hints, true)),
                self.last_tab_width,
                hints,
                snap,
            ),
        )
    }
}
