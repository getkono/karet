//! Expand and Shrink Selection over a caller-supplied enclosing-range chain:
//! the word step, the smallest-enclosing pick, per-caret growth and merging,
//! and Shrink restoring the exact set each press grew from.

use crate::*;

/// `call(alpha, beta)  x`: the ranges a syntax tree would report around it.
const TEXT: &str = "call(alpha, beta)  x";

fn span(start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(0, start),
        end: LineCol::new(0, end),
    }
}

/// Every syntax range in [`TEXT`], deliberately out of order: the editor
/// picks the smallest one that contains a selection.
fn chain(_: Range) -> Vec<Range> {
    vec![
        span(0, 20),  // the file
        span(4, 17),  // the argument list
        span(5, 10),  // `alpha`
        span(0, 17),  // the call
        span(12, 16), // `beta`
    ]
}

fn caret(col: u32) -> Selection {
    Selection::caret(LineCol::new(0, col))
}

fn sel(anchor: u32, head: u32) -> Selection {
    Selection {
        anchor: LineCol::new(0, anchor),
        head: LineCol::new(0, head),
    }
}

fn state_with(buffer: &TextBuffer, selections: Vec<Selection>, primary: usize) -> EditorState {
    let mut state = EditorState::new();
    state.set_cursor_state(
        buffer,
        CursorState {
            selections,
            primary,
        },
    );
    state
}

#[test]
fn expand_grows_through_the_chain_and_shrink_walks_back_exactly() {
    let buffer = TextBuffer::from_text(TEXT);
    let mut state = state_with(&buffer, vec![caret(7)], 0);
    let mut grown = Vec::new();
    while state.expand_selection(&buffer, chain) {
        grown.push(state.cursors().primary());
    }
    assert_eq!(
        grown,
        vec![sel(5, 10), sel(4, 17), sel(0, 17), sel(0, 20)],
        "word, argument list, call, file"
    );
    // At the top nothing grows, and the selection stays.
    assert_eq!(state.cursors().primary(), sel(0, 20));
    let mut shrunk = Vec::new();
    while state.shrink_selection(&buffer) {
        shrunk.push(state.cursors().primary());
    }
    assert_eq!(shrunk, vec![sel(0, 17), sel(4, 17), sel(5, 10), caret(7)]);
}

#[test]
fn every_caret_grows_on_its_own_and_shrink_restores_the_set_and_primary() {
    let buffer = TextBuffer::from_text(TEXT);
    let start = CursorState {
        selections: vec![caret(7), caret(14)],
        primary: 0,
    };
    let mut state = state_with(&buffer, start.selections.clone(), start.primary);
    assert!(state.expand_selection(&buffer, chain));
    assert_eq!(state.cursors().selections, vec![sel(5, 10), sel(12, 16)]);
    let words = state.cursors().clone();
    // Both grow into the argument list and merge into one selection.
    assert!(state.expand_selection(&buffer, chain));
    assert_eq!(state.cursors().selections, vec![sel(4, 17)]);
    assert!(state.shrink_selection(&buffer));
    assert_eq!(*state.cursors(), words);
    assert!(state.shrink_selection(&buffer));
    assert_eq!(*state.cursors(), start);
}

#[test]
fn shrink_restores_a_backward_selection_with_its_direction() {
    let buffer = TextBuffer::from_text(TEXT);
    let mut state = state_with(&buffer, vec![sel(10, 5)], 0);
    assert!(state.expand_selection(&buffer, chain));
    assert_eq!(state.cursors().primary(), sel(4, 17));
    assert!(state.shrink_selection(&buffer));
    assert_eq!(state.cursors().primary(), sel(10, 5));
}

#[test]
fn a_caret_off_any_word_grows_to_the_smallest_enclosing_range() {
    let buffer = TextBuffer::from_text(TEXT);
    // Between `)` and `x`, two spaces from both: no word touches it.
    let mut state = state_with(&buffer, vec![caret(18)], 0);
    assert!(state.expand_selection(&buffer, chain));
    assert_eq!(state.cursors().primary(), sel(0, 20));
}

#[test]
fn a_selection_with_nothing_larger_is_left_alone_and_records_nothing() {
    let buffer = TextBuffer::from_text(TEXT);
    let mut state = state_with(&buffer, vec![caret(18)], 0);
    assert!(!state.expand_selection(&buffer, |_| Vec::new()));
    assert_eq!(state.cursors().primary(), caret(18));
    assert!(!state.shrink_selection(&buffer));
    // A candidate equal to the selection, or one not containing it, is no step.
    let mut state = state_with(&buffer, vec![sel(4, 17)], 0);
    assert!(!state.expand_selection(&buffer, |_| vec![span(4, 17), span(5, 18)]));
    assert_eq!(state.cursors().primary(), sel(4, 17));
}

#[test]
fn any_other_selection_change_ends_the_shrink_history() {
    let buffer = TextBuffer::from_text(TEXT);
    let mut state = state_with(&buffer, vec![caret(7)], 0);
    assert!(state.expand_selection(&buffer, chain));
    assert!(state.expand_selection(&buffer, chain));
    state.move_carets(&buffer, Motion::Right, true);
    let moved = state.cursors().clone();
    assert!(!state.shrink_selection(&buffer));
    assert_eq!(*state.cursors(), moved);
    // A fresh expansion starts a new history: one shrink goes back to `moved`.
    assert!(state.expand_selection(&buffer, chain));
    assert!(state.shrink_selection(&buffer));
    assert_eq!(*state.cursors(), moved);
    assert!(!state.shrink_selection(&buffer));
}

#[test]
fn a_change_made_elsewhere_ends_the_shrink_history() {
    let buffer = TextBuffer::from_text(TEXT);
    let mut state = state_with(&buffer, vec![caret(7)], 0);
    assert!(state.expand_selection(&buffer, chain));
    state.map_cursors(&buffer, |pos| pos);
    assert!(!state.shrink_selection(&buffer));
    assert_eq!(state.cursors().primary(), sel(5, 10));
}
