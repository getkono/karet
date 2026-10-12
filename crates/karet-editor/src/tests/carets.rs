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

fn span(line: u32, start: u32, end: u32) -> Range {
    Range {
        start: LineCol::new(line, start),
        end: LineCol::new(line, end),
    }
}

#[test]
fn add_next_occurrence_reaches_every_match_then_stops() {
    // Started on the last of three: the search wraps to the first, then must go
    // on to the middle one rather than finding the first again.
    let buffer = TextBuffer::from_text("foo foo foo");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 9));
    state.add_next_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 8, 11)]);
    state.add_next_occurrence(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 3));
    state.add_next_occurrence(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
    let all = vec![span(0, 0, 3), span(0, 4, 7), span(0, 8, 11)];
    assert_eq!(state.selection_ranges(), all);
    // Every occurrence is selected: a further press changes nothing.
    state.add_next_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), all);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
}

#[test]
fn add_next_occurrence_from_a_caret_matches_whole_words_only() {
    let buffer = TextBuffer::from_text("id width\nid_x id");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 2)); // touching the end of `id`
    for _ in 0..3 {
        state.add_next_occurrence(&buffer);
    }
    // `wid`th and `id_x` contain `id` but are other words.
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 2), span(1, 5, 7)]);
}

#[test]
fn add_next_occurrence_from_a_selection_matches_inside_words() {
    let buffer = TextBuffer::from_text("id width");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.set_selection(&buffer, LineCol::new(0, 0), LineCol::new(0, 2));
    state.add_next_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 0, 2), span(0, 4, 6)]);
}

#[test]
fn add_next_occurrence_word_run_ends_when_the_user_reselects() {
    // A whole-word run, then the user selects the same text themselves: that
    // selection is theirs, so it matches inside words.
    let buffer = TextBuffer::from_text("id width");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 0));
    state.add_next_occurrence(&buffer);
    state.set_selection(&buffer, LineCol::new(0, 2), LineCol::new(0, 0));
    state.add_next_occurrence(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 6));
}

#[test]
fn add_next_occurrence_off_a_word_does_nothing() {
    let buffer = TextBuffer::from_text("a  b");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 2)); // between the two spaces
    state.add_next_occurrence(&buffer);
    assert_eq!(state.selection_range(), None);
    assert_eq!(state.cursor(), LineCol::new(0, 2));
}

#[test]
fn add_next_occurrence_matches_a_selection_spanning_lines() {
    let buffer = TextBuffer::from_text("ab\ncd ab\ncd");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.set_selection(&buffer, LineCol::new(0, 0), LineCol::new(1, 2));
    state.add_next_occurrence(&buffer);
    assert_eq!(state.cursor(), LineCol::new(2, 2));
    assert!(state.selection_ranges().contains(&Range {
        start: LineCol::new(1, 3),
        end: LineCol::new(2, 2),
    }));
}

#[test]
fn add_next_occurrence_handles_multibyte_text() {
    // Columns are chars, not bytes: `é` is two bytes.
    let buffer = TextBuffer::from_text("é ab é ab");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 2));
    state.add_next_occurrence(&buffer);
    state.add_next_occurrence(&buffer);
    assert_eq!(state.selection_ranges(), vec![span(0, 2, 4), span(0, 7, 9)]);
}

#[test]
fn add_next_occurrence_wraps_across_rope_chunks() {
    // A buffer spanning many rope chunks: the scan must cross chunk boundaries
    // and wrap from the end of the document back to its start.
    let filler = "x".repeat(40);
    let mut text = String::from("foo\n");
    for _ in 0..500 {
        text.push_str(&filler);
        text.push('\n');
    }
    text.push_str("bar foo\n");
    let buffer = TextBuffer::from_text(&text);
    let mut state = EditorState::new();
    state.last_height = 4;
    state.set_selection(&buffer, at(501, 4), at(501, 7));
    state.add_next_occurrence(&buffer);
    assert!(state.has_multiple_cursors());
    assert!(state.selection_ranges().contains(&Range {
        start: at(0, 0),
        end: at(0, 3),
    }));
}

fn at(line: u32, col: u32) -> LineCol {
    LineCol::new(line, col)
}

fn heads(state: &EditorState) -> Vec<LineCol> {
    state.cursors().selections.iter().map(|s| s.head).collect()
}

#[test]
fn a_plain_motion_collapses_each_selection_to_its_leading_edge() {
    let buffer = TextBuffer::from_text("abcde\nfghij\nklmno");
    let forward = Selection {
        anchor: at(1, 1),
        head: at(1, 4),
    };
    let backward = Selection {
        anchor: forward.head,
        head: forward.anchor,
    };
    // (motion, where it lands from a forward and from a backward selection
    // over `ghi`): Left/Right stop at the edge, the rest move on from it.
    for (motion, want) in [
        (Motion::Left, at(1, 1)),
        (Motion::Right, at(1, 4)),
        (Motion::Up, at(0, 1)),
        (Motion::Down, at(2, 4)),
        (Motion::LineStart, at(1, 0)),
        (Motion::LineEnd, at(1, 5)),
        (Motion::PageUp, at(0, 1)),
        (Motion::PageDown, at(2, 4)),
    ] {
        for sel in [forward, backward] {
            let mut state = EditorState::new();
            state.last_height = 4;
            state.set_cursor_state(&buffer, CursorState::single(sel));
            state.move_carets(&buffer, motion, false);
            assert_eq!(
                state.cursors().selections,
                vec![Selection::caret(want)],
                "{motion:?} from {sel:?}"
            );
        }
    }
}

#[test]
fn an_extending_motion_moves_heads_and_keeps_anchors() {
    let buffer = TextBuffer::from_text("abcde");
    let mut state = EditorState::new();
    state.set_cursor_state(
        &buffer,
        CursorState {
            selections: vec![Selection::caret(at(0, 1)), Selection::caret(at(0, 3))],
            primary: 1,
        },
    );
    state.move_carets(&buffer, Motion::Right, true);
    // The selections now touch at (0, 3) and stay apart.
    assert_eq!(
        state.cursors().selections,
        vec![
            Selection {
                anchor: at(0, 1),
                head: at(0, 2),
            },
            Selection {
                anchor: at(0, 3),
                head: at(0, 4),
            },
        ]
    );
    assert_eq!(state.cursors().primary, 1);
}

#[test]
fn plain_motions_move_carets_independently() {
    let buffer = TextBuffer::from_text("abc\nabc\nabc");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.set_carets(&[at(0, 1), at(1, 1)]);
    state.move_carets(&buffer, Motion::Down, false);
    assert_eq!(heads(&state), vec![at(1, 1), at(2, 1)]);
    // Carets that end on one position merge.
    state.move_carets(&buffer, Motion::DocStart, false);
    assert_eq!(heads(&state), vec![at(0, 0)]);
}

#[test]
fn removing_a_caret_keeps_the_primary_on_the_same_caret() {
    let buffer = TextBuffer::from_text("abc\nabc\nabc");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(at(0, 1));
    state.add_caret(&buffer, at(2, 1));
    state.add_caret(&buffer, at(1, 1));
    assert_eq!(state.cursor(), at(1, 1));
    // Removing a caret before the primary shifts its index, not its role.
    state.add_caret(&buffer, at(0, 1));
    assert_eq!(heads(&state), vec![at(1, 1), at(2, 1)]);
    assert_eq!(state.cursor(), at(1, 1));
    // Removing the primary itself hands the role to the caret before it, or
    // the next one when it was first.
    state.add_caret(&buffer, at(1, 1));
    assert_eq!(state.cursor(), at(2, 1));
}

#[test]
fn collapse_to_original_keeps_the_first_placed_caret_and_its_selection() {
    let buffer = TextBuffer::from_text("abc\nabc\nabc");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.set_selection(&buffer, at(1, 0), at(1, 2));
    state.add_caret_above(&buffer);
    state.add_caret(&buffer, at(2, 3));
    assert_eq!(state.cursors().selections.len(), 3);
    state.collapse_to_original(&buffer);
    assert_eq!(
        state.cursors().selections,
        vec![Selection {
            anchor: at(1, 0),
            head: at(1, 2),
        }]
    );
    assert_eq!(state.cursors().primary, 0);
}

#[test]
fn the_original_caret_survives_a_merge_and_a_reset() {
    let buffer = TextBuffer::from_text("abcdef");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(at(0, 1));
    state.add_caret(&buffer, at(0, 0));
    state.add_caret(&buffer, at(0, 4));
    // The original meets the caret at the start of the document and merges
    // with it; the merged caret is the original.
    state.move_carets(&buffer, Motion::Left, false);
    assert_eq!(heads(&state), vec![at(0, 0), at(0, 3)]);
    state.collapse_to_original(&buffer);
    assert_eq!(heads(&state), vec![at(0, 0)]);

    // After the set becomes one caret, a new set's original is that caret.
    state.goto(&buffer, at(0, 4));
    state.add_caret(&buffer, at(0, 0));
    state.collapse_to_original(&buffer);
    assert_eq!(heads(&state), vec![at(0, 4)]);
}

#[test]
fn word_bounds_spans_the_word_under_pos() {
    let buffer = TextBuffer::from_text("foo bar");
    assert_eq!(
        word_bounds(&buffer, LineCol::new(0, 5)),
        (LineCol::new(0, 4), LineCol::new(0, 7))
    );
}

#[test]
fn gutter_marker_line_hits_only_the_marker_column() {
    let buffer = TextBuffer::from_text("a\nb\nc\nd\n");
    let mut state = EditorState::new();
    state.last_height = 4;
    let area = Rect::new(0, 0, 20, 4);
    assert_eq!(
        state.gutter_marker_line_at(area, &buffer, &[], 0, 2),
        Some(2)
    );
    assert_eq!(state.gutter_marker_line_at(area, &buffer, &[], 1, 2), None);
    assert_eq!(state.gutter_marker_line_at(area, &buffer, &[], 10, 2), None);
    state.scroll_line = 1;
    assert_eq!(
        state.gutter_marker_line_at(area, &buffer, &[], 0, 0),
        Some(1)
    );
}
