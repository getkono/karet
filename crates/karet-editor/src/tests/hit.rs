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
