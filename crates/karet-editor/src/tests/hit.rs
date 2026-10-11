//! Click hit-testing lands exactly where the user pointed: every case resolves
//! through the same layout the painter used, so the cell under the pointer and
//! the caret a click produces agree.

use super::*;

/// The gutter of a document with fewer than ten lines: marker, digit, space.
const GUTTER: u16 = 3;

/// The symbol painted at `(x, y)`.
fn symbol(target: &Buffer, x: u16, y: u16) -> String {
    target[(x, y)].symbol().to_owned()
}

#[test]
fn a_click_on_a_sticky_row_counts_its_tabs_as_painted() {
    let buffer = TextBuffer::from_text("\tfunc (s *S) Foo() {\n\ta\n\tb\n\tc\n}\n");
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 4,
    }]);
    let area = Rect::new(0, 0, 30, 3);
    let mut state = EditorState::new();
    state.scroll_line = 2;
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .semantic_blocks(&blocks)
        .sticky_scroll(true)
        .render(area, &mut target, &mut state);
    assert_eq!(state.sticky_rows, vec![0], "line 0 is the sticky header");

    // The tab expands to four cells, so `Foo`'s `F` (column 13) paints at
    // content cell 16, not 13.
    let x = GUTTER + 16;
    assert_eq!(symbol(&target, x, 0), "F");
    assert_eq!(state.pos_at(area, &buffer, &[], x, 0), LineCol::new(0, 13));
}

#[test]
fn a_click_rounds_to_the_nearer_edge_of_a_wide_character() {
    let buffer = TextBuffer::from_text("日本語\n");
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(symbol(&target, GUTTER, 0), "日");

    // `日` covers content cells 0-1 and `本` cells 2-3: a left half lands
    // before the character, a right half after it.
    let expected = [0, 1, 1, 2, 2, 3];
    for (offset, col) in (0_u16..).zip(expected) {
        assert_eq!(
            state.pos_at(area, &buffer, &[], GUTTER + offset, 0),
            LineCol::new(0, col),
            "content cell {offset}"
        );
    }
}

#[test]
fn a_click_rounds_to_the_nearer_edge_of_a_tab() {
    let buffer = TextBuffer::from_text("\tx\n");
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(symbol(&target, GUTTER + 4, 0), "x");

    // The tab covers cells 0-3: the first two are nearer its start, the last
    // two nearer the `x`.
    let expected = [0, 0, 1, 1, 1];
    for (offset, col) in (0_u16..).zip(expected) {
        assert_eq!(
            state.pos_at(area, &buffer, &[], GUTTER + offset, 0),
            LineCol::new(0, col),
            "content cell {offset}"
        );
    }
}

/// `alpha beta gamma` wrapped into ten content cells: `alpha ` on row 0
/// (columns 0..6) and `beta gamma` on row 1 (6..16), so column 6 is both the
/// end of row 0 and the start of row 1.
fn wrapped(height: u16) -> (TextBuffer, Rect, EditorState) {
    let buffer = TextBuffer::from_text("alpha beta gamma");
    let area = Rect::new(0, 0, 13, height);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut target, &mut state);
    (buffer, area, state)
}

/// Render `state` wrapped and report the primary caret's cell.
fn caret_after_render(
    buffer: &TextBuffer,
    area: Rect,
    state: &mut EditorState,
) -> Option<(u16, u16)> {
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .word_wrap(true)
        .render(area, &mut target, state);
    state.primary_caret_cell(area, buffer, &[])
}

#[test]
fn a_click_past_the_end_of_a_wrapped_row_keeps_the_caret_on_that_row() {
    let (buffer, area, mut state) = wrapped(3);
    let pos = state.pos_at(area, &buffer, &[], GUTTER + 8, 0);
    assert_eq!(pos, LineCol::new(0, 6));
    state.set_caret(&buffer, pos);
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER + 6, 0)),
        "drawn at the end of the clicked row, not the start of the next"
    );
}

#[test]
fn a_click_at_the_start_of_a_continuation_row_draws_the_caret_there() {
    let (buffer, area, mut state) = wrapped(3);
    let pos = state.pos_at(area, &buffer, &[], GUTTER, 1);
    assert_eq!(pos, LineCol::new(0, 6));
    state.set_caret(&buffer, pos);
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER, 1))
    );
}

#[test]
fn a_drag_past_the_end_of_a_wrapped_row_keeps_the_head_on_that_row() {
    let (buffer, area, mut state) = wrapped(3);
    state.set_caret(&buffer, LineCol::new(0, 0));
    let pos = state.pos_at(area, &buffer, &[], GUTTER + 9, 0);
    state.extend_to(&buffer, pos);
    assert_eq!(state.selection_range().map(|r| r.end), Some(pos));
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER + 6, 0))
    );
}

#[test]
fn a_keyboard_move_drops_the_wrap_affinity() {
    let (buffer, area, mut state) = wrapped(3);
    let pos = state.pos_at(area, &buffer, &[], GUTTER + 8, 0);
    state.set_caret(&buffer, pos);
    state.move_right(&buffer);
    state.move_left(&buffer);
    assert_eq!(state.cursor(), pos);
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER, 1)),
        "a caret the keyboard placed at a wrap point opens the next row"
    );
}

#[test]
fn a_caret_placed_past_the_bottom_row_does_not_scroll_it_away() {
    // Only row 0 is on screen; the click past its end must not reveal row 1.
    let (buffer, area, mut state) = wrapped(1);
    let pos = state.pos_at(area, &buffer, &[], GUTTER + 8, 0);
    state.set_caret(&buffer, pos);
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER + 6, 0))
    );
    assert_eq!(state.scroll_subrow, 0);
}

#[test]
fn a_stale_pointer_hit_does_not_reach_a_different_caret() {
    let (buffer, area, mut state) = wrapped(3);
    let _ = state.pos_at(area, &buffer, &[], GUTTER + 8, 0);
    // Placed somewhere other than the hit: no affinity, and the hit is spent.
    state.set_caret(&buffer, LineCol::new(0, 2));
    state.set_caret(&buffer, LineCol::new(0, 6));
    assert_eq!(
        caret_after_render(&buffer, area, &mut state),
        Some((GUTTER, 1))
    );
}

#[test]
fn a_click_below_the_last_line_lands_at_the_end_of_the_document() {
    let buffer = TextBuffer::from_text("one\ntwo three");
    let area = Rect::new(0, 0, 20, 5);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);

    // At the left edge, not the clicked column's spot on the last line.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER, 4),
        LineCol::new(1, 9)
    );
    // The last line itself still resolves by column.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER, 1),
        LineCol::new(1, 0)
    );
}

#[test]
fn a_click_below_a_wrapped_last_line_lands_at_the_end_of_the_document() {
    let buffer = TextBuffer::from_text("alpha beta gamma");
    // Ten content cells: the line wraps onto two rows.
    let area = Rect::new(0, 0, 13, 5);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut target, &mut state);

    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER, 3),
        LineCol::new(0, 16)
    );
}

#[test]
fn a_click_below_a_collapsed_last_fold_lands_at_the_end_of_its_header() {
    let buffer = TextBuffer::from_text("head\nbody\ntail");
    let folds = [Fold {
        start: 0,
        end: 2,
        collapsed: true,
    }];
    let area = Rect::new(0, 0, 20, 4);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .folds(&folds)
        .render(area, &mut target, &mut state);

    // The last row shown is the header, so that is where the document ends
    // on screen.
    assert_eq!(
        state.pos_at(area, &buffer, &folds, GUTTER, 2),
        LineCol::new(0, 4)
    );
}

/// `before` inline text `text` over line 0.
fn before_text(text: &str) -> Vec<Decoration> {
    Range::new(LineCol::new(0, 0), LineCol::new(0, 1))
        .map(|range| Decoration {
            range,
            kind: DecorationKind::InlineText {
                text: text.to_owned(),
                before: true,
            },
            role: None,
        })
        .into_iter()
        .collect()
}

/// The row `y` as painted.
fn row(target: &Buffer, y: u16) -> String {
    (0..target.area.width)
        .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[test]
fn before_inline_text_shifts_clicks_and_the_caret_as_it_shifts_the_text() {
    let buffer = TextBuffer::from_text("abc\ndef");
    let decorations = before_text("» ");
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(0, 1));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .decorations(&decorations)
        .render(area, &mut target, &mut state);

    assert_eq!(symbol(&target, GUTTER + 2, 0), "a");
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 3, 0),
        LineCol::new(0, 1)
    );
    // A click on the inline text itself lands on the line's first column.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER, 0),
        LineCol::new(0, 0)
    );
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((GUTTER + 3, 0)),
        "drawn on the `b`"
    );
    // The undecorated line is untouched.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 1, 1),
        LineCol::new(1, 1)
    );
}

#[test]
fn before_inline_text_takes_its_cells_from_the_first_wrapped_row() {
    let buffer = TextBuffer::from_text("alpha beta gamma");
    let decorations = before_text("»»»» ");
    let area = Rect::new(0, 0, 13, 3);
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(0, 3));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .decorations(&decorations)
        .word_wrap(true)
        .render(area, &mut target, &mut state);

    // Five cells of text leave room for `alpha` alone on the first row.
    assert!(
        row(&target, 0).ends_with("»»»» alpha"),
        "{:?}",
        row(&target, 0)
    );
    assert!(
        row(&target, 1).starts_with("    beta "),
        "{:?}",
        row(&target, 1)
    );
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((GUTTER + 8, 0))
    );
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 8, 0),
        LineCol::new(0, 3)
    );
    // The continuation row carries no inline text.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 1, 1),
        LineCol::new(0, 6)
    );
}

#[test]
fn before_inline_text_narrows_the_room_a_reveal_scrolls_into() {
    let line: String = ('a'..='z').collect();
    let buffer = TextBuffer::from_text(&line);
    let decorations = before_text("»»»» ");
    let area = Rect::new(0, 0, 13, 1);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let mut render = |state: &mut EditorState| {
        Editor::new(&buffer)
            .decorations(&decorations)
            .render(area, &mut target, state);
    };
    render(&mut state);
    state.goto(&buffer, LineCol::new(0, 26));
    render(&mut state);
    let Some((x, _)) = state.primary_caret_cell(area, &buffer, &[]) else {
        unreachable!(
            "the end of the line is revealed, scrolled {}",
            state.scroll_col
        );
    };
    assert!(x < area.right());
}
