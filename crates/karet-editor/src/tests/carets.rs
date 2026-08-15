//! Caret, multi-caret, and screen-cell geometry tests, split from the main
//! test module to respect the per-file code-line ceiling.

use crate::text::*;
use crate::*;

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
fn screen_cell_maps_arbitrary_visible_positions() {
    let buffer = TextBuffer::from_text("abc\ndef\n");
    let mut state = EditorState::new();
    let area = Rect::new(10, 5, 20, 4);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(1, 3)),
        Some((16, 6))
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(8, 0)),
        None
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(0, 30)),
        None
    );
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
fn set_cursor_state_preserves_selections_and_clamps_endpoints() {
    let buffer = TextBuffer::from_text("abc\nx");
    let mut state = EditorState::new();
    state.set_cursor_state(
        &buffer,
        CursorState {
            selections: vec![Selection {
                anchor: LineCol::new(0, 1),
                head: LineCol::new(9, 9),
            }],
            primary: 7,
        },
    );
    assert_eq!(
        state.cursors().primary(),
        Selection {
            anchor: LineCol::new(0, 1),
            head: LineCol::new(1, 1),
        }
    );
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
