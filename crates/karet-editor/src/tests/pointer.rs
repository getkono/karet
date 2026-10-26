//! Pointer-originated caret moves never scroll visible text, and the vertical
//! reveal counts rows — collapsed folds, sticky headers — not buffer lines.

use super::*;
use crate::visual::*;

/// A 200-character line, so every column is distinct and off-screen columns
/// exist on both sides.
fn long_line() -> String {
    ('a'..='z').cycle().take(200).collect()
}

/// One line, a 3-cell gutter: 20 content cells.
const NARROW: Rect = Rect::new(0, 0, 23, 1);
const GUTTER: u16 = 3;

/// The symbol painted at content cell `x` of row 0.
fn cell(target: &Buffer, x: u16) -> String {
    target[(GUTTER + x, 0)].symbol().to_owned()
}

/// One hundred numbered lines.
fn hundred_lines() -> TextBuffer {
    let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
    TextBuffer::from_text(&text)
}

#[test]
fn a_click_on_a_visible_cell_never_scrolls_the_line() {
    let buffer = TextBuffer::from_text(&long_line());
    let mut state = EditorState::new();
    let mut target = Buffer::empty(NARROW);
    Editor::new(&buffer).render(NARROW, &mut target, &mut state);
    let under_pointer = cell(&target, 18);

    // Two cells from the right edge: inside the keyboard's margin.
    state.set_caret(&buffer, LineCol::new(0, 18));
    assert_eq!(state.scroll_col, 0, "resolved by the click");
    Editor::new(&buffer).render(NARROW, &mut target, &mut state);
    assert_eq!(state.scroll_col, 0, "and not revised by the render");
    assert_eq!(cell(&target, 18), under_pointer, "the text did not move");
    assert_eq!(
        state.primary_caret_cell(NARROW, &buffer, &[]),
        Some((GUTTER + 18, 0))
    );

    // The keyboard reveal of the same column keeps its margin.
    state.goto(&buffer, LineCol::new(0, 18));
    assert!(state.scroll_col > 0, "goto keeps the keyboard margin");
}

/// One of the editor's pointer-originated caret moves.
type PointerMove = fn(&mut EditorState, &TextBuffer, LineCol);

#[test]
fn every_pointer_move_keeps_a_visible_target_where_it_is() {
    let buffer = TextBuffer::from_text(&long_line());
    let mut target = Buffer::empty(NARROW);
    let edge = LineCol::new(0, 19);
    let moves: [(&str, PointerMove); 4] = [
        ("set_caret", |s, b, p| s.set_caret(b, p)),
        ("extend_to", |s, b, p| s.extend_to(b, p)),
        ("set_selection", |s, b, p| {
            s.set_selection(b, LineCol::new(0, 0), p);
        }),
        ("add_caret", |s, b, p| s.add_caret(b, p)),
    ];
    for (name, pointer_move) in moves {
        let mut state = EditorState::new();
        Editor::new(&buffer).render(NARROW, &mut target, &mut state);
        pointer_move(&mut state, &buffer, edge);
        Editor::new(&buffer).render(NARROW, &mut target, &mut state);
        assert_eq!(state.scroll_col, 0, "{name} scrolled a visible target");
    }
}

#[test]
fn a_pointer_move_off_screen_scrolls_by_the_least_that_shows_it() {
    let buffer = TextBuffer::from_text(&long_line());
    let mut state = EditorState::new();
    let mut target = Buffer::empty(NARROW);
    Editor::new(&buffer).render(NARROW, &mut target, &mut state);

    // A drag one cell past the right edge: the caret lands on the last cell.
    state.extend_to(&buffer, LineCol::new(0, 20));
    Editor::new(&buffer).render(NARROW, &mut target, &mut state);
    assert_eq!(state.scroll_col, 1);
    assert_eq!(
        state.primary_caret_cell(NARROW, &buffer, &[]),
        Some((GUTTER + 19, 0))
    );
}

#[test]
fn a_click_below_a_collapsed_fold_counts_rows_not_lines() {
    // Lines 2..=50 are folded away, so line 60 is drawn on row 11 of a
    // 30-row pane even though it is 60 buffer lines below the top.
    let buffer = hundred_lines();
    let folds = [Fold {
        start: 1,
        end: 50,
        collapsed: true,
    }];
    let area = Rect::new(0, 0, 20, 30);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let render = |state: &mut EditorState, target: &mut Buffer| {
        Editor::new(&buffer)
            .folds(&folds)
            .render(area, target, state);
    };
    render(&mut state, &mut target);

    state.set_caret(&buffer, LineCol::new(60, 0));
    assert_eq!(state.scroll_line, 0, "resolved by the click");
    render(&mut state, &mut target);
    assert_eq!(state.scroll_line, 0, "and not revised by the render");
    let gutter = 5;
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &folds),
        Some((gutter, 11))
    );

    // The keyboard counts rows the same way: one line down stays put, and a
    // line off the bottom lands on the last row.
    state.extend_down(&buffer);
    render(&mut state, &mut target);
    assert_eq!(state.scroll_line, 0, "line 61 is on row 12");
    state.goto(&buffer, LineCol::new(80, 0));
    render(&mut state, &mut target);
    assert_eq!(state.scroll_line, 51);
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &folds),
        Some((gutter, 29))
    );
}

#[test]
fn a_sticky_header_that_appears_does_not_hide_the_caret() {
    // Once the view scrolls past line 0, its header sticks and takes a row
    // from the 5-row pane, so the reveal must be re-checked against the
    // height the header leaves.
    let text: String = (0..20).map(|n| format!("l{n}\n")).collect();
    let buffer = TextBuffer::from_text(&text);
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 19,
    }]);
    let area = Rect::new(0, 0, 20, 5);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let render = |state: &mut EditorState, target: &mut Buffer| {
        Editor::new(&buffer)
            .semantic_blocks(&blocks)
            .sticky_scroll(true)
            .render(area, target, state);
    };
    render(&mut state, &mut target);
    for _ in 0..12 {
        state.extend_down(&buffer);
        render(&mut state, &mut target);
        let line = state.cursor().line;
        assert!(
            state.primary_caret_cell(area, &buffer, &[]).is_some(),
            "caret on line {line} hidden: top {}, sticky {:?}",
            state.scroll_line,
            state.frame.sticky_rows
        );
    }
}

#[test]
fn select_all_does_not_scroll() {
    let buffer = hundred_lines();
    let area = Rect::new(0, 0, 20, 10);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);

    state.select_all(&buffer);
    assert_eq!(state.scroll_line, 0);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(state.scroll_line, 0, "nor does the render after it");
    assert_eq!(state.cursor(), LineCol::new(100, 0));
}

#[test]
fn reveal_line_counts_shown_rows() {
    // Without folds a row is a line.
    assert_eq!(reveal_line(&[], 0, 9, 10, (0, u32::MAX)), 0, "last row");
    assert_eq!(reveal_line(&[], 0, 10, 10, (0, u32::MAX)), 1, "one below");
    assert_eq!(
        reveal_line(&[], 5, 2, 10, (0, u32::MAX)),
        2,
        "above: to the top row"
    );

    let fold = [Fold {
        start: 1,
        end: 50,
        collapsed: true,
    }];
    assert_eq!(reveal_line(&fold, 0, 60, 30, (0, u32::MAX)), 0, "row 11");
    assert_eq!(
        reveal_line(&fold, 0, 80, 30, (0, u32::MAX)),
        51,
        "walks back over shown rows"
    );
    assert_eq!(reveal_line(&fold, 0, 51, 3, (0, u32::MAX)), 0, "row 2 of 3");
    assert_eq!(
        reveal_line(&fold, 0, 52, 3, (0, u32::MAX)),
        1,
        "the walk back jumps the fold"
    );
    assert_eq!(
        reveal_line(&fold, 60, 20, 30, (0, u32::MAX)),
        1,
        "a hidden target is shown by its header"
    );
    assert_eq!(
        reveal_line(&fold, 10, 51, 1, (0, u32::MAX)),
        10,
        "a top inside the fold paints from the line after it"
    );

    let nested = [
        Fold {
            start: 1,
            end: 50,
            collapsed: true,
        },
        Fold {
            start: 10,
            end: 20,
            collapsed: true,
        },
    ];
    assert_eq!(
        reveal_line(&nested, 60, 15, 5, (0, u32::MAX)),
        1,
        "the outermost header"
    );
    // An expanded fold hides nothing.
    let open = [Fold {
        start: 1,
        end: 50,
        collapsed: false,
    }];
    assert_eq!(reveal_line(&open, 0, 60, 30, (0, u32::MAX)), 31);
}
