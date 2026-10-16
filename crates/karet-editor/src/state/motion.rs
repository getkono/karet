//! Caret motion: the [`Motion`] vocabulary, and how each one moves a caret
//! through the text as the last render laid it out — past collapsed folds,
//! along soft-wrapped rows, and back toward a goal column remembered in
//! display cells.

use super::*;

/// A caret motion, applied to every caret by [`EditorState::move_carets`].
///
/// Motions move through the text as the last render showed it: a collapsed
/// fold is stepped over as the one row it shows, never into, and with word
/// wrap on the vertical motions, Home and End act on visual rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Motion {
    /// One row up, toward the goal column. From the first row, to the start
    /// of the document.
    Up,
    /// One row down, toward the goal column. From the last row, to the end
    /// of the document.
    Down,
    /// One character left, wrapping to the previous line's end.
    Left,
    /// One character right, wrapping to the next line's start.
    Right,
    /// Up by the viewport's height in rows, toward the goal column; to the
    /// start of the document when fewer rows are left. The view scrolls with
    /// the primary caret, which keeps its place on screen.
    PageUp,
    /// Down by the viewport's height in rows, toward the goal column; to the
    /// end of the document when fewer rows are left. The view scrolls with
    /// the primary caret, which keeps its place on screen.
    PageDown,
    /// Smart Home: to the first non-blank character of the line, or to
    /// column 0 when already there. On a soft-wrapped continuation row, to
    /// the start of that row.
    LineStart,
    /// To the end of the line. With word wrap on, to the end of the visual
    /// row first, and from there to the end of the line.
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

    /// Whether the motion moves between rows and so steers toward, and keeps,
    /// each caret's goal column.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Up | Self::Down | Self::PageUp | Self::PageDown)
    }
}

/// The goal column of each caret the last vertical motion moved, in display
/// cells from the start of its row (tabs expanded, wide characters counted
/// twice, inlay hints not counted).
///
/// Keyed by the head each caret was left at, so a caret moved since by
/// anything else finds none and measures its column afresh, and valid only at
/// the buffer version it was taken at, so an edit resets it.
#[derive(Clone, Debug)]
pub(crate) struct Goals {
    version: u64,
    carets: Vec<(LineCol, u32)>,
}

/// Where one caret's motion took it.
#[derive(Clone, Copy, Debug)]
struct Moved {
    /// The new head.
    to: LineCol,
    /// Whether `to` is drawn at the end of its row rather than the start of
    /// the next (see [`EditorState::upstream`]).
    upstream: bool,
    /// The goal column the caret keeps, after a vertical motion.
    goal: Option<u32>,
}

impl Moved {
    /// A plain move to `to`.
    const fn to(to: LineCol) -> Self {
        Self {
            to,
            upstream: false,
            goal: None,
        }
    }
}

/// One visual row: which it is and the columns it shows.
#[derive(Clone, Copy, Debug)]
struct Row {
    anchor: VisualAnchor,
    range: VisualRange,
    /// Whether this is its line's last row.
    last: bool,
}

/// The text as the last render laid it out in rows: soft-wrapped at its
/// width, or one row per line, with collapsed folds taking one row.
struct Rows<'a> {
    buffer: &'a TextBuffer,
    folds: &'a [Fold],
    layout: Layout<'a>,
}

impl Rows<'_> {
    /// The text row `anchor` names, clamped to its line's text rows: a
    /// virtual line above them names the first, one below the last.
    fn row(&self, anchor: VisualAnchor) -> Row {
        let ranges = visual_ranges(self.buffer, anchor.line, self.layout);
        let count = ranges.len() as u32;
        let index = match self.layout.kind(anchor.line, anchor.subrow, count) {
            RowKind::Above(_) => 0,
            RowKind::Text(index) => index,
            RowKind::Below(_) => count.saturating_sub(1),
        } as usize;
        let range = ranges
            .get(index)
            .copied()
            .unwrap_or_else(|| VisualRange::empty(0));
        Row {
            anchor: VisualAnchor {
                line: anchor.line,
                subrow: self.layout.above(anchor.line) + index as u32,
            },
            range,
            last: index + 1 >= ranges.len(),
        }
    }

    /// The row a caret at `pos` is drawn on.
    fn row_of(&self, pos: LineCol, upstream: bool) -> Row {
        self.row(visual_anchor_for_position(
            self.buffer,
            self.layout,
            pos,
            upstream,
        ))
    }

    /// The display cell of `pos` within its row: the goal column it sets.
    fn cell(&self, pos: LineCol, upstream: bool) -> u32 {
        let row = self.row_of(pos, upstream);
        offset_within_row(
            &line_chars(self.buffer, pos.line),
            row.range.start,
            pos.col,
            self.layout.tab_width,
            LineHints::default(),
        )
    }

    /// The text row `rows` text rows below (`down`) or above `from`, and
    /// whether there were that many; when not, the first or last text row
    /// reached. Virtual lines are stepped over: no caret lands on one.
    fn walk(&self, from: VisualAnchor, rows: u32, down: bool) -> (VisualAnchor, bool) {
        self.walk_rows(from, rows, down, true)
    }

    /// The row `rows` rows below (`down`) or above `from`, counting only
    /// text rows when `text_only` and every row on screen otherwise, and
    /// whether there were that many.
    fn walk_rows(
        &self,
        from: VisualAnchor,
        rows: u32,
        down: bool,
        text_only: bool,
    ) -> (VisualAnchor, bool) {
        let mut at = normalize_visual_anchor(self.buffer, self.folds, self.layout, from);
        for _ in 0..rows {
            let mut next = at;
            loop {
                let step = if down {
                    next_visual_anchor(self.buffer, self.folds, self.layout, next)
                } else {
                    previous_visual_anchor(self.buffer, self.folds, self.layout, next)
                };
                if step == next {
                    return (at, false);
                }
                next = step;
                if !text_only || self.row(next).anchor == next {
                    break;
                }
            }
            at = next;
        }
        (at, true)
    }

    /// The column of the row `anchor` nearest the display cell `goal`, and
    /// whether it is that row's end where the next row of the line starts.
    fn place(&self, anchor: VisualAnchor, goal: u32) -> (LineCol, bool) {
        let row = self.row(anchor);
        let col = source_col_at_display_offset(
            &line_chars(self.buffer, anchor.line),
            row.range.start,
            row.range.end,
            goal,
            self.layout.tab_width,
            LineHints::default(),
            Snap::Nearest,
        );
        (
            LineCol::new(anchor.line, col),
            !row.last && col == row.range.end,
        )
    }

    /// `pos`, or the nearest position a collapsed fold does not hide it
    /// behind: past the fold for a forward motion (the header's end when the
    /// fold runs to the end of the document), its header's end otherwise.
    fn shown(&self, mut pos: LineCol, forward: bool) -> LineCol {
        let last = last_line(self.buffer);
        // Each pass leaves the outermost fold hiding `pos`, so one per fold
        // is enough; the bound is a backstop.
        for _ in 0..=self.folds.len() {
            let Some(fold) = self
                .folds
                .iter()
                .filter(|f| f.collapsed && f.start < pos.line && pos.line <= f.end)
                .min_by_key(|f| f.start)
            else {
                break;
            };
            pos = if forward && fold.end < last {
                LineCol::new(fold.end + 1, 0)
            } else {
                LineCol::new(fold.start, line_len(self.buffer, fold.start))
            };
        }
        pos
    }

    /// Where `motion` takes a caret at `from`, with `page` rows to a page and
    /// `goals` the goal columns still in effect.
    fn apply(
        &self,
        motion: Motion,
        page: u32,
        (from, upstream): (LineCol, bool),
        goals: &[(LineCol, u32)],
    ) -> Moved {
        let goal = || {
            goals
                .iter()
                .find(|(at, _)| *at == from)
                .map_or_else(|| self.cell(from, upstream), |(_, goal)| *goal)
        };
        match motion {
            Motion::Up => self.vertical(from, upstream, goal(), (1, false)),
            Motion::Down => self.vertical(from, upstream, goal(), (1, true)),
            Motion::PageUp => self.vertical(from, upstream, goal(), (page, false)),
            Motion::PageDown => self.vertical(from, upstream, goal(), (page, true)),
            Motion::LineStart => Moved::to(self.home(from, upstream)),
            Motion::LineEnd => self.end(from, upstream),
            _ => Moved::to(self.shown(step(self.buffer, motion, from), !motion.is_backward())),
        }
    }

    /// `rows` rows down or up from `from` toward `goal`; past the first or
    /// last row, to the start or end of the document.
    fn vertical(
        &self,
        from: LineCol,
        upstream: bool,
        goal: u32,
        (rows, down): (u32, bool),
    ) -> Moved {
        let (anchor, complete) = self.walk(self.row_of(from, upstream).anchor, rows, down);
        let (to, upstream) = if complete {
            self.place(anchor, goal)
        } else if down {
            let last = last_line(self.buffer);
            let end = LineCol::new(last, line_len(self.buffer, last));
            (self.shown(end, false), false)
        } else {
            (LineCol::new(0, 0), false)
        };
        Moved {
            to,
            upstream,
            goal: Some(goal),
        }
    }

    /// Smart Home from `from`.
    fn home(&self, from: LineCol, upstream: bool) -> LineCol {
        let row = self.row_of(from, upstream);
        if row.anchor.subrow > 0 {
            return LineCol::new(from.line, row.range.start);
        }
        // A blank line has no first non-blank character, so Home goes to
        // column 0 there.
        let indent = line_chars(self.buffer, from.line)
            .iter()
            .position(|ch| !ch.is_whitespace())
            .map_or(0, |index| index as u32);
        LineCol::new(from.line, if from.col == indent { 0 } else { indent })
    }

    /// End from `from`: the end of its row, or of the line from a row's end.
    fn end(&self, from: LineCol, upstream: bool) -> Moved {
        let row = self.row_of(from, upstream);
        if row.last || from.col == row.range.end {
            Moved::to(LineCol::new(from.line, line_len(self.buffer, from.line)))
        } else {
            Moved {
                to: LineCol::new(from.line, row.range.end),
                upstream: true,
                goal: None,
            }
        }
    }
}

/// Where a motion that does not depend on the layout takes a caret at `from`.
/// [`Motion::Left`] and [`Motion::Right`] step a whole grapheme cluster, so a
/// caret never lands inside a combining sequence, a ZWJ emoji or a flag.
fn step(buffer: &TextBuffer, motion: Motion, from: LineCol) -> LineCol {
    let last = last_line(buffer);
    match motion {
        Motion::Left if from.col > 0 => LineCol::new(from.line, previous_cluster_col(buffer, from)),
        Motion::Left if from.line > 0 => {
            LineCol::new(from.line - 1, line_len(buffer, from.line - 1))
        },
        Motion::Right if from.col < line_len(buffer, from.line) => {
            LineCol::new(from.line, next_cluster_col(buffer, from))
        },
        Motion::Right if from.line < last => LineCol::new(from.line + 1, 0),
        Motion::DocStart => LineCol::new(0, 0),
        Motion::DocEnd => LineCol::new(last, line_len(buffer, last)),
        Motion::WordLeft => previous_word_boundary(buffer, from),
        Motion::WordRight => next_word_boundary(buffer, from),
        _ => from,
    }
}

impl EditorState {
    /// The rows the last render laid `buffer` out in.
    fn rows<'a>(&'a self, buffer: &'a TextBuffer) -> Rows<'a> {
        Rows {
            buffer,
            folds: &self.last_folds,
            layout: self.row_layout(
                u32::from(self.last_content_width.max(1)),
                self.last_word_wrap,
            ),
        }
    }

    /// The goal columns still in effect for `buffer`: none once it has been
    /// edited since they were taken.
    fn live_goals(&self, buffer: &TextBuffer) -> &[(LineCol, u32)] {
        self.goals
            .as_ref()
            .filter(|goals| goals.version == buffer.version())
            .map_or(&[], |goals| goals.carets.as_slice())
    }

    /// How many rows below the top of the viewport the caret at `pos` is
    /// drawn, when it is on screen.
    fn screen_row(&self, rows: &Rows<'_>, pos: LineCol) -> Option<u32> {
        let target = rows.row_of(pos, self.upstream.contains(&pos)).anchor;
        let top = VisualAnchor {
            line: self.scroll_line,
            subrow: self.scroll_subrow,
        };
        let mut at = normalize_visual_anchor(rows.buffer, rows.folds, rows.layout, top);
        for row in 0..u32::from(self.last_height) {
            if at == target {
                return Some(row);
            }
            let next = next_visual_anchor(rows.buffer, rows.folds, rows.layout, at);
            if next == at {
                break;
            }
            at = next;
        }
        None
    }

    /// Apply `motion` to every caret, then merge carets that collide and keep
    /// the primary head in view.
    ///
    /// With `extend` each head moves and its anchor stays, so the selection
    /// grows or shrinks (Shift + motion). Without it each selection first
    /// collapses to its edge in the motion's direction (its start for a
    /// backward motion, its end for a forward one) and the caret moves from
    /// there; [`Motion::Left`] and [`Motion::Right`] stop at that edge. Carets
    /// are moved independently, so two carets merge only when they end on the
    /// same position.
    ///
    /// Motions follow the last render's layout (see [`Motion`]). A vertical
    /// motion steers each caret toward its goal column — the display cell it
    /// started from, kept across rows too short to reach it until a motion of
    /// another kind, an edit or a pointer move resets it. A page motion also
    /// scrolls the view by the rows the primary caret moved, so the caret
    /// keeps its place on screen.
    pub fn move_carets(&mut self, buffer: &TextBuffer, motion: Motion, extend: bool) {
        let page = u32::from(self.last_height.max(1));
        let rows = self.rows(buffer);
        let goals = self.live_goals(buffer);
        let paging = matches!(motion, Motion::PageUp | Motion::PageDown);
        let screen_row = paging
            .then(|| self.screen_row(&rows, self.cursor()))
            .flatten();
        let mut moved = Vec::with_capacity(self.cursors.selections.len());
        for s in &self.cursors.selections {
            let collapse = !extend && !s.is_empty();
            let from = if collapse {
                let range = s.range();
                if motion.is_backward() {
                    range.start
                } else {
                    range.end
                }
            } else {
                s.head
            };
            let step = if collapse && matches!(motion, Motion::Left | Motion::Right) {
                Moved::to(from)
            } else {
                rows.apply(motion, page, (from, self.upstream.contains(&from)), goals)
            };
            let selection = if extend {
                Selection {
                    anchor: s.anchor,
                    head: step.to,
                }
            } else {
                Selection::caret(step.to)
            };
            moved.push((selection, step));
        }
        // The view keeps the primary caret on the row it was on.
        let top = screen_row
            .zip(moved.get(self.cursors.primary))
            .map(|(row, (_, step))| {
                let at = rows.row_of(step.to, step.upstream).anchor;
                // Screen rows, virtual lines included.
                rows.walk_rows(at, row, false, false).0
            });
        let goals = motion.is_vertical().then(|| Goals {
            version: buffer.version(),
            carets: moved
                .iter()
                .filter_map(|(_, step)| step.goal.map(|goal| (step.to, goal)))
                .collect(),
        });
        let upstream: Vec<LineCol> = moved
            .iter()
            .filter(|(_, step)| step.upstream)
            .map(|(_, step)| step.to)
            .collect();
        self.cursors.selections = moved.into_iter().map(|(selection, _)| selection).collect();
        self.normalize();
        self.goals = goals;
        if let Some(top) = top {
            self.scroll_line = top.line;
            self.scroll_subrow = top.subrow;
        }
        self.reveal(buffer, self.cursor());
        self.upstream = upstream;
    }

    /// Add a bare caret one row above the primary, at the primary's goal
    /// column, as the new primary; it merges if it lands on another caret. A
    /// no-op on the first row.
    pub fn add_caret_above(&mut self, buffer: &TextBuffer) {
        self.add_caret_vertically(buffer, false);
    }

    /// Add a bare caret one row below the primary, at the primary's goal
    /// column, as the new primary. A no-op on the last row.
    pub fn add_caret_below(&mut self, buffer: &TextBuffer) {
        self.add_caret_vertically(buffer, true);
    }

    /// [`add_caret_above`](Self::add_caret_above) or, `down`,
    /// [`add_caret_below`](Self::add_caret_below).
    fn add_caret_vertically(&mut self, buffer: &TextBuffer, down: bool) {
        let head = self.cursor();
        let upstream = self.upstream.contains(&head);
        let rows = self.rows(buffer);
        let mut carets = self.live_goals(buffer).to_vec();
        let goal = carets
            .iter()
            .find(|(at, _)| *at == head)
            .map_or_else(|| rows.cell(head, upstream), |(_, goal)| *goal);
        let (anchor, complete) = rows.walk(rows.row_of(head, upstream).anchor, 1, down);
        if !complete {
            return;
        }
        let (to, to_upstream) = rows.place(anchor, goal);
        carets.extend([(head, goal), (to, goal)]);
        self.push(Selection::caret(to));
        self.goals = Some(Goals {
            version: buffer.version(),
            carets,
        });
        let mut kept = std::mem::take(&mut self.upstream);
        self.reveal(buffer, self.cursor());
        if to_upstream {
            kept.push(to);
        }
        self.upstream = kept;
    }

    /// Move every caret whose head `fold` hides — on lines `fold.start + 1
    /// ..= fold.end` — onto its header line as a bare caret, at the same
    /// column or the header's end. For an application collapsing `fold`, so
    /// no caret is left where it cannot be seen. The view does not scroll.
    pub fn lift_carets_onto_header(&mut self, buffer: &TextBuffer, fold: Fold) {
        let mut lifted = false;
        for s in &mut self.cursors.selections {
            if fold.start < s.head.line && s.head.line <= fold.end {
                let header = LineCol::new(fold.start, s.head.col);
                *s = Selection::caret(clamp_to_buffer(buffer, header));
                lifted = true;
            }
        }
        if lifted {
            self.goals = None;
            self.upstream.clear();
            self.normalize();
        }
    }
}
