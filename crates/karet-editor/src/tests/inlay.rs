//! Column-accurate inlay hints: what they paint, and what they must not move.
//!
//! The hard part is not drawing them. It is that a hint occupies screen cells
//! no buffer column owns, so every mapping between the two has to agree about
//! it — otherwise the caret sits one place, a click resolves to another, and a
//! wrapped line breaks where nothing is.

use karet_core::InlayHintKind;

use super::*;
use crate::visual::*;

/// A type hint of `label` rendering before `col` on `line`.
fn hint(line: u32, col: u32, label: &str) -> InlayHint {
    InlayHint {
        position: LineCol::new(line, col),
        label: label.to_owned(),
        kind: InlayHintKind::Type,
        padding_left: false,
        padding_right: false,
    }
}

/// Render `text` with `hints` and return the first row as a string.
fn painted(text: &str, hints: &[InlayHint], width: u16) -> String {
    let buffer = TextBuffer::from_text(text);
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, width, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .inlay_hints(hints)
        .render(area, &mut target, &mut state);
    (0..area.width)
        .map(|x| target[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// Render, then report where the caret for `at` landed and what `pos_at`
/// resolves each screen column back to.
fn geometry(
    text: &str,
    hints: &[InlayHint],
    at: LineCol,
    width: u16,
) -> (Option<(u16, u16)>, EditorState, TextBuffer) {
    let buffer = TextBuffer::from_text(text);
    let mut state = EditorState::new();
    state.set_caret(&buffer, at);
    let area = Rect::new(0, 0, width, 4);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .inlay_hints(hints)
        .focused(true)
        .render(area, &mut target, &mut state);
    let cell = caret_cell(area, &buffer, &[], &state, at);
    (cell, state, buffer)
}

#[test]
fn a_hint_paints_before_the_character_it_is_anchored_to() {
    // `let count` is columns 0..9, so a hint at 9 lands between `count` and the
    // space, which is where an inferred type belongs.
    let row = painted("let count = items.len();\n", &[hint(0, 9, ": i32")], 40);
    assert!(
        row.starts_with(" 1 let count: i32 = items.len();"),
        "got {row:?}"
    );
}

#[test]
fn without_hints_nothing_about_the_row_changes() {
    // The regression guard the whole mapping change hangs on: the no-hint path
    // has to be exactly what it was.
    let plain = painted("let count = items.len();\n", &[], 40);
    assert!(
        plain.starts_with(" 1 let count = items.len();"),
        "got {plain:?}"
    );
}

#[test]
fn the_caret_sits_after_the_hint_not_inside_it() {
    let text = "let count = items.len();\n";
    let hints = [hint(0, 9, ": i32")];

    // Column 8 is the last `t` of `count`, still ahead of the hint.
    let (before, ..) = geometry(text, &hints, LineCol::new(0, 8), 40);
    // Column 9 is the space, which now renders after five cells of hint.
    let (after, ..) = geometry(text, &hints, LineCol::new(0, 9), 40);

    let (Some((before_x, _)), Some((after_x, _))) = (before, after) else {
        unreachable!("both columns are on screen in a 40-wide viewport");
    };
    // One character of `t` plus five of `: i32`: the caret steps over the hint
    // rather than into it.
    assert_eq!(after_x - before_x, 6, "caret did not clear the hint");
}

#[test]
fn a_click_inside_a_hint_lands_on_the_column_it_annotates() {
    let text = "let count = items.len();\n";
    let hints = [hint(0, 9, ": i32")];
    let area = Rect::new(0, 0, 40, 4);
    let (_, state, buffer) = geometry(text, &hints, LineCol::new(0, 0), 40);

    // The gutter is `marker + one digit + space` = 3 columns, so buffer column
    // 0 starts at screen 3. `let count` fills screen 3..12, the hint occupies
    // screen 12..17, and the space it annotates lands at 17.
    let inside = state.pos_at(area, &buffer, &[], 14, 0);
    assert_eq!(
        inside,
        LineCol::new(0, 9),
        "a click on the annotation should select the column it annotates"
    );

    // Immediately left of the hint is still `count`'s last character.
    let before = state.pos_at(area, &buffer, &[], 11, 0);
    assert_eq!(before, LineCol::new(0, 8));

    // Immediately right of it is the space the hint precedes.
    let after = state.pos_at(area, &buffer, &[], 17, 0);
    assert_eq!(after, LineCol::new(0, 9));
}

#[test]
fn every_column_round_trips_through_the_mapping() {
    // The invariant that makes the caret and the mouse agree:
    // `source_col_at_display_offset(display_col(c)) == c` for every column of a
    // hinted line. A break here is a caret that drifts as you arrow along.
    let chars: Vec<char> = "let count = items.len();".chars().collect();
    let index = crate::hint::HintIndex::new(&[
        hint(0, 4, "«"),
        hint(0, 9, ": i32"),
        hint(0, 12, " /*x*/ "),
    ]);
    let hints = index.line(0);

    for col in 0..=chars.len() as u32 {
        let screen = display_col(&chars, col, 4, hints);
        let back = source_col_at_display_offset(&chars, 0, chars.len() as u32, screen, 4, hints);
        assert_eq!(
            back, col,
            "column {col} did not round trip (screen {screen})"
        );
    }
}

#[test]
fn a_tab_still_reaches_its_stop_once_a_hint_has_shifted_the_line() {
    // A tab's width is measured from the real screen column, so a hint ahead of
    // it changes how far it expands. Ignoring that would put every tab on the
    // wrong stop for the rest of the line.
    let chars: Vec<char> = "a\tb".chars().collect();
    let plain = crate::hint::HintIndex::new(&[]);
    let shifted = crate::hint::HintIndex::new(&[hint(0, 1, "xx")]);

    // Without the hint: `a` at 0, tab fills 1..4, `b` at 4.
    assert_eq!(display_col(&chars, 2, 4, plain.line(0)), 4);
    // With two cells inserted before the tab: `a`, then the hint at 1..3, then
    // the tab fills 3..4, so `b` still lands on the stop at 4.
    assert_eq!(display_col(&chars, 2, 4, shifted.line(0)), 4);
}

#[test]
fn a_trailing_hint_paints_past_the_last_character() {
    // A return-type hint is anchored one past the end of the line, where there
    // is no character for it to precede.
    let row = painted("fn f()\n", &[hint(0, 6, " -> i32")], 40);
    assert!(row.starts_with(" 1 fn f() -> i32"), "got {row:?}");
}

#[test]
fn a_hint_is_not_part_of_the_buffer_so_the_line_is_unchanged() {
    // Virtual text: it must never reach the document. A hint that edited the
    // buffer would be saved to disk.
    let buffer = TextBuffer::from_text("let count = items.len();\n");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 40, 2);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .inlay_hints(&[hint(0, 9, ": i32")])
        .render(area, &mut target, &mut state);
    assert_eq!(
        buffer.line(0).unwrap_or_default(),
        "let count = items.len();"
    );
}

#[test]
fn wrapping_counts_the_cells_a_hint_consumes() {
    // A hint makes the line wider, so it wraps sooner. Measuring the wrap
    // without it would overflow the viewport by the hint's width.
    let buffer = TextBuffer::from_text("aaaa bbbb cccc\n");
    let index = crate::hint::HintIndex::new(&[hint(0, 0, "12345")]);
    let empty = crate::hint::HintIndex::new(&[]);

    let unhinted = visual_ranges(
        &buffer,
        0,
        Layout {
            width: 10,
            tab_width: 4,
            unwrapped_lines: &[],
            hints: &empty,
        },
    );
    let hinted = visual_ranges(
        &buffer,
        0,
        Layout {
            width: 10,
            tab_width: 4,
            unwrapped_lines: &[],
            hints: &index,
        },
    );
    // Five of the ten cells go to the hint, so only five characters fit on the
    // first row instead of ten. Measuring the wrap without the hint would have
    // painted ten characters into a row that had room for five.
    assert_eq!(unhinted.first().map(|range| range.end), Some(10));
    assert_eq!(hinted.first().map(|range| range.end), Some(5));
}

#[test]
fn a_hint_on_a_wrap_boundary_does_not_sit_under_the_caret() {
    // The subtlest case in the mapping. A row paints the hint anchored at its
    // own first column, and `display_col(start)` has already counted that hint
    // -- so a naive `display_col(c) - display_col(start)` puts the caret at
    // offset 0, on top of the annotation it should follow.
    let buffer = TextBuffer::from_text("aaaa bbbb cccc\n");
    let index = crate::hint::HintIndex::new(&[hint(0, 5, ">>")]);
    let layout = Layout {
        width: 10,
        tab_width: 4,
        unwrapped_lines: &[],
        hints: &index,
    };
    let ranges = visual_ranges(&buffer, 0, layout);
    // The hint is at column 5, which is where the second row begins.
    assert!(
        ranges.iter().any(|range| range.start == 5),
        "expected a row starting at the hinted column: {ranges:?}"
    );

    let chars: Vec<char> = "aaaa bbbb cccc".chars().collect();
    let hints = index.line(0);
    // Two cells of hint precede the row's first character, so the caret for
    // that character sits after them rather than on them.
    assert_eq!(offset_within_row(&chars, 5, 5, 4, hints), 2);
    // And the next column is one character further along.
    assert_eq!(offset_within_row(&chars, 5, 6, 4, hints), 3);
}

#[test]
fn a_hint_at_column_zero_still_round_trips() {
    // Column 0 is the one place where "hints at or before `col`" and "hints
    // strictly before `col`" differ most visibly: the hint renders at the very
    // start of the line, and the caret at column 0 must follow it.
    let chars: Vec<char> = "value".chars().collect();
    let index = crate::hint::HintIndex::new(&[hint(0, 0, "let ")]);
    let hints = index.line(0);

    assert_eq!(display_col(&chars, 0, 4, hints), 4);
    // Row-relative: the caret for column 0 sits four cells in, after the hint.
    assert_eq!(offset_within_row(&chars, 0, 0, 4, hints), 4);
    for col in 0..=chars.len() as u32 {
        let screen = offset_within_row(&chars, 0, col, 4, hints);
        let back = source_col_at_display_offset(&chars, 0, chars.len() as u32, screen, 4, hints);
        assert_eq!(back, col, "column {col} did not round trip");
    }
}
