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
fn the_caret_at_a_hints_anchor_sits_before_the_hint() {
    let text = "let count = items.len();\n";
    let hints = [hint(0, 9, ": i32")];

    // Column 8 is the last `t` of `count`.
    let (last_t, ..) = geometry(text, &hints, LineCol::new(0, 8), 40);
    // Column 9 is the hint's anchor: the end of `count`, where typing extends
    // the identifier. The caret belongs against that text, not past `: i32`.
    let (anchor, ..) = geometry(text, &hints, LineCol::new(0, 9), 40);
    // Column 10 is past the space the hint precedes.
    let (past, ..) = geometry(text, &hints, LineCol::new(0, 10), 40);

    let (Some((last_t_x, _)), Some((anchor_x, _)), Some((past_x, _))) = (last_t, anchor, past)
    else {
        unreachable!("every column is on screen in a 40-wide viewport");
    };
    // Gutter is 3 cells, so `count` ends at screen 12: the caret sits right
    // after the `t`, on the first cell of the hint.
    assert_eq!(anchor_x, 12);
    assert_eq!(anchor_x - last_t_x, 1, "caret at the anchor left its text");
    // Stepping right clears the five cells of hint and the space together.
    assert_eq!(past_x - anchor_x, 6, "caret did not step over the hint");
}

#[test]
fn the_caret_at_a_trailing_hints_anchor_sits_before_it() {
    // End of line is the common case: a return-type hint after `fn f()`, and
    // the caret where the next typed character will go.
    let (cell, ..) = geometry("fn f()\n", &[hint(0, 6, " -> i32")], LineCol::new(0, 6), 40);
    assert_eq!(cell, Some((9, 0)));
}

#[test]
fn typing_at_a_hints_anchor_lands_where_the_caret_is_drawn() {
    // The user-visible contract: the character typed appears in the cell the
    // caret occupied, pushing the hint right, not on the far side of it.
    let hints = [hint(0, 9, ": i32")];
    let (before, ..) = geometry("let count = 1;\n", &hints, LineCol::new(0, 9), 40);
    let row = painted("let countx = 1;\n", &[hint(0, 10, ": i32")], 40);
    let Some((x, _)) = before else {
        unreachable!("the caret is on screen in a 40-wide viewport");
    };
    assert_eq!(row.chars().nth(usize::from(x)), Some('x'), "got {row:?}");
}

#[test]
fn a_click_inside_a_hint_lands_on_the_column_it_annotates() {
    let text = "let count = items.len();\n";
    let hints = [hint(0, 9, ": i32")];
    let area = Rect::new(0, 0, 40, 4);
    let (_, state, buffer) = geometry(text, &hints, LineCol::new(0, 0), 40);

    // The gutter is `marker + one digit + space` = 3 columns, so buffer column
    // 0 starts at screen 3. `let count` fills screen 3..12, the hint occupies
    // screen 12..17, and the space it annotates lands at 17. Every hint cell
    // resolves to the anchor, whose caret is drawn at 12 -- the hint's first
    // cell -- so a click on an annotation never moves the caret past it.
    for x in 12..17 {
        assert_eq!(
            state.pos_at(area, &buffer, &[], x, 0),
            LineCol::new(0, 9),
            "a click on hint cell {x} should select the column it annotates"
        );
    }

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
fn a_hint_on_a_wrap_boundary_belongs_to_the_row_it_starts() {
    // The row that begins at a hinted column paints that hint first, after the
    // caret slot of its anchor. So the caret for the row's first column sits
    // at offset 0, and the row's first character follows the hint.
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
    // The anchor's caret opens the row, ahead of the hint.
    assert_eq!(offset_within_row(&chars, 5, 5, 4, hints), 0);
    // The next column is past two cells of hint and the `b` at column 5.
    assert_eq!(offset_within_row(&chars, 5, 6, 4, hints), 3);
    // And both invert: offset 0 and every hint cell resolve to the anchor.
    for offset in 0..3 {
        assert_eq!(
            source_col_at_display_offset(&chars, 5, 10, offset, 4, hints),
            5,
            "offset {offset} left the anchor"
        );
    }
}

#[test]
fn a_hint_at_column_zero_still_round_trips() {
    // Column 0 is where the caret-before-hint convention is most visible: the
    // hint renders at the very start of the line, and the caret at column 0
    // sits ahead of it, in the line's first cell.
    let chars: Vec<char> = "value".chars().collect();
    let index = crate::hint::HintIndex::new(&[hint(0, 0, "let ")]);
    let hints = index.line(0);

    assert_eq!(display_col(&chars, 0, 4, hints), 0);
    assert_eq!(offset_within_row(&chars, 0, 0, 4, hints), 0);
    // Column 1 follows the four cells of hint and the `v`.
    assert_eq!(display_col(&chars, 1, 4, hints), 5);
    for col in 0..=chars.len() as u32 {
        let screen = offset_within_row(&chars, 0, col, 4, hints);
        let back = source_col_at_display_offset(&chars, 0, chars.len() as u32, screen, 4, hints);
        assert_eq!(back, col, "column {col} did not round trip");
    }
}

#[test]
fn a_trailing_hint_stays_off_a_line_scrolled_past_its_end() {
    // An unwrapped row carries `u32::MAX` as its end, so "is this the last
    // row" is always true. Without also checking the row's *start*, a line
    // scrolled entirely off to the left painted its trailing hint at the left
    // margin, floating over nothing.
    let buffer = TextBuffer::from_text("fn f()\na_very_long_line_of_text_here\n");
    let mut state = EditorState::new();
    state.scroll_col = 20;
    let area = Rect::new(0, 0, 40, 2);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .inlay_hints(&[hint(0, 6, " -> i32")])
        .render(area, &mut target, &mut state);
    let row: String = (0..area.width)
        .map(|x| target[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        !row.contains("i32"),
        "a hint painted on a row scrolled past its line: {row:?}"
    );
}

#[test]
fn a_tab_after_the_row_start_lands_where_the_mapping_says() {
    // The painter walks the skipped prefix to keep its running column, and a
    // tab expands from that column. Omitting the hint widths in the prefix put
    // the painted tab on a different stop than `display_col` computed, so the
    // caret sat beside the character it belonged to.
    let chars: Vec<char> = "abc\tX".chars().collect();
    let index = crate::hint::HintIndex::new(&[hint(0, 0, "ZZ")]);
    let hints = index.line(0);

    // `ZZ` then `abc` puts the tab at screen 5, so it fills to the stop at 8.
    assert_eq!(display_col(&chars, 3, 4, hints), 5);
    assert_eq!(display_col(&chars, 4, 4, hints), 8);

    // Row-relative from a scrolled start, the same stop has to come out: the
    // absolute column 8 minus the row's origin (column 1 renders at 3, and the
    // row carries no leading hint of its own).
    assert_eq!(offset_within_row(&chars, 1, 4, 4, hints), 5);
}
