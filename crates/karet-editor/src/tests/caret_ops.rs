//! Select All Occurrences, Skip Occurrence, Add Cursors to Line Ends, and Undo
//! Last Cursor Operation. Split from `carets.rs` to respect the per-file
//! code-line ceiling.

use karet_core::Change;
use karet_core::TextEdit;

use crate::*;

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

/// A state with a viewport, so reveals have somewhere to scroll.
fn state_at(line: u32, col: u32) -> EditorState {
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(line, col));
    state
}

/// Every caret's head, in document order.
fn heads(state: &EditorState) -> Vec<LineCol> {
    state.cursors().selections.iter().map(|s| s.head).collect()
}

#[test]
fn select_all_occurrences_from_a_caret_takes_every_whole_word() {
    // `food` contains `foo` but is another word.
    let buffer = TextBuffer::from_text("foo food foo\nfoo");
    let mut state = state_at(0, 10); // inside the second whole `foo`
    state.select_all_occurrences(&buffer);
    assert_eq!(
        state.selection_ranges(),
        vec![span(0, 0, 3), span(0, 9, 12), span(1, 0, 3)]
    );
    // The primary stays on the occurrence the caret was in.
    assert_eq!(state.cursor(), LineCol::new(0, 12));
}

#[test]
fn select_all_occurrences_from_a_selection_matches_inside_words() {
    let buffer = TextBuffer::from_text("foo food");
    let mut state = state_at(0, 0);
    state.set_selection(&buffer, LineCol::new(0, 1), LineCol::new(0, 3));
    state.select_all_occurrences(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 1, 3), span(0, 5, 7)]);
    assert_eq!(state.cursor(), LineCol::new(0, 3));
}

#[test]
fn select_all_occurrences_keeps_a_whole_word_ctrl_d_run_whole_word() {
    let buffer = TextBuffer::from_text("id width id");
    let mut state = state_at(0, 1);
    state.add_next_occurrence(&buffer);
    state.select_all_occurrences(&buffer);
    assert_eq!(
        state.selection_ranges(),
        vec![span(0, 0, 2), span(0, 9, 11)]
    );
}

#[test]
fn select_all_occurrences_off_a_word_changes_nothing() {
    let buffer = TextBuffer::from_text("a  b");
    let mut state = state_at(0, 2);
    state.select_all_occurrences(&buffer);
    assert_eq!(heads(&state), vec![LineCol::new(0, 2)]);
    assert!(state.selection_ranges().is_empty());
    // A press that changed nothing records nothing to undo.
    assert!(!state.undo_caret_operation(&buffer));
}

#[test]
fn select_all_occurrences_keeps_a_primary_an_earlier_match_overlaps() {
    // A left-to-right scan for `aa` in `aaa` finds 0..2 and never 1..3.
    let buffer = TextBuffer::from_text("aaa");
    let mut state = state_at(0, 0);
    state.set_selection(&buffer, LineCol::new(0, 1), LineCol::new(0, 3));
    state.select_all_occurrences(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 1, 3)]);
    assert_eq!(state.cursor(), LineCol::new(0, 3));
}

#[test]
fn skip_occurrence_moves_the_last_match_on_and_wraps() {
    let buffer = TextBuffer::from_text("foo foo foo");
    let mut state = state_at(0, 1);
    state.add_next_occurrence(&buffer); // 0..3
    state.add_next_occurrence(&buffer); // + 4..7
    state.skip_occurrence(&buffer);
    assert_eq!(
        state.selection_ranges(),
        vec![span(0, 0, 3), span(0, 8, 11)]
    );
    assert_eq!(state.cursor(), LineCol::new(0, 11));
    // Past the last occurrence it wraps, passing over 0..3, to the one it skipped.
    state.skip_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 3), span(0, 4, 7)]);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
}

#[test]
fn skip_occurrence_from_a_caret_selects_the_word_then_keeps_whole_words() {
    let buffer = TextBuffer::from_text("foo food foo");
    let mut state = state_at(0, 1);
    state.skip_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 3)]);
    state.skip_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 9, 12)]);
}

#[test]
fn skip_occurrence_without_another_occurrence_changes_nothing() {
    let buffer = TextBuffer::from_text("foo bar");
    let mut state = state_at(0, 0);
    state.set_selection(&buffer, LineCol::new(0, 0), LineCol::new(0, 3));
    state.skip_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 3)]);
    assert!(!state.undo_caret_operation(&buffer));
}

#[test]
fn add_cursors_to_line_ends_puts_a_caret_on_every_selected_line() {
    let buffer = TextBuffer::from_text("ab\ncde\nfgh");
    let mut state = state_at(0, 0);
    state.set_selection(&buffer, LineCol::new(0, 1), LineCol::new(2, 1));
    state.add_cursors_to_line_ends(&buffer);
    assert_eq!(
        heads(&state),
        vec![LineCol::new(0, 2), LineCol::new(1, 3), LineCol::new(2, 1)]
    );
    assert!(state.selection_ranges().is_empty());
    // The primary is the last caret the primary selection produced.
    assert_eq!(state.cursor(), LineCol::new(2, 1));
}

#[test]
fn add_cursors_to_line_ends_skips_a_last_line_the_selection_only_reaches() {
    let buffer = TextBuffer::from_text("ab\ncde\nfgh");
    let mut state = state_at(0, 0);
    // Reversed, ending at the start of line 2: that line gets no caret.
    state.set_selection(&buffer, LineCol::new(2, 0), LineCol::new(0, 0));
    state.add_cursors_to_line_ends(&buffer);
    assert_eq!(heads(&state), vec![LineCol::new(0, 2), LineCol::new(1, 3)]);
    assert_eq!(state.cursor(), LineCol::new(1, 3));
}

#[test]
fn add_cursors_to_line_ends_turns_a_one_line_selection_into_its_end() {
    let buffer = TextBuffer::from_text("abcd");
    let mut state = state_at(0, 0);
    state.set_selection(&buffer, LineCol::new(0, 3), LineCol::new(0, 1));
    state.add_cursors_to_line_ends(&buffer);
    assert_eq!(heads(&state), vec![LineCol::new(0, 3)]);
    assert!(state.selection_ranges().is_empty());
}

#[test]
fn undo_caret_operation_walks_back_through_each_operation() {
    let buffer = TextBuffer::from_text("foo foo foo");
    let mut state = state_at(0, 1);
    state.add_next_occurrence(&buffer);
    state.add_next_occurrence(&buffer);
    state.add_caret(&buffer, LineCol::new(0, 9));
    assert!(state.undo_caret_operation(&buffer));
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 3), span(0, 4, 7)]);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
    assert!(state.undo_caret_operation(&buffer));
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 3)]);
    assert!(state.undo_caret_operation(&buffer));
    assert_eq!(heads(&state), vec![LineCol::new(0, 1)]);
    assert!(state.selection_ranges().is_empty());
    assert!(!state.undo_caret_operation(&buffer));
}

#[test]
fn undo_caret_operation_restores_a_whole_word_run() {
    let buffer = TextBuffer::from_text("id width id");
    let mut state = state_at(0, 1);
    state.add_next_occurrence(&buffer); // `id`, whole-word run
    state.add_next_occurrence(&buffer); // + the last `id`
    assert!(state.undo_caret_operation(&buffer));
    // The run resumes whole-word: `wid`th is passed over again.
    state.add_next_occurrence(&buffer);
    assert_eq!(
        state.selection_ranges(),
        vec![span(0, 0, 2), span(0, 9, 11)]
    );
}

#[test]
fn undo_caret_operation_covers_vertical_carets_and_line_ends() {
    let buffer = TextBuffer::from_text("ab\ncd\nef");
    let mut state = state_at(0, 0);
    state.add_caret_below(&buffer);
    assert!(state.has_multiple_cursors());
    assert!(state.undo_caret_operation(&buffer));
    assert_eq!(heads(&state), vec![LineCol::new(0, 0)]);
    state.set_selection(&buffer, LineCol::new(0, 0), LineCol::new(2, 2));
    state.add_cursors_to_line_ends(&buffer);
    assert!(state.undo_caret_operation(&buffer));
    assert_eq!(
        state.selection_ranges(),
        vec![Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(2, 2),
        }]
    );
}

#[test]
fn undo_caret_operation_restores_nothing_once_the_text_changed() {
    let mut buffer = TextBuffer::from_text("foo foo");
    let mut state = state_at(0, 1);
    state.add_next_occurrence(&buffer);
    state.add_next_occurrence(&buffer);
    let edit = TextEdit {
        range: span(0, 0, 0),
        new_text: "x".to_string(),
    };
    assert!(
        buffer
            .apply_simple(&Change::new(buffer.version(), vec![edit]))
            .is_ok()
    );
    let before = state.cursors().clone();
    assert!(!state.undo_caret_operation(&buffer));
    assert_eq!(*state.cursors(), before);
    // The stale history is gone, not merely skipped.
    assert!(!state.undo_caret_operation(&buffer));
}

#[test]
fn undo_caret_operation_history_is_bounded() {
    let text = vec!["x"; 80].join("\n");
    let buffer = TextBuffer::from_text(&text);
    let mut state = state_at(0, 0);
    for _ in 0..60 {
        state.add_caret_below(&buffer);
    }
    let undone = (0..60)
        .take_while(|_| state.undo_caret_operation(&buffer))
        .count();
    assert_eq!(undone, 50);
    // The oldest ten fell off: what remains is the set after the tenth add.
    assert_eq!(state.cursors().selections.len(), 11);
}
