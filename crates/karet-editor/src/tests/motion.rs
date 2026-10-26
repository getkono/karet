//! Caret motion through the rendered layout: goal columns in display cells,
//! the document's first and last rows, pages, collapsed folds, soft-wrapped
//! rows and smart Home.

use karet_core::Change;
use karet_core::TextEdit;

use crate::*;

fn at(line: u32, col: u32) -> LineCol {
    LineCol::new(line, col)
}

/// Render `buffer` `width` cells wide and ten rows tall, so the state records
/// the layout motions follow.
fn paint(buffer: &TextBuffer, state: &mut EditorState, folds: &[Fold], wrap: bool, width: u16) {
    let area = Rect::new(0, 0, width, 10);
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .word_wrap(wrap)
        .folds(folds)
        .render(area, &mut target, state);
}

/// A state over `buffer`, rendered unwrapped and unfolded, with carets at
/// `carets`.
fn state_at(buffer: &TextBuffer, carets: &[LineCol]) -> EditorState {
    let mut state = EditorState::new();
    paint(buffer, &mut state, &[], false, 40);
    state.set_carets(carets);
    state
}

/// Every head, in order.
fn heads(state: &EditorState) -> Vec<LineCol> {
    state.cursors().selections.iter().map(|s| s.head).collect()
}

/// Apply each motion in turn, without Shift, and record where the primary
/// lands after each.
fn walk(buffer: &TextBuffer, state: &mut EditorState, motions: &[Motion]) -> Vec<LineCol> {
    motions
        .iter()
        .map(|motion| {
            state.move_carets(buffer, *motion, false);
            state.cursor()
        })
        .collect()
}

#[test]
fn the_goal_column_survives_a_shorter_line() {
    let buffer = TextBuffer::from_text("abcdef\nab\nabcdef");
    let mut state = state_at(&buffer, &[at(0, 5)]);
    let path = walk(
        &buffer,
        &mut state,
        &[Motion::Down, Motion::Down, Motion::Up, Motion::Up],
    );
    assert_eq!(path, vec![at(1, 2), at(2, 5), at(1, 2), at(0, 5)]);
}

#[test]
fn the_goal_column_is_measured_in_display_cells() {
    // A tab four cells wide puts the caret after `f` in cell 5, which is
    // column 5 on a line without tabs; wide characters count two cells.
    let buffer = TextBuffer::from_text("\tfoo\nabcdefgh\n日本語");
    let mut state = state_at(&buffer, &[at(0, 2)]);
    let path = walk(&buffer, &mut state, &[Motion::Down, Motion::Up]);
    assert_eq!(path, vec![at(1, 5), at(0, 2)]);

    let mut state = state_at(&buffer, &[at(1, 4)]);
    let path = walk(&buffer, &mut state, &[Motion::Down, Motion::Up]);
    assert_eq!(path, vec![at(2, 2), at(1, 4)]);
}

#[test]
fn each_caret_keeps_its_own_goal_column() {
    let buffer = TextBuffer::from_text("abcdef\nab\nabcdef\nab\nabcdef");
    let mut state = state_at(&buffer, &[at(0, 5), at(2, 4)]);
    state.move_carets(&buffer, Motion::Down, false);
    assert_eq!(heads(&state), vec![at(1, 2), at(3, 2)]);
    state.move_carets(&buffer, Motion::Down, false);
    assert_eq!(heads(&state), vec![at(2, 5), at(4, 4)]);
}

#[test]
fn a_horizontal_motion_resets_the_goal_column() {
    let buffer = TextBuffer::from_text("abcdef\nab\nabcdef");
    let mut state = state_at(&buffer, &[at(0, 5)]);
    let path = walk(
        &buffer,
        &mut state,
        &[Motion::Down, Motion::Left, Motion::Down],
    );
    assert_eq!(path, vec![at(1, 2), at(1, 1), at(2, 1)]);
}

#[test]
fn an_edit_resets_the_goal_column() {
    let mut buffer = TextBuffer::from_text("abcdef\nab\nabcdef");
    let mut state = state_at(&buffer, &[at(0, 5)]);
    state.move_carets(&buffer, Motion::Down, false);
    let edit = TextEdit {
        range: Range {
            start: at(2, 0),
            end: at(2, 0),
        },
        new_text: "x".to_string(),
    };
    let applied = buffer.apply_simple(&Change::new(buffer.version(), vec![edit]));
    assert!(applied.is_ok());
    state.move_carets(&buffer, Motion::Down, false);
    assert_eq!(
        state.cursor(),
        at(2, 2),
        "the caret's own column, not the goal"
    );
}

#[test]
fn up_on_the_first_row_and_down_on_the_last_go_to_the_document_ends() {
    let buffer = TextBuffer::from_text("abc\ndefg");
    let mut state = state_at(&buffer, &[at(0, 2)]);
    let path = walk(&buffer, &mut state, &[Motion::Up, Motion::Down]);
    assert_eq!(
        path,
        vec![at(0, 0), at(1, 2)],
        "the goal column outlives it"
    );

    let mut state = state_at(&buffer, &[at(1, 1)]);
    let path = walk(&buffer, &mut state, &[Motion::Down, Motion::Up]);
    assert_eq!(path, vec![at(1, 4), at(0, 1)]);

    let mut state = state_at(&buffer, &[at(0, 2)]);
    state.move_carets(&buffer, Motion::Up, true);
    assert_eq!(
        state.cursors().selections,
        vec![Selection {
            anchor: at(0, 2),
            head: at(0, 0),
        }],
        "Shift+Up on the first line selects to its start"
    );
}

#[test]
fn a_page_moves_the_caret_by_the_viewport_and_scrolls_with_it() {
    let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
    let buffer = TextBuffer::from_text(&text.join("\n"));
    let mut state = state_at(&buffer, &[at(3, 2)]);
    state.move_carets(&buffer, Motion::PageDown, false);
    assert_eq!(state.cursor(), at(13, 2));
    assert_eq!(state.scroll_line, 10, "the caret stays on its screen row");
    paint(&buffer, &mut state, &[], false, 40);
    state.move_carets(&buffer, Motion::PageUp, false);
    assert_eq!(state.cursor(), at(3, 2));
    assert_eq!(state.scroll_line, 0);

    state.goto(&buffer, at(25, 2));
    paint(&buffer, &mut state, &[], false, 40);
    state.move_carets(&buffer, Motion::PageDown, false);
    assert_eq!(state.cursor(), at(29, 7), "fewer rows left than a page");
    state.move_carets(&buffer, Motion::PageUp, false);
    assert_eq!(state.cursor(), at(19, 2), "back at the goal column");
}

#[test]
fn extend_page_down_and_up_move_the_caret_a_page_and_keep_the_anchor() {
    let text: Vec<String> = (0..30).map(|n| format!("line {n}")).collect();
    let buffer = TextBuffer::from_text(&text.join("\n"));
    let mut state = state_at(&buffer, &[at(3, 2)]);
    state.extend_page_down(&buffer);
    assert_eq!(
        state.cursors().selections,
        vec![Selection {
            anchor: at(3, 2),
            head: at(13, 2),
        }],
        "Shift+PageDown grows the selection by one page"
    );
    paint(&buffer, &mut state, &[], false, 40);
    state.extend_page_up(&buffer);
    assert_eq!(
        state.cursors().selections,
        vec![Selection {
            anchor: at(3, 2),
            head: at(3, 2),
        }],
        "Shift+PageUp moves the head back one page, and the anchor stays put"
    );
    state.extend_page_up(&buffer);
    assert_eq!(
        state.cursors().selections,
        vec![Selection {
            anchor: at(3, 2),
            head: at(0, 0),
        }],
        "a page past the first line selects to the document start"
    );
}

/// `zero`, then a fold headed by `head` hiding `in1` and `in2`, then `tail`.
fn folded() -> (TextBuffer, EditorState, [Fold; 1]) {
    let buffer = TextBuffer::from_text("zero\nhead\nin1\nin2\ntail");
    let folds = [Fold {
        start: 1,
        end: 3,
        collapsed: true,
    }];
    let mut state = EditorState::new();
    paint(&buffer, &mut state, &folds, false, 40);
    (buffer, state, folds)
}

#[test]
fn vertical_motion_steps_over_a_collapsed_fold() {
    let (buffer, mut state, _) = folded();
    state.place_caret(at(0, 3));
    let path = walk(
        &buffer,
        &mut state,
        &[Motion::Down, Motion::Down, Motion::Up, Motion::PageDown],
    );
    assert_eq!(path, vec![at(1, 3), at(4, 3), at(1, 3), at(4, 4)]);
}

#[test]
fn horizontal_motion_never_enters_a_collapsed_fold() {
    let (buffer, mut state, _) = folded();
    state.place_caret(at(1, 4));
    let path = walk(&buffer, &mut state, &[Motion::Right, Motion::Left]);
    assert_eq!(path, vec![at(4, 0), at(1, 4)]);

    state.place_caret(at(1, 4));
    let path = walk(&buffer, &mut state, &[Motion::WordRight]);
    assert_eq!(path, vec![at(4, 0)], "a word inside the fold is skipped");
}

#[test]
fn the_document_end_inside_a_fold_lands_on_its_header() {
    let buffer = TextBuffer::from_text("zero\nhead\nin1\nin2");
    let folds = [Fold {
        start: 1,
        end: 3,
        collapsed: true,
    }];
    let mut state = EditorState::new();
    paint(&buffer, &mut state, &folds, false, 40);
    let path = walk(&buffer, &mut state, &[Motion::DocEnd, Motion::Down]);
    assert_eq!(path, vec![at(1, 4), at(1, 4)]);
}

#[test]
fn collapsing_a_fold_lifts_every_caret_it_hides_onto_its_header() {
    let buffer = TextBuffer::from_text("head\nin1\nin2\ntail");
    let mut state = EditorState::new();
    state.set_carets(&[at(0, 1), at(1, 3), at(2, 2), at(3, 1)]);
    state.lift_carets_onto_header(
        &buffer,
        Fold {
            start: 0,
            end: 2,
            collapsed: true,
        },
    );
    assert_eq!(heads(&state), vec![at(0, 1), at(0, 2), at(0, 3), at(3, 1)]);
}

/// `alpha beta gamma` wrapped into ten content cells (`alpha ` on row 0,
/// columns 0..6; `beta gamma` on row 1, 6..16), then `xy`.
fn wrapped() -> (TextBuffer, EditorState) {
    let buffer = TextBuffer::from_text("alpha beta gamma\nxy");
    let mut state = EditorState::new();
    // A three-cell gutter: marker, one digit, space.
    paint(&buffer, &mut state, &[], true, 13);
    (buffer, state)
}

#[test]
fn wrapped_vertical_motion_moves_by_visual_rows() {
    let (buffer, mut state) = wrapped();
    state.place_caret(at(0, 2));
    let path = walk(
        &buffer,
        &mut state,
        &[Motion::Down, Motion::Down, Motion::Up, Motion::Up],
    );
    assert_eq!(path, vec![at(0, 8), at(1, 2), at(0, 8), at(0, 2)]);
}

#[test]
fn wrapped_end_goes_to_the_row_end_then_the_line_end() {
    let (buffer, mut state) = wrapped();
    state.place_caret(at(0, 2));
    state.move_carets(&buffer, Motion::LineEnd, false);
    assert_eq!(state.cursor(), at(0, 6));
    assert_eq!(state.upstream, vec![at(0, 6)], "drawn at the end of row 0");
    state.move_carets(&buffer, Motion::LineEnd, false);
    assert_eq!(state.cursor(), at(0, 16));
}

#[test]
fn wrapped_home_goes_to_the_start_of_a_continuation_row() {
    let (buffer, mut state) = wrapped();
    state.place_caret(at(0, 9));
    let path = walk(&buffer, &mut state, &[Motion::LineStart, Motion::Up]);
    assert_eq!(path, vec![at(0, 6), at(0, 0)]);
}

#[test]
fn home_toggles_between_the_first_non_blank_and_column_zero() {
    let buffer = TextBuffer::from_text("    let x\n   ");
    let mut state = state_at(&buffer, &[at(0, 7)]);
    let path = walk(
        &buffer,
        &mut state,
        &[Motion::LineStart, Motion::LineStart, Motion::LineStart],
    );
    assert_eq!(path, vec![at(0, 4), at(0, 0), at(0, 4)]);

    let mut state = state_at(&buffer, &[at(1, 3)]);
    let path = walk(&buffer, &mut state, &[Motion::LineStart]);
    assert_eq!(path, vec![at(1, 0)], "a blank line has no first non-blank");
}

#[test]
fn an_added_caret_keeps_the_goal_column() {
    let buffer = TextBuffer::from_text("abcdef\nab\nabcdef");
    let mut state = state_at(&buffer, &[at(0, 5)]);
    state.add_caret_below(&buffer);
    state.add_caret_below(&buffer);
    assert_eq!(heads(&state), vec![at(0, 5), at(1, 2), at(2, 5)]);
    assert_eq!(state.cursor(), at(2, 5), "the newest caret is primary");
}

#[test]
fn only_row_motions_are_vertical() {
    let vertical = [Motion::Up, Motion::Down, Motion::PageUp, Motion::PageDown];
    for motion in [
        Motion::Up,
        Motion::Down,
        Motion::Left,
        Motion::Right,
        Motion::PageUp,
        Motion::PageDown,
        Motion::LineStart,
        Motion::LineEnd,
        Motion::DocStart,
        Motion::DocEnd,
        Motion::WordLeft,
        Motion::WordRight,
    ] {
        assert_eq!(
            motion.is_vertical(),
            vertical.contains(&motion),
            "{motion:?}"
        );
    }
}
