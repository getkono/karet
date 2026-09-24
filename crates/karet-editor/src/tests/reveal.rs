//! Horizontal cursor reveal: resolved by the motion itself, in display cells.
//!
//! A motion must leave `scroll_col` right *before* the next render, because a
//! view that mirrors this editor's offset (the merge-conflict side panes) reads
//! it before the editor paints.

use karet_core::InlayHintKind;

use crate::visual::*;
use crate::*;

/// One line, a 3-cell gutter, so a 33-cell area leaves 30 content cells.
const AREA: Rect = Rect::new(0, 0, 33, 1);
const GUTTER: u16 = 3;
const MARGIN: u16 = 10;

fn hint(col: u32, label: &str) -> InlayHint {
    InlayHint {
        position: LineCol::new(0, col),
        label: label.to_owned(),
        kind: InlayHintKind::Type,
        padding_left: false,
        padding_right: false,
    }
}

/// Paint `buffer` into `target`, returning the character under the caret and
/// its cell's column.
fn paint(
    editor: Editor<'_>,
    buffer: &TextBuffer,
    target: &mut Buffer,
    state: &mut EditorState,
) -> Option<(u16, String)> {
    editor.focused(true).render(AREA, target, state);
    let (x, y) = caret_cell(AREA, buffer, &[], state, state.cursor())?;
    Some((x, target[(x, y)].symbol().to_owned()))
}

#[test]
fn a_hinted_line_reveals_leftward_by_display_cells_before_any_render() {
    // An 8-cell hint before column 20 sits between the caret and the left
    // edge. Walking left from the end crosses the left margin; the reveal must
    // leave ten *cells* before the caret, so the origin is column 20 (hint,
    // then `uvw`). Counting chars would stop at column 12.
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz0123456789\n");
    let hints = [hint(20, "::::::::")];
    let mut state = EditorState::new();
    let mut target = Buffer::empty(AREA);
    let editor = || Editor::new(&buffer).inlay_hints(&hints);
    let _ = paint(editor(), &buffer, &mut target, &mut state);

    state.move_line_end(&buffer);
    assert_eq!(state.scroll_col, 21, "right reveal leaves 10 cells after");
    for _ in 0..14 {
        state.move_left(&buffer);
    }
    assert_eq!(state.cursor(), LineCol::new(0, 22));
    assert_eq!(
        state.scroll_col, 20,
        "resolved by the motion, not the render"
    );

    let painted = paint(editor(), &buffer, &mut target, &mut state);
    assert_eq!(state.scroll_col, 20, "the render does not revise it");
    assert_eq!(painted, Some((GUTTER + MARGIN, "w".to_owned())));
}

#[test]
fn a_tabbed_line_reveals_leftward_by_display_cells() {
    // Three tabs after `j` expand to ten cells, so a caret just right of them
    // is much further from the origin on screen than by char count.
    let buffer = TextBuffer::from_text("abcdefghij\t\t\tklmnopqrstuvwxyz0123456789\n");
    let mut state = EditorState::new();
    let mut target = Buffer::empty(AREA);
    let editor = || Editor::new(&buffer).tab_width(4);
    let _ = paint(editor(), &buffer, &mut target, &mut state);

    state.move_line_end(&buffer);
    for _ in 0..23 {
        state.move_left(&buffer);
    }
    assert_eq!(state.cursor(), LineCol::new(0, 16));
    let revealed = state.scroll_col;
    assert!(revealed > 0, "the tabs leave room to stay scrolled");

    let Some((x, glyph)) = paint(editor(), &buffer, &mut target, &mut state) else {
        unreachable!("a revealed caret is on screen");
    };
    assert_eq!(state.scroll_col, revealed, "the render does not revise it");
    assert_eq!(glyph, "n", "the caret is drawn over its own character");
    assert!(
        (GUTTER + MARGIN..AREA.width - MARGIN).contains(&x),
        "caret at cell {x} is inside a margin"
    );
}

#[test]
fn a_wrapped_view_never_scrolls_horizontally() {
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz0123456789\n");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 13, 4);
    let mut target = Buffer::empty(area);
    let editor = || Editor::new(&buffer).word_wrap(true);
    editor().render(area, &mut target, &mut state);

    state.move_line_end(&buffer);
    assert_eq!(state.scroll_col, 0);
    editor().render(area, &mut target, &mut state);
    assert_eq!(state.scroll_col, 0);
}

#[test]
fn scroll_to_without_a_buffer_is_resolved_by_the_next_render() {
    // `scroll_to` has no line text to measure, so it owes the horizontal
    // reveal to the render; `reveal` pays it at once.
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 25, 1); // 22 content cells, 10-cell margin.
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);

    state.scroll_to(LineCol::new(0, 15));
    assert_eq!(state.scroll_col, 0);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(state.scroll_col, 4);

    state.reveal(&buffer, LineCol::new(0, 2));
    assert_eq!(state.scroll_col, 0);
}

#[test]
fn a_reveal_before_the_first_render_waits_for_its_geometry() {
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
    let mut state = EditorState::new();
    state.reveal(&buffer, LineCol::new(0, 15));
    assert_eq!(state.scroll_col, 0, "no viewport width to measure against");

    let area = Rect::new(0, 0, 25, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(state.scroll_col, 4);
}
