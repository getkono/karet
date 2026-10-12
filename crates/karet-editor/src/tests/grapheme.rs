//! Editing steps over extended grapheme clusters, never through one: a
//! combining sequence, a ZWJ emoji, a regional-indicator flag and a wide CJK
//! character each behave as the single character the user sees, for caret
//! motion, deletion, word bounds, cell widths, wrapping and hit-testing.

use super::*;
use crate::editing;
use crate::visual::cluster_widths;

/// `e` + COMBINING ACUTE ACCENT: one cluster of two chars.
const E_ACUTE: &str = "e\u{301}";
/// MAN ZWJ WOMAN ZWJ GIRL: one cluster of five chars, two cells wide.
const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
/// The flags of Japan and the United States: two clusters of two regional
/// indicators each.
const FLAGS: &str = "\u{1F1EF}\u{1F1F5}\u{1F1FA}\u{1F1F8}";

/// The gutter of a document with fewer than ten lines: marker, digit, space.
const GUTTER: u16 = 3;

fn at(line: u32, col: u32) -> LineCol {
    LineCol::new(line, col)
}

fn chars(text: &str) -> Vec<char> {
    text.chars().collect()
}

/// A state over `buffer`, rendered once, with its caret at `col` on line 0.
fn state_at(buffer: &TextBuffer, col: u32) -> EditorState {
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 40, 4);
    let mut target = Buffer::empty(area);
    Editor::new(buffer).render(area, &mut target, &mut state);
    state.set_carets(&[at(0, col)]);
    state
}

/// The primary caret's column after each `motion` in turn.
fn walk(buffer: &TextBuffer, col: u32, motion: Motion, steps: usize) -> Vec<u32> {
    let mut state = state_at(buffer, col);
    (0..steps)
        .map(|_| {
            state.move_carets(buffer, motion, false);
            state.cursor().col
        })
        .collect()
}

/// The one range an edit deletes.
fn deleted(edit: Option<editing::Edit>) -> Option<Range> {
    let edit = edit?;
    assert_eq!(edit.change.edits.len(), 1);
    let first = edit.change.edits.first()?;
    assert!(first.new_text.is_empty());
    assert_eq!(edit.caret, first.range.start);
    Some(first.range)
}

fn span(start: u32, end: u32) -> Option<Range> {
    Some(Range {
        start: at(0, start),
        end: at(0, end),
    })
}

#[test]
fn cluster_bounds_mark_only_user_perceived_character_edges() {
    assert_eq!(cluster_bounds(&chars("")), vec![0]);
    assert_eq!(
        cluster_bounds(&chars(&format!("{E_ACUTE}x"))),
        vec![0, 2, 3]
    );
    assert_eq!(
        cluster_bounds(&chars(&format!("a{FAMILY}b"))),
        vec![0, 1, 6, 7]
    );
    assert_eq!(cluster_bounds(&chars(FLAGS)), vec![0, 2, 4]);
    assert_eq!(cluster_bounds(&chars("日本")), vec![0, 1, 2]);
}

#[test]
fn a_cluster_takes_its_width_on_its_first_char_only() {
    assert_eq!(
        cluster_widths(&chars(&format!("a{FAMILY}b"))),
        vec![1, 2, 0, 0, 0, 0, 1]
    );
    assert_eq!(cluster_widths(&chars(FLAGS)), vec![2, 0, 2, 0]);
    assert_eq!(cluster_widths(&chars(E_ACUTE)), vec![1, 0]);
    assert_eq!(cluster_widths(&chars("日本")), vec![2, 2]);
    // A tab is its own cluster; its expansion is decided where it lands.
    assert_eq!(cluster_widths(&chars("\tx")), vec![0, 1]);
}

#[test]
fn left_and_right_step_over_a_combining_sequence() {
    let buffer = TextBuffer::from_text(&format!("{E_ACUTE}x\n"));
    assert_eq!(walk(&buffer, 0, Motion::Right, 2), vec![2, 3]);
    assert_eq!(walk(&buffer, 3, Motion::Left, 3), vec![2, 0, 0]);
}

#[test]
fn left_and_right_step_over_a_zwj_sequence_and_flags() {
    let family = TextBuffer::from_text(&format!("a{FAMILY}b\n"));
    assert_eq!(walk(&family, 0, Motion::Right, 3), vec![1, 6, 7]);
    assert_eq!(walk(&family, 7, Motion::Left, 3), vec![6, 1, 0]);

    let flags = TextBuffer::from_text(&format!("{FLAGS}\n"));
    assert_eq!(walk(&flags, 0, Motion::Right, 2), vec![2, 4]);
    assert_eq!(walk(&flags, 4, Motion::Left, 2), vec![2, 0]);
}

#[test]
fn shift_right_extends_a_selection_by_whole_clusters() {
    let buffer = TextBuffer::from_text(&format!("{FAMILY}b\n"));
    let mut state = state_at(&buffer, 0);
    state.move_carets(&buffer, Motion::Right, true);
    assert_eq!(state.selection_range(), span(0, 5));
}

#[test]
fn backspace_and_delete_remove_a_whole_cluster() {
    let family = TextBuffer::from_text(&format!("a{FAMILY}\n"));
    assert_eq!(
        deleted(editing::backspace(at(0, 6), None, &family, 0)),
        span(1, 6)
    );
    assert_eq!(
        deleted(editing::delete_forward(at(0, 1), None, &family, 0)),
        span(1, 6)
    );

    let flags = TextBuffer::from_text(&format!("{FLAGS}\n"));
    assert_eq!(
        deleted(editing::backspace(at(0, 4), None, &flags, 0)),
        span(2, 4)
    );
    assert_eq!(
        deleted(editing::delete_forward(at(0, 0), None, &flags, 0)),
        span(0, 2)
    );

    let accent = TextBuffer::from_text(&format!("{E_ACUTE}x\n"));
    assert_eq!(
        deleted(editing::backspace(at(0, 2), None, &accent, 0)),
        span(0, 2)
    );

    // A wide CJK character is one char and one cluster: unchanged.
    let cjk = TextBuffer::from_text("日本\n");
    assert_eq!(
        deleted(editing::backspace(at(0, 2), None, &cjk, 0)),
        span(1, 2)
    );
}

#[test]
fn a_combining_mark_stays_inside_its_word() {
    // `café` spelled with a combining accent: `caf`, `e`, U+0301.
    let buffer = TextBuffer::from_text(&format!("caf{E_ACUTE} x\n"));
    assert_eq!(next_word_boundary(&buffer, at(0, 0)), at(0, 5));
    assert_eq!(previous_word_boundary(&buffer, at(0, 5)), at(0, 0));
    assert_eq!(word_bounds(&buffer, at(0, 2)), (at(0, 0), at(0, 5)));
    assert_eq!(identifier_at(&buffer, at(0, 4)), Some((at(0, 0), at(0, 5))));
    assert_eq!(class_run_at(&buffer, at(0, 1)), (at(0, 0), at(0, 5)));
}

#[test]
fn word_motions_treat_an_emoji_sequence_as_one_symbol() {
    let buffer = TextBuffer::from_text(&format!("ab {FAMILY} cd\n"));
    assert_eq!(next_word_boundary(&buffer, at(0, 2)), at(0, 8));
    assert_eq!(previous_word_boundary(&buffer, at(0, 8)), at(0, 3));
    assert_eq!(class_run_at(&buffer, at(0, 5)), (at(0, 3), at(0, 8)));
    // Off a word, a double-click falls back to the whole cluster.
    assert_eq!(word_bounds(&buffer, at(0, 3)), (at(0, 3), at(0, 8)));
    assert_eq!(identifier_at(&buffer, at(0, 4)), None);
}

#[test]
fn a_zwj_sequence_paints_two_cells_and_hit_tests_as_one_character() {
    let buffer = TextBuffer::from_text(&format!("{FAMILY}x\n"));
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(target[(GUTTER, 0)].symbol(), FAMILY);
    assert_eq!(target[(GUTTER + 2, 0)].symbol(), "x");

    // Left half before the cluster, right half after it; never inside.
    assert_eq!(state.pos_at(area, &buffer, &[], GUTTER, 0), at(0, 0));
    assert_eq!(state.pos_at(area, &buffer, &[], GUTTER + 1, 0), at(0, 5));
    assert_eq!(state.pos_at(area, &buffer, &[], GUTTER + 2, 0), at(0, 5));
    assert_eq!(
        state.char_pos_at(area, &buffer, &[], GUTTER + 1, 0),
        at(0, 0)
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], at(0, 5)),
        Some((GUTTER + 2, 0))
    );
}

#[test]
fn soft_wrap_never_splits_a_cluster() {
    // Three content cells: `ab` fills two, and the two-cell family cannot
    // follow, so it moves to the next row whole.
    let buffer = TextBuffer::from_text(&format!("ab{FAMILY}\n"));
    let area = Rect::new(0, 0, GUTTER + 3, 3);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut target, &mut state);
    assert_eq!(target[(GUTTER, 1)].symbol(), FAMILY);
    assert_eq!(
        state.screen_cell(area, &buffer, &[], at(0, 2)),
        Some((GUTTER, 1))
    );
    assert_eq!(state.pos_at(area, &buffer, &[], GUTTER + 1, 1), at(0, 7));
}

#[test]
fn a_tab_after_a_wide_cluster_stops_where_it_is_painted() {
    // The flag takes two cells, so the tab fills two more to the stop at 4.
    let buffer = TextBuffer::from_text("\u{1F1EF}\u{1F1F5}\tx\n");
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(target[(GUTTER + 4, 0)].symbol(), "x");
    assert_eq!(
        state.screen_cell(area, &buffer, &[], at(0, 3)),
        Some((GUTTER + 4, 0))
    );
}
