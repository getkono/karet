//! Multi-line text editing: a composer-style text area with vertical motion,
//! wrap-aware row mapping, selection, and a ratatui renderer.
//!
//! [`TextAreaState`] layers the missing multi-line pieces — vertical motion, the
//! wrapped-row model, and a vertical viewport — over the single-line
//! [`crate::textfield::TextFieldState`], whose character/word motions and editing
//! primitives already honour embedded `\n`.
//!
//! # Buffer ownership
//!
//! The state does **not** own the buffer: every method takes the text as a
//! `&str`/`&mut String` parameter, exactly as [`crate::textfield`] does. That keeps
//! the two field flavours interchangeable for a caller that already stores the
//! draft elsewhere (a commit message retained across a blur, an agent composer
//! whose text is submitted verbatim), and lets one renderer serve both.
//!
//! # One layout, two halves
//!
//! [`layout`] owns the soft-wrap model, and both halves of the widget derive from
//! it: [`TextArea`] paints the rows it produces, and [`byte_at_row_col`] maps a
//! clicked cell back through the same rows. Where those two once had separate
//! answers — a character-cell model for the hit test against `Paragraph`'s word
//! wrapping for the paint — a click could land on the wrong glyph and the caret
//! could scroll to the wrong row.

mod layout;
mod render;

use std::ops::Range;

pub use layout::WrapRow;
pub use layout::byte_at_row_col;
pub use layout::caret_cell;
pub use layout::cursor_row;
pub use layout::glyph_symbol;
pub use layout::glyph_width;
pub use layout::wrap_rows;
pub use render::TextArea;
pub use render::TextAreaStyle;
pub use render::styled_text;

use crate::textfield::TextFieldState;

/// Cursor, selection anchor, and vertical viewport for a multi-line text area.
///
/// See the [module docs](self) on why the buffer is passed in rather than owned.
#[derive(Clone, Debug, Default)]
pub struct TextAreaState {
    field: TextFieldState,
    /// The vertical viewport offset: the first wrapped display row in view.
    pub scroll: u16,
}

impl TextAreaState {
    /// The cursor's byte offset into the buffer.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.field.cursor()
    }

    /// The selected byte range, if a non-empty selection is active.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.field.selection()
    }

    /// The selected slice of `text`, if a non-empty selection is active.
    #[must_use]
    pub fn selected_text<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.field.selected_text(text)
    }

    /// Select the whole buffer, cursor at the end.
    pub fn select_all(&mut self, text: &str) {
        self.field.select_all(text);
    }

    /// Place the cursor at byte `cursor` (clamped to a char boundary),
    /// extending the selection when `extend`.
    pub fn set_cursor(&mut self, text: &str, cursor: usize, extend: bool) {
        self.field.set_cursor(text, cursor, extend);
    }

    /// Move one character left (collapsing or extending the selection).
    pub fn move_left(&mut self, text: &str, extend: bool) {
        self.field.move_left(text, extend);
    }

    /// Move one character right (collapsing or extending the selection).
    pub fn move_right(&mut self, text: &str, extend: bool) {
        self.field.move_right(text, extend);
    }

    /// Jump to the previous word boundary.
    pub fn move_word_left(&mut self, text: &str, extend: bool) {
        self.field.move_word_left(text, extend);
    }

    /// Jump to the next word boundary.
    pub fn move_word_right(&mut self, text: &str, extend: bool) {
        self.field.move_word_right(text, extend);
    }

    /// Move to the start of the current logical line, or of the whole buffer
    /// when `document`.
    pub fn move_start(&mut self, text: &str, document: bool, extend: bool) {
        self.field.move_start(text, document, extend);
    }

    /// Move to the end of the current logical line, or of the whole buffer when
    /// `document`.
    pub fn move_end(&mut self, text: &str, document: bool, extend: bool) {
        self.field.move_end(text, document, extend);
    }

    /// Move one **logical** line up (`delta < 0`) or down, keeping the cursor's
    /// column — counted in characters, and clamped to the end of a shorter target
    /// line. At the first or last line the cursor does not move.
    ///
    /// The column is re-derived from the cursor on every call, so a round trip
    /// through a short line loses the original column (there is deliberately no
    /// sticky goal column).
    pub fn move_vertical(&mut self, text: &str, delta: i8, extend: bool) {
        let cursor = self.field.cursor();
        let start = text[..cursor].rfind('\n').map_or(0, |newline| newline + 1);
        let column = text[start..cursor].chars().count();
        let target_start = if delta < 0 {
            let Some(previous_end) = start.checked_sub(1) else {
                return;
            };
            text[..previous_end]
                .rfind('\n')
                .map_or(0, |newline| newline + 1)
        } else {
            let Some(next) = text[cursor..].find('\n') else {
                return;
            };
            cursor + next + 1
        };
        let target_end = text[target_start..]
            .find('\n')
            .map_or(text.len(), |newline| target_start + newline);
        let target = text[target_start..target_end]
            .char_indices()
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset);
        self.field.set_cursor(text, target, extend);
    }

    /// Insert `inserted` at the cursor, replacing any selection.
    pub fn insert(&mut self, text: &mut String, inserted: &str) {
        self.field.insert(text, inserted);
    }

    /// Delete backward: the selection if any, else one char (or one word).
    pub fn backspace(&mut self, text: &mut String, word: bool) {
        self.field.backspace(text, word);
    }

    /// Delete forward: the selection if any, else one char (or one word).
    pub fn delete(&mut self, text: &mut String, word: bool) {
        self.field.delete(text, word);
    }

    /// Remove and return the selected text, if a selection is active.
    pub fn cut(&mut self, text: &mut String) -> Option<String> {
        self.field.cut(text)
    }

    /// The wrapped display row the caret sits on, for a viewport `width` cells
    /// wide. See [`layout`] on the wrap model.
    #[must_use]
    pub fn cursor_row(&self, text: &str, width: u16) -> u16 {
        cursor_row(text, self.field.cursor(), width)
    }

    /// Scroll the vertical viewport so the caret's row stays within `height` rows.
    pub fn ensure_cursor_visible(&mut self, text: &str, width: u16, height: u16) {
        let row = self.cursor_row(text, width);
        let visible = height.max(1);
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll.saturating_add(visible) {
            self.scroll = row.saturating_sub(visible.saturating_sub(1));
        }
    }

    /// Clamp [`Self::scroll`] so the viewport cannot sit past the last row.
    ///
    /// [`Self::ensure_cursor_visible`] keeps the caret in view, but a wheel or a
    /// scrollbar drag writes the offset directly, and shrinking the buffer can
    /// strand a viewport that was valid when it was set.
    pub fn clamp_scroll(&mut self, text: &str, width: u16, height: u16) {
        let rows = wrap_rows(text, width).len();
        let last = rows.saturating_sub(usize::from(height.max(1)));
        self.scroll = self.scroll.min(u16::try_from(last).unwrap_or(u16::MAX));
    }

    /// Place the cursor at a viewport cell: `column`/`row` are relative to the
    /// text area's top-left, so `row` is offset by the current [`Self::scroll`].
    ///
    /// The hit test reads the same [`wrap_rows`] layout the renderer paints from,
    /// so a click lands on the glyph that was clicked.
    pub fn place_cursor(&mut self, text: &str, column: u16, row: u16, width: u16, extend: bool) {
        let target_row = usize::from(row.saturating_add(self.scroll));
        let target = byte_at_row_col(text, target_row, usize::from(column), width);
        self.field.set_cursor(text, target, extend);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, cursor: usize) -> TextAreaState {
        let mut state = TextAreaState::default();
        state.set_cursor(text, cursor, false);
        state
    }

    #[test]
    fn vertical_motion_clamps_the_column_onto_a_shorter_line() {
        let text = "longer line\nab\nlonger again";
        let mut state = at(text, 7); // "longer |line"
        state.move_vertical(text, 1, false);
        assert_eq!(state.cursor(), 14, "clamped to the end of \"ab\"");
        state.move_vertical(text, 1, false);
        // No sticky goal column: the column is re-read from the clamped position.
        assert_eq!(state.cursor(), 17);
    }
    #[test]
    fn vertical_motion_stops_at_the_first_and_last_line() {
        let text = "one\ntwo";
        let mut state = at(text, 1);
        state.move_vertical(text, -1, false);
        assert_eq!(state.cursor(), 1, "already on the first line");
        let mut state = at(text, 5);
        state.move_vertical(text, 1, false);
        assert_eq!(state.cursor(), 5, "already on the last line");
    }
    #[test]
    fn vertical_motion_counts_the_column_in_characters_not_bytes() {
        let text = "héllo\nworld";
        // "é" is two bytes, so byte 5 ("héll|o") is *character* column 4.
        let mut state = at(text, 5);
        state.move_vertical(text, 1, false);
        // Column 4 of "world" is byte 11; a byte-counted column 5 would have
        // run off the end of the line instead.
        assert_eq!(state.cursor(), 11);
        state.move_vertical(text, -1, false);
        assert_eq!(state.cursor(), 5, "and back onto the multi-byte line");
    }
    #[test]
    fn every_motion_can_extend_the_selection() {
        let text = "alpha beta\ngamma";
        let mut state = at(text, 0);
        state.move_vertical(text, 1, true);
        assert_eq!(state.selected_text(text), Some("alpha beta\n"));
        let mut state = at(text, 0);
        state.move_word_right(text, true);
        assert_eq!(state.selected_text(text), Some("alpha"));
        let mut state = at(text, 0);
        state.move_end(text, false, true);
        assert_eq!(state.selected_text(text), Some("alpha beta"));
        let mut state = at(text, 0);
        state.move_end(text, true, true);
        assert_eq!(state.selected_text(text), Some(text));
        let mut state = at(text, 16);
        state.move_start(text, false, true);
        assert_eq!(state.selected_text(text), Some("gamma"));
        let mut state = at(text, 1);
        state.move_left(text, true);
        assert_eq!(state.selected_text(text), Some("a"));
        let mut state = at(text, 0);
        state.move_right(text, true);
        assert_eq!(state.selected_text(text), Some("a"));
    }
    #[test]
    fn editing_primitives_reach_through_to_the_buffer() {
        let mut text = "alpha beta".to_string();
        let mut state = at(&text, 10);
        state.insert(&mut text, "\ngamma");
        assert_eq!(text, "alpha beta\ngamma");
        state.backspace(&mut text, true);
        assert_eq!(text, "alpha beta\n");
        state.set_cursor(&text, 0, false);
        state.delete(&mut text, true);
        assert_eq!(text, " beta\n");
        state.select_all(&text);
        assert_eq!(state.cut(&mut text).as_deref(), Some(" beta\n"));
        assert!(text.is_empty());
    }
    #[test]
    fn a_click_lands_on_the_cell_the_caret_was_painted_in() {
        let text = "the quick brown fox\njumps";
        // The caret row never runs backwards as the cursor advances.
        let mut previous = 0;
        for cursor in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let row = cursor_row(text, cursor, 8);
            assert!(row >= previous, "row {row} < {previous} at cursor {cursor}");
            previous = row;
        }
        // And clicking the caret's own cell puts the cursor back where it was —
        // the property that broke when the paint and the hit test disagreed.
        for cursor in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let mut state = at(text, cursor);
            let (row, column) = caret_cell(text, cursor, 8);
            state.place_cursor(text, column, row, 8, false);
            assert_eq!(state.cursor(), cursor, "round trip at {cursor}");
        }
    }

    #[test]
    fn an_empty_buffer_and_a_trailing_newline_stay_in_bounds() {
        let mut state = TextAreaState::default();
        state.move_vertical("", -1, false);
        state.move_vertical("", 1, false);
        state.place_cursor("", 9, 9, 10, false);
        assert_eq!(state.cursor(), 0);
        assert_eq!(cursor_row("", 0, 10), 0);

        let text = "line\n";
        let mut state = at(text, 5);
        state.move_vertical(text, 1, false);
        assert_eq!(state.cursor(), 5, "the empty last line has no successor");
        state.move_vertical(text, -1, false);
        assert_eq!(state.cursor(), 0, "column 0 of the first line");
        assert_eq!(cursor_row(text, 5, 10), 1);
        assert_eq!(byte_at_row_col(text, 1, 0, 10), 5);
    }
    #[test]
    fn place_cursor_offsets_the_row_by_the_scrolled_viewport() {
        let text = "one\ntwo\nthree";
        let mut state = TextAreaState {
            scroll: 2,
            ..TextAreaState::default()
        };
        state.place_cursor(text, 1, 0, 20, false);
        assert_eq!(state.cursor(), 9, "viewport row 0 is buffer row 2");
    }
    #[test]
    fn clamping_pulls_a_stranded_viewport_back_onto_the_buffer() {
        let text = "a\nb\nc\nd\ne";
        let mut state = at(text, 0);
        state.scroll = 40;
        state.clamp_scroll(text, 20, 3);
        assert_eq!(
            state.scroll, 2,
            "rows 2..=4 are the last that fill the view"
        );
        // A buffer shorter than the viewport can only sit at the top.
        state.clamp_scroll("a", 20, 3);
        assert_eq!(state.scroll, 0);
        // A view that already fits is left alone.
        state.scroll = 1;
        state.clamp_scroll(text, 20, 4);
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn the_viewport_follows_the_caret_row_in_both_directions() {
        let text = "a\nb\nc\nd\ne";
        let mut state = at(text, 8);
        state.ensure_cursor_visible(text, 20, 3);
        assert_eq!(state.scroll, 2, "row 4 is the last of rows 2..=4");
        state.set_cursor(text, 0, false);
        state.ensure_cursor_visible(text, 20, 3);
        assert_eq!(state.scroll, 0);
    }
}
