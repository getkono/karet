//! Selection painting: a selected line break shows as one cell past the line
//! end, so blank lines and line ends inside a selection are visible.

use crate::text::*;
use crate::*;

/// The gutter width for a buffer of fewer than ten lines: marker, digit, space.
const GUTTER: u16 = 3;

/// Render `buffer` with one selection from `anchor` to `head`, unfocused so no
/// caret cell overlays the painting under test.
fn render_selected(
    buffer: &TextBuffer,
    anchor: LineCol,
    head: LineCol,
    area: Rect,
    wrap: bool,
) -> (Buffer, EditorState) {
    let theme = Theme::dark();
    let mut state = EditorState::new();
    let mut buf = Buffer::empty(area);
    // A first frame records the viewport height, so placing the selection
    // reveals its head against the real area rather than a zero-row one.
    for selection in [None, Some(Selection { anchor, head })] {
        if let Some(selection) = selection {
            state.set_cursor_state(buffer, CursorState::single(selection));
        }
        buf = Buffer::empty(area);
        Editor::new(buffer)
            .theme(&theme)
            .focused(false)
            .word_wrap(wrap)
            .render(area, &mut buf, &mut state);
    }
    (buf, state)
}

fn selected(buf: &Buffer, x: u16, y: u16) -> bool {
    buf[(x, y)].bg == Theme::dark().role(ThemeRole::Selection).to_ratatui()
}

#[test]
fn select_all_paints_blank_lines_and_line_ends() {
    let buffer = TextBuffer::from_text("ab\n\ncd");
    let (buf, _) = render_selected(
        &buffer,
        LineCol::new(0, 0),
        LineCol::new(2, 2),
        Rect::new(0, 0, 12, 10),
        false,
    );
    let theme = Theme::dark();
    // Line 0: both characters, then its line break one cell past the end.
    assert!(selected(&buf, GUTTER, 0) && selected(&buf, GUTTER + 1, 0));
    assert!(selected(&buf, GUTTER + 2, 0), "line 0's line break");
    assert_eq!(
        buf[(GUTTER + 2, 0)].fg,
        theme.role(ThemeRole::Foreground).to_ratatui()
    );
    assert!(!selected(&buf, GUTTER + 3, 0), "exactly one cell");
    // The blank line paints its single line-break cell.
    assert!(selected(&buf, GUTTER, 1), "blank line");
    assert!(!selected(&buf, GUTTER + 1, 1));
    // The last line has no line break: nothing past its text.
    assert!(selected(&buf, GUTTER + 1, 2));
    assert!(
        !selected(&buf, GUTTER + 2, 2),
        "no break after the last line"
    );
}

#[test]
fn shift_down_from_a_blank_line_paints_its_break_only() {
    // Shift+Down from column 0 of the blank line selects just its break.
    let buffer = TextBuffer::from_text("ab\n\ncd\n");
    let (buf, _) = render_selected(
        &buffer,
        LineCol::new(1, 0),
        LineCol::new(2, 0),
        Rect::new(0, 0, 12, 10),
        false,
    );
    assert!(selected(&buf, GUTTER, 1), "the blank line's break");
    assert!(!selected(&buf, GUTTER + 2, 0), "line 0's break is outside");
    assert!(!selected(&buf, GUTTER, 2), "nothing on the end line");
}

#[test]
fn a_selection_ending_at_the_line_end_leaves_the_break_unpainted() {
    let buffer = TextBuffer::from_text("ab\ncd\n");
    let (buf, _) = render_selected(
        &buffer,
        LineCol::new(0, 0),
        LineCol::new(0, 2),
        Rect::new(0, 0, 12, 10),
        false,
    );
    assert!(selected(&buf, GUTTER + 1, 0));
    assert!(!selected(&buf, GUTTER + 2, 0));
}

#[test]
fn a_selection_starting_at_the_line_end_paints_the_break() {
    let buffer = TextBuffer::from_text("ab\ncd\n");
    let (buf, _) = render_selected(
        &buffer,
        LineCol::new(0, 2),
        LineCol::new(1, 1),
        Rect::new(0, 0, 12, 10),
        false,
    );
    assert!(!selected(&buf, GUTTER + 1, 0));
    assert!(selected(&buf, GUTTER + 2, 0), "the break after `ab`");
    assert!(selected(&buf, GUTTER, 1) && !selected(&buf, GUTTER + 1, 1));
}

#[test]
fn a_wrapped_line_paints_its_break_on_the_last_row_only() {
    // Five content cells wrap `abcdefg` into `abcde` / `fg`.
    let buffer = TextBuffer::from_text("abcdefg\nx\n");
    let (buf, _) = render_selected(
        &buffer,
        LineCol::new(0, 0),
        LineCol::new(1, 0),
        Rect::new(0, 0, 8, 10),
        true,
    );
    assert!(selected(&buf, GUTTER + 4, 0));
    assert!(selected(&buf, GUTTER + 1, 1), "`g` on the second row");
    assert!(selected(&buf, GUTTER + 2, 1), "the break after `g`");
    assert!(!selected(&buf, GUTTER, 2), "nothing on the end line");
}

#[test]
fn a_break_scrolled_off_to_the_left_is_not_painted_at_the_margin() {
    let buffer = TextBuffer::from_text("ab\nabcdefghij\n");
    let (buf, state) = render_selected(
        &buffer,
        LineCol::new(0, 0),
        LineCol::new(1, 9),
        Rect::new(0, 0, 8, 10),
        false,
    );
    assert!(state.scroll_col > 2, "line 0's end is off to the left");
    assert!((GUTTER..8).all(|x| !selected(&buf, x, 0)));
}

#[test]
fn newline_in_any_covers_the_slot_past_the_last_character() {
    let Ok(range) = Range::new(LineCol::new(0, 1), LineCol::new(2, 0)) else {
        return;
    };
    let selections = [range];
    assert!(newline_in_any(&selections, 0, 3), "start line");
    assert!(newline_in_any(&selections, 1, 0), "blank interior line");
    assert!(!newline_in_any(&selections, 2, 4), "end line");
    assert!(!newline_in_any(&selections, 0, 0), "break before the start");
    assert!(!newline_in_any(&[], 1, 0), "no selection");
}
