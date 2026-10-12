//! The display options: line-number modes, the cursor-line highlight,
//! rulers, and the keyboard reveal's vertical margin (`scroll_off`).

use crate::*;

/// `count` numbered lines.
fn lines(count: u32) -> TextBuffer {
    let text: String = (0..count).map(|n| format!("line {n}\n")).collect();
    TextBuffer::from_text(&text)
}

/// The text painted on row `y`, from column `x` for `width` cells.
fn row_text(target: &Buffer, x: u16, y: u16, width: u16) -> String {
    (x..x + width)
        .map(|x| target[(x, y)].symbol().to_owned())
        .collect()
}

fn role(theme: &Theme, role: ThemeRole) -> ratatui::style::Color {
    theme.role(role).to_ratatui()
}

#[test]
fn relative_numbers_count_from_the_caret_which_shows_its_own() {
    // Ten lines take two digits, so every number column is 3 cells wide.
    let buffer = lines(10);
    let area = Rect::new(0, 0, 12, 5);
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(2, 0));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .line_numbers(LineNumbers::Relative)
        .render(area, &mut target, &mut state);
    let gutters: Vec<String> = (0..5).map(|y| row_text(&target, 0, y, 4)).collect();
    assert_eq!(gutters, ["  2 ", "  1 ", "  3 ", "  1 ", "  2 "]);
    assert_eq!(row_text(&target, 4, 2, 6), "line 2", "the text is unmoved");
}

#[test]
fn relative_numbers_on_a_wrapped_line_number_only_its_first_row() {
    let buffer = TextBuffer::from_text("abcdefgh\nxy\n");
    let area = Rect::new(0, 0, 8, 4);
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(1, 0));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .line_numbers(LineNumbers::Relative)
        .render(area, &mut target, &mut state);
    let gutters: Vec<String> = (0..4).map(|y| row_text(&target, 0, y, 3)).collect();
    // `abcdefgh` wraps over two 5-cell rows; then the caret line and the
    // trailing empty line.
    assert_eq!(gutters, [" 1 ", "   ", " 2 ", " 1 "]);
}

#[test]
fn numbers_off_narrow_the_gutter_and_hit_testing_follows() {
    let buffer = lines(100);
    let area = Rect::new(0, 0, 12, 3);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .line_numbers(LineNumbers::Off)
        .render(area, &mut target, &mut state);
    assert_eq!(row_text(&target, 0, 1, 8), "  line 1", "marker + spacer");
    assert_eq!(state.pos_at(area, &buffer, &[], 2, 1), LineCol::new(1, 0));
    assert_eq!(state.pos_at(area, &buffer, &[], 7, 1), LineCol::new(1, 5));
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((2, 0)),
        "the caret is drawn where the text starts"
    );
}

#[test]
fn cursor_line_off_leaves_the_caret_row_on_the_base_background() {
    let theme = Theme::dark();
    let buffer = lines(3);
    let area = Rect::new(0, 0, 12, 3);
    let render = |highlight: bool| {
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(1, 0));
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .theme(&theme)
            .cursor_line(highlight)
            .render(area, &mut target, &mut state);
        target
    };
    let on = render(true);
    assert_eq!(on[(8, 1)].bg, role(&theme, ThemeRole::CursorLine));

    let off = render(false);
    assert_eq!(off[(8, 1)].bg, role(&theme, ThemeRole::Background));
    assert_eq!(
        off[(1, 1)].fg,
        role(&theme, ThemeRole::LineNumberActive),
        "the caret line's number stays emphasized"
    );
    assert_eq!(off[(1, 0)].fg, role(&theme, ThemeRole::LineNumber));
}

#[test]
fn rulers_tint_their_column_on_every_row_except_over_a_selection() {
    let theme = Theme::dark();
    let ruler = role(&theme, ThemeRole::IndentGuide);
    // Three lines: a 3-cell gutter, so column 4 is cell 7.
    let buffer = lines(3);
    let area = Rect::new(0, 0, 14, 5);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let editor = || Editor::new(&buffer).theme(&theme).rulers(&[4, 40]);
    editor().render(area, &mut target, &mut state);
    state.set_selection(&buffer, LineCol::new(1, 2), LineCol::new(1, 6));
    editor().render(area, &mut target, &mut state);
    for y in [0, 2, 3, 4] {
        assert_eq!(
            target[(7, y)].bg,
            ruler,
            "row {y}, rows past the end included"
        );
        assert_ne!(target[(6, y)].bg, ruler, "row {y}: only the ruler's column");
    }
    assert_eq!(target[(7, 0)].symbol(), " ", "under `line 0`'s space");
    assert_eq!(target[(8, 0)].symbol(), "0");
    assert_eq!(
        target[(7, 1)].bg,
        role(&theme, ThemeRole::Selection),
        "a selection the ruler crosses keeps its colour"
    );
}

#[test]
fn a_ruler_moves_with_horizontal_scroll_and_leaves_with_it() {
    let theme = Theme::dark();
    let ruler = role(&theme, ThemeRole::IndentGuide);
    let buffer = TextBuffer::from_text(&"x".repeat(60));
    let area = Rect::new(0, 0, 13, 1);
    let mut state = EditorState::new();
    state.scroll_to_column(4);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .theme(&theme)
        .rulers(&[6, 2])
        .render(area, &mut target, &mut state);
    assert_eq!(state.scroll_col, 4);
    let tinted: Vec<u16> = (0..13).filter(|&x| target[(x, 0)].bg == ruler).collect();
    assert_eq!(
        tinted,
        [3 + 2],
        "column 6 two cells in; column 2 scrolled off"
    );
}

#[test]
fn a_sticky_header_is_numbered_in_the_active_mode() {
    // Eleven lines (two digits): line 0's block covers the rest, so once the
    // view scrolls past it, it sticks to the top row.
    let buffer = lines(10);
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 9,
    }]);
    let area = Rect::new(0, 0, 14, 5);
    let render = |numbers: LineNumbers| {
        let mut state = EditorState::new();
        state.place_caret(LineCol::new(6, 0));
        state.scroll_line = 4;
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .semantic_blocks(&blocks)
            .sticky_scroll(true)
            .line_numbers(numbers)
            .render(area, &mut target, &mut state);
        assert_eq!(state.sticky_rows, vec![0], "line 0 is the sticky header");
        row_text(&target, 0, 0, 10)
    };
    assert_eq!(render(LineNumbers::Absolute), "  1 line 0");
    assert_eq!(
        render(LineNumbers::Relative),
        "  6 line 0",
        "six lines above the caret"
    );
    assert_eq!(
        render(LineNumbers::Off),
        "  line 0  ",
        "the narrowed gutter: marker + spacer"
    );
}

/// Render `buffer` in a 10-row viewport with a 3-line keyboard margin.
fn paint(buffer: &TextBuffer, state: &mut EditorState, wrap: bool) {
    let area = Rect::new(0, 0, 20, 10);
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .scroll_off(3)
        .word_wrap(wrap)
        .render(area, &mut target, state);
}

#[test]
fn scroll_off_keeps_rows_below_and_above_a_keyboard_caret() {
    for wrap in [false, true] {
        let buffer = lines(30);
        let mut state = EditorState::new();
        paint(&buffer, &mut state, wrap);
        for _ in 0..6 {
            state.move_down(&buffer);
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 0, "line 6 has three rows below it");
        state.move_down(&buffer);
        if !wrap {
            // A wrapped view leaves the margin to the render, in rows.
            assert_eq!(state.scroll_line, 1, "resolved by the motion");
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 1, "and kept by the render");

        state.goto(&buffer, LineCol::new(20, 0));
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 14, "line 20 three rows from the bottom");
        for _ in 0..4 {
            state.move_up(&buffer);
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 13, "line 16 three rows from the top");
    }
}

#[test]
fn a_wrapped_margin_counts_rows_not_lines() {
    // Thirty-one lines take a 4-cell gutter, leaving 12 cells: every
    // 20-character line wraps onto two rows.
    let text: String = (0..30).map(|_| format!("{}\n", "a".repeat(20))).collect();
    let buffer = TextBuffer::from_text(&text);
    let area = Rect::new(0, 0, 16, 10);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let mut paint = |state: &mut EditorState| {
        Editor::new(&buffer)
            .scroll_off(3)
            .word_wrap(true)
            .render(area, &mut target, state);
    };
    paint(&mut state);
    state.goto(&buffer, LineCol::new(3, 0));
    paint(&mut state);
    assert_eq!(state.scroll_line, 0, "row 6 has three rows below it");
    state.goto(&buffer, LineCol::new(4, 0));
    paint(&mut state);
    assert_eq!(
        (state.scroll_line, state.scroll_subrow),
        (1, 0),
        "row 8 scrolls two rows, not three lines"
    );
    state.goto(&buffer, LineCol::new(2, 0));
    paint(&mut state);
    assert_eq!(
        (state.scroll_line, state.scroll_subrow),
        (0, 1),
        "three rows above: line 0's second row on top"
    );
}

#[test]
fn scroll_off_stops_at_either_end_of_the_buffer() {
    for wrap in [false, true] {
        // 30 lines and the trailing empty one: line 30 is the last.
        let buffer = lines(30);
        let mut state = EditorState::new();
        paint(&buffer, &mut state, wrap);
        state.move_doc_end(&buffer);
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 21, "the last line on the bottom row");
        state.move_doc_start(&buffer);
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line, 0);
    }
}

#[test]
fn a_click_inside_the_margin_does_not_scroll() {
    let buffer = lines(30);
    let mut state = EditorState::new();
    paint(&buffer, &mut state, false);
    state.set_caret(&buffer, LineCol::new(9, 0));
    paint(&buffer, &mut state, false);
    assert_eq!(state.scroll_line, 0, "the bottom row stays put");
}

#[test]
fn scroll_off_is_capped_to_what_a_short_viewport_can_spare() {
    // A 4-row viewport spares one row either side, so a margin of 10 acts as 1.
    let buffer = lines(30);
    let area = Rect::new(0, 0, 20, 4);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    let mut paint = |state: &mut EditorState| {
        Editor::new(&buffer)
            .scroll_off(10)
            .render(area, &mut target, state);
    };
    paint(&mut state);
    state.move_down(&buffer);
    state.move_down(&buffer);
    paint(&mut state);
    assert_eq!(state.scroll_line, 0, "line 2 has a row below it");
    state.move_down(&buffer);
    paint(&mut state);
    assert_eq!(state.scroll_line, 1);
}
