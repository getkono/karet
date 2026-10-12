//! Box (column) selection: one selection per line over a rectangle of display
//! cells, made by Shift+Alt+drag and grown by Ctrl+Shift+Alt+arrows.
//!
//! The rectangle is held in display cells — tabs expanded to their stops, wide
//! characters counted twice — rather than in buffer columns, so it stays
//! rectangular across lines that indent with tabs or hold CJK text. Its corners
//! may lie past a line's end (virtual space): a short line inside the rectangle
//! gets a caret at its end, and the corner is remembered so the next step grows
//! the box from where the pointer or the keys left it, not from where the text
//! happened to stop. Inlay hints are not counted, as they are not text.

use karet_core::CursorState;
use karet_core::Selection;
use karet_session::ViewId;
use unicode_width::UnicodeWidthChar as _;

use super::super::*;

/// A corner of a box selection: a buffer line and a display-cell column on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Corner {
    /// The buffer line.
    pub(crate) line: u32,
    /// The display cell, counted from the start of the line.
    pub(crate) cell: u32,
}

/// The box selection last made in a view, so the next gesture continues it.
#[derive(Clone, Debug)]
pub(crate) struct ColumnSelect {
    /// The tab the box was made in.
    view: ViewId,
    /// The corner the box grows from.
    anchor: Corner,
    /// The corner that moves.
    head: Corner,
    /// The cursor set the box produced. The box continues only while the
    /// editor still holds exactly this set; any other change of selection
    /// means the user moved on, and the next gesture starts from the primary
    /// selection instead.
    cursors: CursorState,
}

/// How far a keyboard step moves the box's moving corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ColumnStep {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// One cell left.
    Left,
    /// One cell right, no further than the longest line the box spans.
    Right,
}

/// The cells `ch` occupies when it starts at cell `at`.
fn char_cells(ch: char, at: u32, tab_width: u16) -> u32 {
    if ch == '\t' {
        let width = u32::from(tab_width.max(1));
        width - at % width
    } else {
        u32::try_from(ch.width().unwrap_or(0)).unwrap_or(u32::MAX)
    }
}

/// The display cell buffer column `col` of `chars` starts at. Past the end of
/// the line each column counts one cell.
pub(crate) fn cell_of(chars: &[char], col: u32, tab_width: u16) -> u32 {
    let mut cell = 0_u32;
    for ch in chars.iter().take(col as usize) {
        cell = cell.saturating_add(char_cells(*ch, cell, tab_width));
    }
    let past = col.saturating_sub(u32::try_from(chars.len()).unwrap_or(u32::MAX));
    cell.saturating_add(past)
}

/// The buffer column of `chars` nearest to display cell `cell`, clamped to the
/// line's end. A cell inside a multi-cell character resolves to whichever of
/// its two edges is nearer, the left one on a tie.
pub(crate) fn col_at_cell(chars: &[char], cell: u32, tab_width: u16) -> u32 {
    let mut before = 0_u32;
    for (col, ch) in chars.iter().enumerate() {
        let after = before.saturating_add(char_cells(*ch, before, tab_width));
        if after > cell {
            let col = u32::try_from(col).unwrap_or(u32::MAX);
            return if after - cell < cell - before {
                col + 1
            } else {
                col
            };
        }
        before = after;
    }
    u32::try_from(chars.len()).unwrap_or(u32::MAX)
}

/// The characters of `line` in `buffer`, empty past its end.
fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect()
}

/// The selections a box from `anchor` to `head` makes in `buffer`.
///
/// Every line between the two corners gets one selection from the anchor's
/// cell to the head's, so the head of each points the way the box was drawn.
/// A line that ends before the box's left edge is left out, as VS Code leaves
/// it; when that leaves out every line, the box lies wholly past the text and
/// each line gets a caret at its end instead, so the gesture never selects
/// nothing. The primary is the selection on the head's line, or the one
/// nearest it, because that is where the pointer or the keys are working.
pub(crate) fn box_cursors(
    buffer: &TextBuffer,
    anchor: Corner,
    head: Corner,
    tab_width: u16,
) -> CursorState {
    let left = anchor.cell.min(head.cell);
    let lines = anchor.line.min(head.line)..=anchor.line.max(head.line);
    let mut selections = Vec::new();
    for line in lines.clone() {
        let chars = line_chars(buffer, line);
        let from = col_at_cell(&chars, anchor.cell, tab_width);
        let to = col_at_cell(&chars, head.cell, tab_width);
        let end = cell_of(&chars, from.max(to), tab_width);
        if end < left {
            continue;
        }
        selections.push(Selection {
            anchor: LineCol::new(line, from),
            head: LineCol::new(line, to),
        });
    }
    if selections.is_empty() {
        selections = lines
            .map(|line| {
                let end = u32::try_from(line_chars(buffer, line).len()).unwrap_or(u32::MAX);
                Selection::caret(LineCol::new(line, end))
            })
            .collect();
    }
    let primary = if head.line >= anchor.line {
        selections.len().saturating_sub(1)
    } else {
        0
    };
    CursorState {
        selections,
        primary,
    }
}

impl App {
    /// The box selection the active editor is in, or one started from its
    /// primary selection: anchored at the primary's anchor, with its moving
    /// corner at the primary's head.
    fn column_select_state(&self) -> Option<ColumnSelect> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return None;
        };
        if let Some(live) = &self.column_select
            && live.view == tab.view
            && live.cursors == *tab.editor.cursors()
        {
            return Some(live.clone());
        }
        let tab_width = self.tab_width_for(tab);
        let primary = tab.editor.cursors().primary();
        let corner = |pos: LineCol| Corner {
            line: pos.line,
            cell: cell_of(&line_chars(buffer, pos.line), pos.col, tab_width),
        };
        Some(ColumnSelect {
            view: tab.view,
            anchor: corner(primary.anchor),
            head: corner(primary.head),
            cursors: tab.editor.cursors().clone(),
        })
    }

    /// Select the box from the current box's anchor (or the primary
    /// selection's) to `head`, and remember it for the next gesture.
    ///
    /// `pointer` marks a pointer gesture: like every other pointer move it
    /// scrolls only when the primary head is off screen, where the keyboard
    /// keeps its usual margin around it.
    fn column_select_to(&mut self, head: Corner, pointer: bool) {
        let Some(mut state) = self.column_select_state() else {
            return;
        };
        let Some(tab_width) = self
            .tabs
            .get(self.active)
            .map(|tab| self.tab_width_for(tab))
        else {
            return;
        };
        let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return;
        };
        let cursors = box_cursors(buffer, state.anchor, head, tab_width);
        let scroll_col = editor.scroll_col;
        editor.set_cursor_state(buffer, cursors);
        if pointer {
            // `set_cursor_state` reveals with the keyboard's margin. Undo its
            // horizontal part and reveal again as a pointer move, which keeps
            // a head already on screen exactly where it is.
            editor.scroll_col = scroll_col;
            let primary = editor.cursors().primary().head;
            editor.extend_to(buffer, primary);
        }
        state.head = head;
        state.cursors = editor.cursors().clone();
        self.column_select = Some(state);
    }

    /// Grow or shrink the active editor's box selection by one keyboard step
    /// (Ctrl+Shift+Alt+arrows), starting a box from the primary selection when
    /// none is in progress.
    pub(in crate::app) fn column_select_step(&mut self, step: ColumnStep) {
        let Some(state) = self.column_select_state() else {
            return;
        };
        let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            ..
        }) = self.tabs.get(self.active)
        else {
            return;
        };
        let last = u32::try_from(buffer.line_count().saturating_sub(1)).unwrap_or(u32::MAX);
        let mut head = state.head;
        match step {
            ColumnStep::Up => head.line = head.line.saturating_sub(1),
            ColumnStep::Down => head.line = head.line.saturating_add(1).min(last),
            ColumnStep::Left => head.cell = head.cell.saturating_sub(1),
            ColumnStep::Right => {
                let tab_width = self
                    .tabs
                    .get(self.active)
                    .map_or(4, |t| self.tab_width_for(t));
                let lines = state.anchor.line.min(head.line)..=state.anchor.line.max(head.line);
                let widest = lines
                    .map(|line| {
                        let chars = line_chars(buffer, line);
                        cell_of(
                            &chars,
                            u32::try_from(chars.len()).unwrap_or(u32::MAX),
                            tab_width,
                        )
                    })
                    .max()
                    .unwrap_or(0);
                if head.cell < widest {
                    head.cell += 1;
                }
            },
        }
        self.column_select_to(head, false);
    }

    /// Box-select from the current box's anchor (or the primary selection's)
    /// to the cell under the pointer at `(col, row)` in the editor `area`
    /// (Shift+Alt+click, and every step of a Shift+Alt+drag).
    ///
    /// Past a line's end the pointer's cell is kept, so dragging across a
    /// short line toward a longer one keeps the box as wide as the pointer.
    pub(in crate::app) fn column_select_pointer(&mut self, area: Rect, col: u16, row: u16) {
        let left = self.drag_text_left(area);
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            return;
        };
        let tab_width = self.tab_width_for(tab);
        let fold_lines = resolve_folds(folds, folded);
        let pos = tab.editor.pos_at(area, buffer, &fold_lines, col, row);
        let chars = line_chars(buffer, pos.line);
        let len = u32::try_from(chars.len()).unwrap_or(u32::MAX);
        let mut cell = cell_of(&chars, pos.col, tab_width);
        if pos.col >= len {
            // Where the line's end is drawn, if it is on screen; otherwise it is
            // scrolled off to the left and the text area's first column stands
            // for the scroll offset, past the end like every cell after it.
            let past = match tab.editor.screen_cell(area, buffer, &fold_lines, pos) {
                Some((x, _)) => u32::from(col.saturating_sub(x)),
                None => {
                    tab.editor.scroll_col.saturating_sub(len) + u32::from(col.saturating_sub(left))
                },
            };
            cell = cell.saturating_add(past);
        }
        self.column_select_to(
            Corner {
                line: pos.line,
                cell,
            },
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn corner(line: u32, cell: u32) -> Corner {
        Corner { line, cell }
    }

    fn sel(line: u32, anchor: u32, head: u32) -> Selection {
        Selection {
            anchor: LineCol::new(line, anchor),
            head: LineCol::new(line, head),
        }
    }

    #[test]
    fn cells_expand_tabs_to_their_stops_and_count_wide_characters_twice() {
        let line = chars("\ta界b");
        assert_eq!(cell_of(&line, 0, 4), 0);
        assert_eq!(cell_of(&line, 1, 4), 4, "a tab at cell 0 runs to the stop");
        assert_eq!(cell_of(&line, 2, 4), 5);
        assert_eq!(cell_of(&line, 3, 4), 7, "a wide character takes two cells");
        assert_eq!(cell_of(&line, 4, 4), 8);
        assert_eq!(
            cell_of(&line, 6, 4),
            10,
            "past the end each column is one cell"
        );
        assert_eq!(
            cell_of(&chars("ab\tc"), 3, 4),
            4,
            "a mid-line tab stops at 4"
        );
    }

    #[test]
    fn a_cell_resolves_to_the_nearer_edge_of_the_character_over_it() {
        let line = chars("\ta界b");
        assert_eq!(col_at_cell(&line, 1, 4), 0, "near the tab's left edge");
        assert_eq!(col_at_cell(&line, 2, 4), 0, "a tie goes left");
        assert_eq!(col_at_cell(&line, 3, 4), 1, "near the tab's right edge");
        assert_eq!(
            col_at_cell(&line, 6, 4),
            2,
            "the wide character's middle ties left"
        );
        assert_eq!(col_at_cell(&line, 7, 4), 3);
        assert_eq!(col_at_cell(&line, 40, 4), 4, "past the end clamps to it");
    }

    #[test]
    fn a_box_selects_the_same_cells_on_every_line_it_spans() {
        let buffer = TextBuffer::from_text("abcdef\n\tx\nabcdef\n");
        let cursors = box_cursors(&buffer, corner(0, 1), corner(2, 5), 4);
        assert_eq!(
            cursors.selections,
            vec![sel(0, 1, 5), sel(1, 0, 2), sel(2, 1, 5)],
            "cell 1 is nearer the tab's start than its stop, and `x` ends at cell 5"
        );
        assert_eq!(cursors.primary, 2, "the head's line is primary");
    }

    #[test]
    fn a_box_drawn_up_and_left_points_every_head_that_way() {
        let buffer = TextBuffer::from_text("abcdef\nabcdef\n");
        let cursors = box_cursors(&buffer, corner(1, 4), corner(0, 2), 4);
        assert_eq!(cursors.selections, vec![sel(0, 4, 2), sel(1, 4, 2)]);
        assert_eq!(cursors.primary, 0, "the head is on the first line");
    }

    #[test]
    fn a_line_ending_before_the_box_is_left_out_and_one_inside_it_gets_a_caret() {
        let buffer = TextBuffer::from_text("abcdef\nab\nabcd\nabcdef\n");
        let cursors = box_cursors(&buffer, corner(0, 3), corner(3, 6), 4);
        assert_eq!(
            cursors.selections,
            vec![sel(0, 3, 6), sel(2, 3, 4), sel(3, 3, 6)],
            "`ab` ends left of cell 3; `abcd` reaches into the box"
        );
        let leftward = box_cursors(&buffer, corner(0, 6), corner(3, 3), 4);
        assert_eq!(
            leftward.selections,
            vec![sel(0, 6, 3), sel(2, 4, 3), sel(3, 6, 3)],
            "the same holds when the box is drawn right to left"
        );
    }

    #[test]
    fn a_box_wholly_past_the_text_puts_a_caret_at_each_line_end() {
        let buffer = TextBuffer::from_text("ab\nabc\n");
        let cursors = box_cursors(&buffer, corner(0, 8), corner(1, 10), 4);
        assert_eq!(
            cursors.selections,
            vec![
                Selection::caret(LineCol::new(0, 2)),
                Selection::caret(LineCol::new(1, 3))
            ]
        );
    }
}
