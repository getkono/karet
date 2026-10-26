//! Virtual text through the one path both placements share: where it paints,
//! what it reports it painted, and what it must leave alone.
//!
//! Inline text is covered column by column in `inlay.rs`; this module holds
//! the end-of-line slot and the cases where the two placements meet.

use karet_core::EolAlign;

use super::*;

/// The gutter of a document with fewer than ten lines: marker, digit, space.
const GUTTER: u16 = 3;

/// End-of-line text on `line` with `align` and `priority`.
fn eol(line: u32, text: &str, align: EolAlign, priority: u8) -> VirtualText {
    VirtualText {
        line,
        placement: VirtualTextPlacement::EndOfLine { align, priority },
        text: text.to_owned(),
        role: ThemeRole::Muted,
    }
}

/// The row `y` as painted.
fn row(target: &Buffer, y: u16) -> String {
    (target.area.x..target.area.right())
        .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// The text painted inside `rect`.
fn painted_in(target: &Buffer, rect: Rect) -> String {
    (rect.x..rect.right())
        .map(|x| target[(x, rect.y)].symbol().to_owned())
        .collect()
}

/// Render `text` with `virtual_text` into `area`, unwrapped.
fn render(text: &str, virtual_text: &[VirtualText], area: Rect) -> (Buffer, EditorState) {
    let buffer = TextBuffer::from_text(text);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .virtual_text(virtual_text)
        .render(area, &mut target, &mut state);
    (target, state)
}

#[test]
fn each_text_reports_the_cells_it_was_painted_in() {
    let buffer = TextBuffer::from_text("let x\nbody\n");
    let texts = [
        eol(0, "  Ada", EolAlign::Left, 0),
        VirtualText::inline(0, 5, ": i32", ThemeRole::InlayHint),
    ];
    let folds = [Fold {
        start: 0,
        end: 1,
        collapsed: true,
    }];
    let area = Rect::new(0, 0, 30, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .folds(&folds)
        .virtual_text(&texts)
        .render(area, &mut target, &mut state);

    // `let x`, the trailing inline text, ` ⋯`, and only then the slot: not at
    // the end-of-line caret cell, where the inline text begins.
    assert_eq!(row(&target, 0), " 1▸let x: i32 ⋯  Ada          ");
    let Some(slot) = state.virtual_text_rect(0) else {
        unreachable!("the end-of-line text is on screen");
    };
    assert_eq!(slot, Rect::new(GUTTER + 12, 0, 5, 1));
    assert_eq!(painted_in(&target, slot), "  Ada");
    assert_eq!(
        state.virtual_text_rect(1),
        Some(Rect::new(GUTTER + 5, 0, 5, 1))
    );
    assert_eq!(state.virtual_text_rect(2), None, "no such text");
}

#[test]
fn a_slot_too_narrow_cuts_the_text_short_and_reports_what_was_painted() {
    let area = Rect::new(4, 0, 14, 1);
    let (target, state) = render(
        "let x\n",
        &[eol(0, "  Ada, a long while ago", EolAlign::Left, 0)],
        area,
    );

    // Gutter 3 + `let x` 5 leaves six of the editor's fourteen cells.
    let rect = Rect::new(4 + GUTTER + 5, 0, 6, 1);
    assert_eq!(state.virtual_text_rect(0), Some(rect));
    assert_eq!(painted_in(&target, rect), "  Ada…");
}

#[test]
fn right_aligned_text_keeps_to_the_right_edge() {
    let area = Rect::new(0, 0, 20, 1);
    let (target, state) = render(
        "let x\n",
        &[
            eol(0, "[R]", EolAlign::Right, 0),
            eol(0, "  L", EolAlign::Left, 0),
        ],
        area,
    );

    assert_eq!(row(&target, 0), " 1 let x  L      [R]");
    assert_eq!(state.virtual_text_rect(0), Some(Rect::new(17, 0, 3, 1)));
    assert_eq!(
        state.virtual_text_rect(1),
        Some(Rect::new(GUTTER + 5, 0, 3, 1))
    );
}

#[test]
fn the_higher_priority_keeps_its_cells_when_the_slot_is_short() {
    // Ten cells of slot: the advisory takes its seven, and the blame is cut
    // to the three left over.
    let area = Rect::new(0, 0, 18, 1);
    let (target, state) = render(
        "let x\n",
        &[
            eol(0, "  blame here", EolAlign::Left, 0),
            eol(0, "  ✗ 1.0", EolAlign::Left, 5),
        ],
        area,
    );

    assert_eq!(row(&target, 0), " 1 let x  …  ✗ 1.0");
    assert_eq!(
        state.virtual_text_rect(1),
        Some(Rect::new(GUTTER + 8, 0, 7, 1))
    );
}

#[test]
fn a_click_on_end_of_line_text_lands_at_the_end_of_the_line() {
    let buffer = TextBuffer::from_text("let x\nnext\n");
    let texts = [eol(0, "  Ada", EolAlign::Left, 0)];
    let area = Rect::new(0, 0, 20, 2);
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(0, 5));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .virtual_text(&texts)
        .focused(true)
        .render(area, &mut target, &mut state);

    for x in GUTTER + 5..GUTTER + 10 {
        assert_eq!(
            state.pos_at(area, &buffer, &[], x, 0),
            LineCol::new(0, 5),
            "cell {x}"
        );
        assert_eq!(
            state.char_pos_at(area, &buffer, &[], x, 0),
            LineCol::new(0, 5)
        );
    }
    // The end-of-line caret takes the slot's first cell: the text supplies
    // the gap that keeps the two apart.
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((GUTTER + 5, 0))
    );
}

#[test]
fn end_of_line_text_goes_on_the_last_wrapped_row_and_moves_no_break() {
    let text = "alpha beta gamma";
    let area = Rect::new(0, 0, 16, 3);
    let paint = |texts: &[VirtualText]| {
        let buffer = TextBuffer::from_text(text);
        let mut state = EditorState::new();
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .virtual_text(texts)
            .word_wrap(true)
            .render(area, &mut target, &mut state);
        target
    };
    let plain = paint(&[]);
    let annotated = paint(&[eol(0, "  Ada was here", EolAlign::Left, 0)]);

    assert_eq!(row(&annotated, 0), row(&plain, 0));
    assert_eq!(row(&annotated, 0), " 1 alpha beta   ");
    // The eight cells `gamma` leaves, and no more.
    assert_eq!(row(&annotated, 1), "   gamma  Ada w…");
}

#[test]
fn end_of_line_text_moves_neither_the_reveal_nor_the_scroll_extent() {
    let area = Rect::new(0, 0, 13, 1);
    let measure = |texts: &[VirtualText]| {
        let buffer = TextBuffer::from_text("let x\n");
        let mut state = EditorState::new();
        let mut target = Buffer::empty(area);
        let mut render = |state: &mut EditorState| {
            Editor::new(&buffer)
                .virtual_text(texts)
                .render(area, &mut target, state);
        };
        render(&mut state);
        state.goto(&buffer, LineCol::new(0, 5));
        render(&mut state);
        (state.scroll_col, state.longest_col())
    };

    assert_eq!(
        measure(&[eol(0, "  Ada, a long while ago", EolAlign::Left, 0)]),
        measure(&[])
    );
}

#[test]
fn end_of_line_text_stays_off_a_line_scrolled_past_its_end() {
    let buffer = TextBuffer::from_text("fn f()\na_very_long_line_of_text_here\n");
    let texts = [eol(0, "  E", EolAlign::Left, 0)];
    let area = Rect::new(0, 0, 13, 2);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let mut render = |state: &mut EditorState| {
        Editor::new(&buffer)
            .virtual_text(&texts)
            .render(area, &mut target, state);
    };
    render(&mut state);
    state.goto(&buffer, LineCol::new(1, 30));
    render(&mut state);

    assert!(state.scroll_col > 6, "scrolled past `fn f()`");
    assert_eq!(state.virtual_text_rect(0), None);
}

#[test]
fn a_sticky_row_paints_its_inline_text_and_a_click_counts_it() {
    let buffer = TextBuffer::from_text("fn f(a) {\n\tx\n\ty\n\tz\n}\n");
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 4,
    }]);
    let texts = [VirtualText::inline(0, 5, "n: ", ThemeRole::InlayHint)];
    let area = Rect::new(0, 0, 30, 3);
    let mut state = EditorState::new();
    state.scroll_line = 2;
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .semantic_blocks(&blocks)
        .sticky_scroll(true)
        .virtual_text(&texts)
        .render(area, &mut target, &mut state);
    assert_eq!(
        state.frame.sticky_rows,
        vec![0],
        "line 0 is the sticky header"
    );

    assert!(
        row(&target, 0).starts_with(" 1 fn f(n: a) {"),
        "{:?}",
        row(&target, 0)
    );
    // The `a` is three cells right of where the buffer alone would put it.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 8, 0),
        LineCol::new(0, 5)
    );
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 9, 0),
        LineCol::new(0, 6)
    );
    // A click on the hint lands on the column it annotates.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 6, 0),
        LineCol::new(0, 5)
    );
    assert_eq!(
        state.virtual_text_rect(0),
        Some(Rect::new(GUTTER + 5, 0, 3, 1))
    );
}
