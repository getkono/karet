use std::cell::Cell;
use std::ops::RangeInclusive;

use super::hint::BlockIndex;
use super::hint::HintIndex;
use super::hint::LineHints;
use super::text::*;
use super::visual::*;
use super::*;

mod gutter;
mod motion;
mod syntax_select;

pub use gutter::GutterHit;
pub use gutter::GutterLane;
pub use motion::Motion;

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
    /// The gutter numbering the last render used, which sets the gutter's
    /// width for hit-testing and caret placement.
    pub(super) last_line_numbers: LineNumbers,
    /// Whether the last render reserved the gutter's change lane, which
    /// widens the gutter by one column.
    pub(super) last_change_lane: bool,
    /// The keyboard reveal's vertical margin the last render was given.
    pub(super) last_scroll_off: u16,
    /// The buffer's last line at the last render, where a margin stops.
    pub(super) last_line: u32,
    /// Whether the last render used soft wrapping.
    pub(super) last_word_wrap: bool,
    /// Hard-tab width captured at the last render.
    pub(super) last_tab_width: u16,
    /// Logical-line ranges exempted from soft wrapping at the last render.
    pub(super) last_unwrapped_lines: Vec<RangeInclusive<u32>>,
    /// Virtual text captured at the last render, indexed for lookup.
    ///
    /// Cached rather than recomputed because a mouse click resolves outside a
    /// render, and it must land against the geometry actually on screen. A
    /// click resolved against hints the frame did not paint would select the
    /// wrong character.
    pub(super) last_hints: HintIndex,
    /// Virtual lines captured at the last render, for the same reason as
    /// [`last_hints`](Self::last_hints): the rows a click counts are the rows
    /// that frame painted.
    pub(super) last_blocks: BlockIndex,
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
    /// The heads placed at the end of a wrapped row rather than the start of
    /// the next: that position is also where the next row starts, and these
    /// are drawn at the end of the row they were placed on. A pointer sets it
    /// for the primary head it placed past the end of a row; End and vertical
    /// motion set it for every caret they leave at a row's end. Dropped by any
    /// other keyboard move, and by the render for a head that has moved
    /// elsewhere.
    pub(super) upstream: Vec<LineCol>,
    /// The goal columns vertical motion steers each caret back toward — see
    /// [`motion::Goals`].
    pub(super) goals: Option<motion::Goals>,
    /// What the last [`pos_at`](Self::pos_at) resolved, when it resolved past
    /// the end of a wrapped row, so the pointer move that consumes it
    /// ([`set_caret`](Self::set_caret), [`extend_to`](Self::extend_to)) can
    /// keep the caret on that row. A `Cell` because hit-testing is `&self`.
    pub(super) pointer_upstream: Cell<Option<LineCol>>,
    /// Where the last render painted each virtual text, keyed by its index
    /// in the slice given to `Editor::virtual_text` — see
    /// [`virtual_text_rect`](Self::virtual_text_rect).
    pub(super) last_virtual_text: Vec<(usize, Rect)>,
    /// The row the last render painted each virtual line on, keyed by its
    /// index in the slice given to `Editor::virtual_lines` — see
    /// [`virtual_line_at`](Self::virtual_line_at).
    pub(super) last_virtual_lines: Vec<(usize, Rect)>,
    /// Where the last render painted each collapsed fold header's ` ⋯`,
    /// with that header's line — see
    /// [`fold_ellipsis_at`](Self::fold_ellipsis_at).
    pub(super) last_fold_ellipses: Vec<(Rect, u32)>,
    /// The selection set the last whole-word `Ctrl+D` press left — see
    /// [`add_next_occurrence`](Self::add_next_occurrence). The next press keeps
    /// matching whole words only while the set is still exactly this one.
    pub(super) word_occurrences: Option<Vec<Selection>>,
    /// The Expand Selection presses Shrink Selection can still walk back,
    /// oldest first — see [`expand_selection`](Self::expand_selection).
    pub(super) syntax_steps: Vec<syntax_select::SyntaxStep>,
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
            last_line_numbers: LineNumbers::Absolute,
            last_change_lane: false,
            last_scroll_off: 0,
            last_line: u32::MAX,
            last_word_wrap: false,
            last_tab_width: 4,
            last_unwrapped_lines: Vec::new(),
            last_hints: HintIndex::default(),
            last_blocks: BlockIndex::default(),
            follow_cursor: false,
            follow_col: None,
            last_folds: Vec::new(),
            sticky_rows: Vec::new(),
            sticky_height: 0,
            last_visible_lines: 0,
            last_longest_col: 0,
            upstream: Vec::new(),
            goals: None,
            pointer_upstream: Cell::new(None),
            last_virtual_text: Vec::new(),
            last_virtual_lines: Vec::new(),
            last_fold_ellipses: Vec::new(),
            word_occurrences: None,
            syntax_steps: Vec::new(),
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
        self.follow_col = Some((pos, Reveal::Margin));
        self.scroll_vertically_to(pos, self.last_line);
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
    /// lines take several, sticky headers shrink the viewport — keeping the
    /// rows [`Editor::scroll_off`] asks for between it and either edge, so a
    /// line already inside that margin never scrolls. The estimate made here
    /// uses the last frame's folds, height and margin; the render completes
    /// it against the ones it paints.
    ///
    /// The pointer-originated moves — [`set_caret`](Self::set_caret),
    /// [`extend_to`](Self::extend_to), [`set_selection`](Self::set_selection)
    /// and [`add_caret`](Self::add_caret) — keep no margin instead: they
    /// scroll only when the target is off screen.
    pub fn reveal(&mut self, buffer: &TextBuffer, pos: LineCol) {
        // A keyboard-placed caret at a wrap point opens the next row.
        self.upstream.clear();
        self.reveal_with(buffer, pos, Reveal::Margin);
    }

    /// [`reveal`](Self::reveal) keeping the horizontal margin `reveal` asks for.
    fn reveal_with(&mut self, buffer: &TextBuffer, pos: LineCol, reveal: Reveal) {
        // Re-resolved by the render whatever happens here: the last frame's
        // width may not be the next one's (a resize, a split, a background tab
        // brought forward), and a wrapped frame may be followed by an
        // unwrapped one. Set first, as it decides the vertical margin.
        self.follow_col = Some((pos, reveal));
        self.scroll_vertically_to(pos, last_line(buffer));
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
    fn scroll_vertically_to(&mut self, pos: LineCol, last: u32) {
        self.follow_cursor = true;
        let height = u32::from(self.last_height.max(1));
        // A wrapped line, or a line with virtual lines, takes several rows, so
        // a margin counted in lines could scroll further than the rows the
        // margin asks for; the render keeps it in rows instead.
        let off = if self.last_word_wrap || !self.last_blocks.is_empty() {
            0
        } else {
            self.reveal_margin()
        };
        let margin = (off, last);
        let top = reveal_line(&self.last_folds, self.scroll_line, pos.line, height, margin);
        if top != self.scroll_line {
            self.scroll_line = top;
            self.scroll_subrow = 0;
        }
    }

    /// The vertical margin the pending reveal keeps: the editor's
    /// `scroll_off` for a keyboard reveal, none for a pointer-originated one.
    pub(super) fn reveal_margin(&self) -> u32 {
        match self.follow_col {
            Some((_, Reveal::Minimal)) => 0,
            _ => u32::from(self.last_scroll_off),
        }
    }

    /// The geometry of the last render at `width` content columns, soft
    /// wrapped when `wrap`, else one text row per line.
    pub(super) fn row_layout(&self, width: u32, wrap: bool) -> Layout<'_> {
        Layout {
            width,
            tab_width: self.last_tab_width,
            unwrapped_lines: &self.last_unwrapped_lines,
            hints: &self.last_hints,
            blocks: &self.last_blocks,
            wrap,
        }
    }

    /// The inline virtual text the last render painted on `line`.
    pub(super) fn hints_on(&self, line: u32) -> LineHints<'_> {
        self.last_hints.line(line)
    }

    /// Scroll the viewport vertically by display rows.
    ///
    /// In overflow mode a display row is one buffer line or one of its
    /// virtual lines. In soft-wrap mode this also walks continuation rows
    /// before advancing to the next visible buffer line. A collapsed fold's
    /// hidden lines take no rows in either.
    pub fn scroll_rows(
        &mut self,
        buffer: &TextBuffer,
        folds: &[Fold],
        word_wrap: bool,
        delta: i32,
    ) {
        self.follow_cursor = false;
        let width = u32::from(self.last_content_width.max(1));
        let layout = self.row_layout(width, word_wrap);
        let mut anchor = VisualAnchor {
            line: self.scroll_line.min(last_line(buffer)),
            subrow: self.scroll_subrow,
        };
        let steps = delta.unsigned_abs();
        for _ in 0..steps {
            let next = if delta.is_negative() {
                previous_visual_anchor(buffer, folds, layout, anchor)
            } else {
                next_visual_anchor(buffer, folds, layout, anchor)
            };
            if next == anchor {
                break;
            }
            anchor = next;
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
    /// [`VirtualText`](karet_core::VirtualText) at `index` of the slice given
    /// to [`Editor::virtual_text`] in, clipped to the editor area; `None`
    /// when it painted none of it (off screen, scrolled past, or left out of
    /// a full end-of-line slot). End-of-line text cut short reports the cells
    /// of what was painted, `…` included.
    ///
    /// Read back from the painted spans rather than re-derived, so a hit
    /// region built from it covers exactly the text the user sees — after
    /// any inline text or fold marker the line ends with.
    #[must_use]
    pub fn virtual_text_rect(&self, index: usize) -> Option<Rect> {
        self.last_virtual_text
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

    /// Go to Bracket: move each caret to the other bracket of the pair it
    /// touches, or, inside a pair, to that pair's closing bracket; a caret with
    /// no pair stays put. The caret lands *on* the target bracket (just before
    /// it), so pressing again toggles between the two ends. Brackets inside
    /// strings and comments, per `highlights`, are ignored.
    pub fn go_to_bracket(&mut self, buffer: &TextBuffer, highlights: Option<&Highlights>) {
        for s in &mut self.cursors.selections {
            let head = s.head;
            if let Some(pair) = crate::bracket_pair_at(buffer, highlights, head) {
                let at_close = head == pair.close || head == pair.outer().end;
                *s = Selection::caret(if at_close { pair.open } else { pair.close });
            }
        }
        self.normalize();
        self.reveal(buffer, self.cursor());
    }

    /// Select to Bracket: select each caret's pair — the one it touches, else
    /// the one enclosing it — brackets included, with the caret after the
    /// closing bracket. A selection that already covers exactly its pair grows
    /// to the enclosing one, so repeating the command widens outward.
    pub fn select_to_bracket(&mut self, buffer: &TextBuffer, highlights: Option<&Highlights>) {
        for s in &mut self.cursors.selections {
            let range = s.range();
            let pair = match crate::matching_pair(buffer, highlights, range.start) {
                Some(own) if !s.is_empty() && own.outer() == range => {
                    crate::enclosing_pair(buffer, highlights, own.open)
                },
                _ => crate::bracket_pair_at(buffer, highlights, s.head),
            };
            if let Some(pair) = pair {
                let outer = pair.outer();
                *s = Selection {
                    anchor: outer.start,
                    head: outer.end,
                };
            }
        }
        self.normalize();
        self.reveal(buffer, self.cursor());
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
        self.pointer_moved(p);
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
        self.goals = None;
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
        self.goals = None;
        self.normalize();
        self.reveal(buffer, self.cursor());
    }

    /// Carry every anchor and head through a change made elsewhere (another
    /// view, a formatter, a reload): `map` takes a position in the text before
    /// the change to one in `buffer`, the text after it, and each result is
    /// then clamped to `buffer`. Carets that land together merge, keeping the
    /// primary.
    ///
    /// The viewport does not move: the change was not this view's doing, so
    /// it must not scroll the text out from under the reader.
    pub fn map_cursors(&mut self, buffer: &TextBuffer, map: impl Fn(LineCol) -> LineCol) {
        for selection in &mut self.cursors.selections {
            selection.anchor = clamp_to_buffer(buffer, map(selection.anchor));
            selection.head = clamp_to_buffer(buffer, map(selection.head));
        }
        // A pointer hint, a goal or an occurrence run names positions in the
        // old text.
        self.upstream.clear();
        self.goals = None;
        self.word_occurrences = None;
        self.syntax_steps.clear();
        self.normalize();
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
        self.pointer_moved(p);
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
        self.upstream.clear();
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
        self.goals = None;
    }

    /// Record what a pointer move to `p` leaves: the wrap-row affinity the
    /// hit-test found for it, and no goal column (the pointer chose the
    /// column).
    fn pointer_moved(&mut self, p: LineCol) {
        self.upstream = self
            .pointer_upstream
            .take()
            .filter(|hit| *hit == p)
            .into_iter()
            .collect();
        self.goals = None;
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
        self.pointer_moved(p);
        self.reveal_with(buffer, p, Reveal::Minimal);
    }

    /// `Ctrl+D`: if the primary is a bare caret, select the word touching it (nothing
    /// happens off a word); otherwise add a selection over the next occurrence of the
    /// primary selection's text.
    ///
    /// The search runs forward from the primary — the most recently added match —
    /// wrapping at the end of the buffer and skipping occurrences already selected, so
    /// every occurrence is reached once and a press after the last one is a no-op.
    /// A run started from a bare caret matches whole words only; one started from a
    /// selection matches the text anywhere, inside words included.
    pub fn add_next_occurrence(&mut self, buffer: &TextBuffer) {
        let primary = self.cursors.primary();
        if primary.is_empty() {
            if let Some((anchor, head)) = word_at(buffer, primary.head) {
                if let Some(s) = self.cursors.selections.get_mut(self.cursors.primary) {
                    *s = Selection { anchor, head };
                }
                self.normalize();
                self.word_occurrences = Some(self.cursors.selections.clone());
                self.reveal(buffer, self.cursor());
            }
            return;
        }
        // Whole-word only while the set is still exactly what the last whole-word
        // press left: any other change means the user chose this selection.
        let whole_word = self
            .word_occurrences
            .as_ref()
            .is_some_and(|set| *set == self.cursors.selections);
        let needle = range_text(buffer, primary.range());
        let selections = &self.cursors.selections;
        let taken = |r: Range| {
            selections.iter().any(|s| {
                let s = s.range();
                s.start != s.end && s.start < r.end && r.start < s.end
            })
        };
        if let Some(found) = find_next(buffer, &needle, primary.range().end, whole_word, taken) {
            self.push(Selection {
                anchor: found.start,
                head: found.end,
            });
            self.reveal(buffer, self.cursor());
        }
        self.word_occurrences = whole_word.then(|| self.cursors.selections.clone());
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
        let gutter = gutter_width(line_count, self.last_line_numbers, self.last_change_lane);
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
            // `scroll_col`, tabs and inline virtual text expanded.
            return LineCol::new(
                line,
                source_col_at_display_offset(
                    &chars,
                    self.scroll_col,
                    chars.len() as u32,
                    rel_col,
                    self.last_tab_width,
                    self.hints_on(line),
                    snap,
                ),
            );
        }
        rel_row = rel_row.saturating_sub(u32::from(self.sticky_height));
        let width = u32::from(area.width.saturating_sub(gutter).max(1));
        let layout = self.row_layout(width, self.last_word_wrap);
        let (anchor, below_end) = self.row_at(buffer, folds, layout, rel_row);
        // Below the last row: the end of the document, as shown.
        if below_end {
            return LineCol::new(anchor.line, line_len(buffer, anchor.line));
        }
        let ranges = layout.text_rows(buffer, anchor.line, self.scroll_col);
        let index = match layout.kind(anchor.line, anchor.subrow, ranges.len() as u32) {
            RowKind::Text(index) => index as usize,
            // A virtual line is routed to the line it belongs to: one above
            // it to the line's start, one below to its end.
            RowKind::Above(_) => return LineCol::new(anchor.line, 0),
            RowKind::Below(_) => {
                return LineCol::new(anchor.line, line_len(buffer, anchor.line));
            },
        };
        let range = ranges
            .get(index)
            .copied()
            .unwrap_or_else(|| VisualRange::empty(line_len(buffer, anchor.line)));
        let chars = line_chars(buffer, anchor.line);
        let hit = LineCol::new(
            anchor.line,
            source_col_at_display_offset(
                &chars,
                range.start,
                range.end,
                rel_col,
                self.last_tab_width,
                layout.hints_on(anchor.line),
                snap,
            ),
        );
        // Past the end of a row the line continues below: the same column
        // starts the next row, so remember which one was clicked.
        let continues = index.saturating_add(1) < ranges.len();
        if snap == Snap::Nearest {
            self.pointer_upstream
                .set((continues && hit.col == range.end).then_some(hit));
        }
        hit
    }

    /// The visual row `rel_row` rows below the viewport top as `layout`
    /// lays it out, walking past collapsed folds, and whether `rel_row` lies
    /// below the document's last row (the anchor is then that last row).
    pub(super) fn row_at(
        &self,
        buffer: &TextBuffer,
        folds: &[Fold],
        layout: Layout<'_>,
        rel_row: u32,
    ) -> (VisualAnchor, bool) {
        let top = VisualAnchor {
            line: self.scroll_line,
            subrow: self.scroll_subrow,
        };
        visual_anchor_at_row(buffer, folds, layout, top, rel_row)
    }

    /// The [`VirtualLine`](karet_core::VirtualLine) the last render painted
    /// on the row under the screen cell `(col, row)`, as its index in the
    /// slice given to [`Editor::virtual_lines`]; `None` off a virtual line,
    /// and in the gutter beside one.
    ///
    /// A click on a virtual line belongs to whoever supplied it (a code
    /// lens's command, a diff hunk's actions), so check this before
    /// [`pos_at`](Self::pos_at), which resolves the same cell to the line
    /// the virtual line is anchored to. Read back from what was painted, so
    /// it agrees with the screen whatever has changed since.
    #[must_use]
    pub fn virtual_line_at(&self, col: u16, row: u16) -> Option<usize> {
        let at = ratatui::layout::Position { x: col, y: row };
        self.last_virtual_lines
            .iter()
            .find_map(|&(index, rect)| rect.contains(at).then_some(index))
    }

    /// The row the last render painted the virtual line at `index` of the
    /// slice given to [`Editor::virtual_lines`] on, from the end of the
    /// gutter to the editor's right edge; `None` when it was not painted
    /// (scrolled off, or hidden in a collapsed fold).
    #[must_use]
    pub fn virtual_line_rect(&self, index: usize) -> Option<Rect> {
        self.last_virtual_lines
            .iter()
            .find_map(|&(painted, rect)| (painted == index).then_some(rect))
    }
}
