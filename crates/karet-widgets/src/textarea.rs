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
//! # The wrap model
//!
//! [`cursor_row`] and [`byte_at_row_col`] are the two halves of a single model:
//! **character-cell wrapping** — glyphs are laid out left to right and a glyph
//! that would overflow the viewport starts the next row. That approximates
//! ratatui's `Wrap { trim: false }`, which additionally breaks on word
//! boundaries, so the two drift on a line that ratatui breaks early at a space.
//! Both live here so they cannot drift from *each other*.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use unicode_width::UnicodeWidthChar;

use crate::textfield::TextFieldState;

/// The caret glyph painted at the cursor, one cell wide.
const CARET: &str = "▏";

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
    /// wide. See the [module docs](self) on the wrap model.
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

    /// Place the cursor at a viewport cell: `column`/`row` are relative to the
    /// text area's top-left, so `row` is offset by the current [`Self::scroll`].
    ///
    /// The hit test maps the buffer's own glyphs and does not account for the
    /// one-cell caret [`styled_text`] injects *into* the line. Known limitation:
    /// on the caret's own row every glyph after the caret paints one cell right
    /// of where the hit test places it, so a click to the right of the caret on
    /// that row can land one glyph early.
    pub fn place_cursor(&mut self, text: &str, column: u16, row: u16, width: u16, extend: bool) {
        let target_row = usize::from(row.saturating_add(self.scroll));
        let target = byte_at_row_col(text, target_row, usize::from(column), width);
        self.field.set_cursor(text, target, extend);
    }
}

/// Where a glyph `char_width` cells wide lands when the pen sits at (`row`,
/// `column`) in a viewport `width` cells wide: it starts the next row when it
/// would overflow.
fn place_glyph(row: usize, column: usize, char_width: usize, width: usize) -> (usize, usize) {
    if column + char_width > width {
        (row.saturating_add(1), 0)
    } else {
        (row, column)
    }
}

/// The display width of `character` in cells, never less than one — a glyph the
/// renderer emits as its own span always occupies a cell.
fn glyph_width(character: char) -> usize {
    character.width().unwrap_or(0).max(1)
}

/// The wrapped display row holding the caret at byte `cursor`, for a viewport
/// `width` cells wide.
///
/// The row is a single left-to-right walk of the text before `cursor`: every
/// glyph advances the pen by its display width, a glyph that would overflow the
/// row starts the next one, and a `\n` starts the next row at column 0. A
/// completed line therefore costs exactly the rows its glyphs fill. The caret
/// glyph is itself one cell, so a cursor at the end of an exactly-full row
/// reports the row below — which is where the renderer paints it. See the
/// [module docs](self) on the wrap model.
///
/// `cursor` is an arbitrary byte offset and never panics: one past the end of
/// `text` walks the whole buffer, and one inside a multi-byte character reports
/// the same row as the next `char` boundary at or after it, since the walk
/// covers every character that *starts* before `cursor`.
#[must_use]
pub fn cursor_row(text: &str, cursor: usize, width: u16) -> u16 {
    let width = usize::from(width.max(1));
    let mut row = 0usize;
    let mut column = 0usize;
    for (_, character) in text.char_indices().take_while(|(index, _)| *index < cursor) {
        if character == '\n' {
            row = row.saturating_add(1);
            column = 0;
            continue;
        }
        let advance = glyph_width(character);
        let (next_row, next_column) = place_glyph(row, column, advance, width);
        row = next_row;
        column = next_column + advance;
    }
    let (row, _) = place_glyph(row, column, 1, width);
    u16::try_from(row).unwrap_or(u16::MAX)
}

/// The byte offset of the character shown at display cell (`row`, `column`) of a
/// buffer wrapped to `width` cells.
///
/// A row past the end of the text maps to the end of the buffer, and a column
/// past the end of a row maps to that row's last offset — clicking in the empty
/// space after a line puts the cursor at its end.
#[must_use]
pub fn byte_at_row_col(text: &str, row: usize, column: usize, width: u16) -> usize {
    let width = usize::from(width.max(1));
    let mut display_row = 0usize;
    let mut display_col = 0usize;
    let mut candidate = 0usize;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            if display_row == row {
                return candidate;
            }
            display_row = display_row.saturating_add(1);
            display_col = 0;
            candidate = index + 1;
            continue;
        }
        let advance = glyph_width(character);
        let (next_row, next_column) = place_glyph(display_row, display_col, advance, width);
        display_row = next_row;
        display_col = next_column;
        if display_row > row || (display_row == row && column < display_col + advance) {
            return index;
        }
        display_col += advance;
        candidate = index + character.len_utf8();
    }
    candidate
}

/// The three styles a text area paints its content with.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextAreaStyle {
    /// Unselected text.
    pub normal: Style,
    /// Text inside the active selection.
    pub selection: Style,
    /// The insertion caret.
    pub caret: Style,
}

impl TextAreaStyle {
    /// A style set painting unselected text with `normal`, the run inside the
    /// active selection with `selection`, and the insertion caret with `caret`.
    #[must_use]
    pub fn new(normal: Style, selection: Style, caret: Style) -> Self {
        Self {
            normal,
            selection,
            caret,
        }
    }
}

/// Build the styled [`Text`] for a buffer: one line per `\n`, one span per
/// character, the selected run in [`TextAreaStyle::selection`], and a caret span
/// at `cursor` when it is `Some`.
///
/// Passing `cursor: None` (an unfocused field) paints no caret. This is the
/// renderer shared by the single-line [`crate::textfield`] and the multi-line
/// [`TextArea`], so the two can never diverge.
#[must_use]
pub fn styled_text(
    text: &str,
    cursor: Option<usize>,
    selection: Option<Range<usize>>,
    style: TextAreaStyle,
) -> Text<'static> {
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    for (index, character) in text.char_indices() {
        if cursor == Some(index) {
            spans.push(Span::styled(CARET, style.caret));
        }
        if character == '\n' {
            lines.push(Line::from(std::mem::take(&mut spans)));
            continue;
        }
        let selected = selection
            .as_ref()
            .is_some_and(|range| range.start <= index && index < range.end);
        let painted = if selected {
            style.selection
        } else {
            style.normal
        };
        spans.push(Span::styled(character.to_string(), painted));
    }
    if cursor == Some(text.len()) {
        spans.push(Span::styled(CARET, style.caret));
    }
    lines.push(Line::from(spans));
    Text::from(lines)
}

/// A multi-line text area widget: paints a buffer with its selection and caret,
/// soft-wrapped to the area and scrolled to [`TextAreaState::scroll`].
///
/// The caller owns the frame around it (borders, title, focus styling) and calls
/// [`TextAreaState::ensure_cursor_visible`] before rendering.
pub struct TextArea<'a> {
    text: &'a str,
    state: &'a TextAreaState,
    style: TextAreaStyle,
    focused: bool,
    placeholder: Option<(&'a str, Style)>,
}

impl<'a> TextArea<'a> {
    /// A text area painting `text` through `state`.
    #[must_use]
    pub fn new(text: &'a str, state: &'a TextAreaState) -> Self {
        Self {
            text,
            state,
            style: TextAreaStyle::default(),
            focused: false,
            placeholder: None,
        }
    }

    /// Set the content styles.
    #[must_use]
    pub fn style(mut self, style: TextAreaStyle) -> Self {
        self.style = style;
        self
    }

    /// Whether the field has keyboard focus; an unfocused area paints no caret.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// A hint painted in place of the content while the buffer is empty **and**
    /// the field is unfocused.
    #[must_use]
    pub fn placeholder(mut self, text: &'a str, style: Style) -> Self {
        self.placeholder = Some((text, style));
        self
    }
}

impl Widget for TextArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let paragraph = match self.placeholder {
            Some((hint, style)) if self.text.is_empty() && !self.focused => {
                Paragraph::new(hint).style(style).wrap(Wrap { trim: false })
            },
            _ => Paragraph::new(styled_text(
                self.text,
                self.focused.then(|| self.state.cursor()),
                self.state.selection(),
                self.style,
            ))
            .wrap(Wrap { trim: false })
            .scroll((self.state.scroll, 0)),
        };
        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

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
    fn cursor_row_wraps_on_display_cells_and_reserves_the_caret_cell() {
        assert_eq!(cursor_row("subject\nbody", 8, 40), 1);
        // A cursor at the end of an exactly-full row: the caret is pushed down.
        assert_eq!(cursor_row("abcdefghij", 10, 5), 2);
        assert_eq!(cursor_row("abcdefghij", 9, 5), 1);
        assert_eq!(cursor_row("", 0, 5), 0);
        // Wide glyphs consume two cells, so they wrap twice as fast.
        assert_eq!(cursor_row("界界", 6, 3), 1);
        assert_eq!(cursor_row("界界", 6, 5), 0);
        // A zero width never stalls the walk.
        assert_eq!(cursor_row("abc", 3, 0), 3);
    }

    #[test]
    fn cursor_row_charges_a_full_line_only_the_rows_it_fills() {
        // Regression. The old floor-division model summed `segment.width() /
        // width` over every `\n`-segment of the prefix and added the newline
        // count, which over-counts by one for every *completed* preceding line
        // whose display width is a positive exact multiple of `width`: such a
        // line consumes `floor((w - 1) / width)` extra rows, not `floor(w /
        // width)`. It returned 2 and 4 for the first two cases below, so the
        // caret follow-scrolled one row too far — a commit panel of inner width
        // 50 with a 50-column subject line scrolled past the caret. The values
        // pinned here are the rows ratatui actually paints, and the ones
        // `byte_at_row_col`/`place_cursor` already mapped back.
        assert_eq!(cursor_row("subject\nbody", 12, 7), 1); // old: 2
        assert_eq!(cursor_row("aaaaa\nbbbbb\ncc", 14, 5), 2); // old: 4
        assert_eq!(cursor_row("abcdefghij\nxy", 13, 5), 2); // old: 3
        assert_eq!(cursor_row("abcde\nx", 7, 5), 1); // old: 2
        assert_eq!(cursor_row("abcd\n", 5, 4), 1); // old: 2
    }

    #[test]
    fn cursor_row_tolerates_a_cursor_off_a_char_boundary() {
        // Byte 2 is interior to the two-byte "é"; slicing the prefix panicked.
        assert_eq!(cursor_row("héllo", 2, 10), 0);
        // An interior offset reports the next boundary's row, not its own.
        let text = "hé\nllo";
        assert_eq!(cursor_row(text, 2, 10), cursor_row(text, 3, 10));
        assert_eq!(cursor_row("héllo", 99, 10), 0, "past the end walks it all");
    }

    #[test]
    fn hit_testing_maps_rows_and_wide_characters_back_to_byte_offsets() {
        let text = "subject\nbody";
        assert_eq!(byte_at_row_col(text, 0, 3, 20), 3);
        assert_eq!(byte_at_row_col(text, 0, 40, 20), 7, "past the line end");
        assert_eq!(byte_at_row_col(text, 1, 2, 20), 10);
        assert_eq!(byte_at_row_col(text, 9, 0, 20), text.len(), "past the end");
        // "a界b": the wide glyph covers cells 1 and 2.
        assert_eq!(byte_at_row_col("a界b", 0, 0, 10), 0);
        assert_eq!(byte_at_row_col("a界b", 0, 1, 10), 1);
        assert_eq!(byte_at_row_col("a界b", 0, 2, 10), 1);
        assert_eq!(byte_at_row_col("a界b", 0, 3, 10), 4);
        // Width 2 cannot fit "a界" — the wide glyph starts row 1.
        assert_eq!(byte_at_row_col("a界b", 1, 0, 2), 1);
    }

    #[test]
    fn hit_testing_round_trips_the_wrapped_row_of_the_cursor() {
        let text = "the quick brown fox\njumps";
        // The caret row never runs backwards as the cursor advances.
        let mut previous = 0;
        for cursor in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let row = cursor_row(text, cursor, 8);
            assert!(row >= previous, "row {row} < {previous} at cursor {cursor}");
            previous = row;
        }
        // A concrete round trip: the caret row of a wrapped offset hit-tests back.
        let mut state = at(text, 12);
        let row = state.cursor_row(text, 8);
        state.place_cursor(text, 4, row, 8, false);
        assert_eq!(state.cursor(), 12);
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
    fn the_viewport_follows_the_caret_row_in_both_directions() {
        let text = "a\nb\nc\nd\ne";
        let mut state = at(text, 8);
        state.ensure_cursor_visible(text, 20, 3);
        assert_eq!(state.scroll, 2, "row 4 is the last of rows 2..=4");
        state.set_cursor(text, 0, false);
        state.ensure_cursor_visible(text, 20, 3);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn the_renderer_splits_lines_marks_the_caret_and_styles_the_selection() {
        let text = "subject\nbody";
        let mut state = at(text, 8);
        let display = styled_text(text, Some(state.cursor()), None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2);
        assert_eq!(display.lines[0].to_string(), "subject");
        assert_eq!(display.lines[1].to_string(), "▏body");

        state.move_word_right(text, true);
        let style = TextAreaStyle::new(
            Style::default().fg(Color::White),
            Style::default().fg(Color::White).bg(Color::Blue),
            Style::default().fg(Color::Red),
        );
        let display = styled_text(text, Some(state.cursor()), state.selection(), style);
        let spans = &display.lines[1].spans;
        assert_eq!(spans.last().map(|span| span.content.as_ref()), Some("▏"));
        assert!(
            spans[..4]
                .iter()
                .all(|span| span.style.bg == Some(Color::Blue)),
            "the selected run is painted"
        );

        // Unfocused: no caret at all.
        let display = styled_text(text, None, None, TextAreaStyle::default());
        assert_eq!(display.lines[1].to_string(), "body");
    }

    #[test]
    fn the_renderer_opens_a_line_after_a_trailing_newline() {
        let display = styled_text("a\n", Some(2), None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2, "the trailing newline opens a line");
        assert_eq!(display.lines[0].to_string(), "a");
        assert_eq!(display.lines[1].to_string(), "▏");
        assert_eq!(display.lines[1].spans.len(), 1, "the caret alone");

        // No caret at all still leaves the empty final line in place.
        let display = styled_text("a\n", None, None, TextAreaStyle::default());
        assert_eq!(display.lines.len(), 2);
        assert!(display.lines[1].spans.is_empty());
    }

    #[test]
    fn the_renderer_splits_a_selection_around_a_caret_inside_it() {
        let style = TextAreaStyle::new(
            Style::default().fg(Color::White),
            Style::default().fg(Color::White).bg(Color::Blue),
            Style::default().fg(Color::Red),
        );
        let display = styled_text("abcd", Some(2), Some(1..3), style);
        let spans = &display.lines[0].spans;
        assert_eq!(display.lines[0].to_string(), "ab▏cd");
        assert_eq!(spans.len(), 5, "\"a\", \"b\", caret, \"c\", \"d\"");
        assert_eq!(spans[2].content.as_ref(), "▏");
        assert_eq!(
            spans[2].style.fg,
            Some(Color::Red),
            "the caret keeps its own style"
        );
        assert_eq!(spans[1].style.bg, Some(Color::Blue));
        assert_eq!(
            spans[3].style.bg,
            Some(Color::Blue),
            "the selection resumes past it"
        );
        assert_eq!(spans[0].style.bg, None);
        assert_eq!(spans[4].style.bg, None);
    }

    #[test]
    fn the_widget_paints_wrapped_content_the_caret_and_the_placeholder() {
        let area = Rect::new(0, 0, 4, 3);
        let text = "abcdef";
        let state = at(text, 6);
        let mut buf = Buffer::empty(area);
        TextArea::new(text, &state)
            .focused(true)
            .style(TextAreaStyle::new(
                Style::default().fg(Color::White),
                Style::default(),
                Style::default().fg(Color::Red),
            ))
            .render(area, &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some("a".to_owned())
        );
        // "abcdef▏" wraps to "abcd" / "ef▏".
        assert_eq!(
            buf.cell((2, 1)).map(|cell| cell.symbol().to_owned()),
            Some("▏".to_owned())
        );
        assert_eq!(buf.cell((2, 1)).map(|cell| cell.fg), Some(Color::Red));

        // The placeholder shows only while empty and unfocused.
        let state = TextAreaState::default();
        let mut buf = Buffer::empty(area);
        TextArea::new("", &state)
            .placeholder("hint", Style::default().fg(Color::Gray))
            .render(area, &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some("h".to_owned())
        );
        let mut buf = Buffer::empty(area);
        TextArea::new("", &state)
            .focused(true)
            .placeholder("hint", Style::default().fg(Color::Gray))
            .render(area, &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some("▏".to_owned())
        );
    }

    #[test]
    fn the_widget_scrolls_to_the_state_viewport_and_skips_an_empty_area() {
        let text = "one\ntwo\nthree";
        let mut state = at(text, 0);
        state.scroll = 2;
        let area = Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        TextArea::new(text, &state).render(area, &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some("t".to_owned())
        );

        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        TextArea::new(text, &state).render(Rect::new(0, 0, 0, 0), &mut buf);
        assert_eq!(
            buf.cell((0, 0)).map(|cell| cell.symbol().to_owned()),
            Some(" ".to_owned())
        );
    }
}
