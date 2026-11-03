//! The display options: line-number modes, the cursor-line highlight,
//! rulers, the keyboard reveal's vertical margin (`scroll_off`), and the
//! soft-wrap geometry: the wrap column and continuation-row indentation.

use super::*;

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
    assert_eq!(state.scroll_col(), 4);
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
        state.scroll_to_line(4);
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .semantic_blocks(&blocks)
            .sticky_scroll(true)
            .line_numbers(numbers)
            .render(area, &mut target, &mut state);
        assert_eq!(
            state.frame.sticky_rows,
            vec![0],
            "line 0 is the sticky header"
        );
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
            state.extend_down(&buffer);
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line(), 0, "line 6 has three rows below it");
        state.extend_down(&buffer);
        if !wrap {
            // A wrapped view leaves the margin to the render, in rows.
            assert_eq!(state.scroll_line(), 1, "resolved by the motion");
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line(), 1, "and kept by the render");

        state.goto(&buffer, LineCol::new(20, 0));
        paint(&buffer, &mut state, wrap);
        assert_eq!(
            state.scroll_line(),
            14,
            "line 20 three rows from the bottom"
        );
        for _ in 0..4 {
            state.extend_up(&buffer);
        }
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line(), 13, "line 16 three rows from the top");
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
    assert_eq!(state.scroll_line(), 0, "row 6 has three rows below it");
    state.goto(&buffer, LineCol::new(4, 0));
    paint(&mut state);
    assert_eq!(
        (state.scroll_line(), state.scroll_subrow),
        (1, 0),
        "row 8 scrolls two rows, not three lines"
    );
    state.goto(&buffer, LineCol::new(2, 0));
    paint(&mut state);
    assert_eq!(
        (state.scroll_line(), state.scroll_subrow),
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
        state.extend_doc_end(&buffer);
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line(), 21, "the last line on the bottom row");
        state.extend_doc_start(&buffer);
        paint(&buffer, &mut state, wrap);
        assert_eq!(state.scroll_line(), 0);
    }
}

#[test]
fn a_click_inside_the_margin_does_not_scroll() {
    let buffer = lines(30);
    let mut state = EditorState::new();
    paint(&buffer, &mut state, false);
    state.set_caret(&buffer, LineCol::new(9, 0));
    paint(&buffer, &mut state, false);
    assert_eq!(state.scroll_line(), 0, "the bottom row stays put");
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
    state.extend_down(&buffer);
    state.extend_down(&buffer);
    paint(&mut state);
    assert_eq!(state.scroll_line(), 0, "line 2 has a row below it");
    state.extend_down(&buffer);
    paint(&mut state);
    assert_eq!(state.scroll_line(), 1);
}

/// The one-line buffer the wrapping-indent tests share: two leading spaces,
/// then three words that wrap a 10-cell row into three rows.
const INDENTED: &str = "  aaaa bbbb cccc";

/// One line takes a 3-cell gutter: the digit, its padding and the fold lane.
const ONE_LINE_GUTTER: u16 = 3;

/// The area the wrapping-indent tests render into: 10 content cells.
const INDENT_AREA: Rect = Rect::new(0, 0, ONE_LINE_GUTTER + 10, 4);

/// Render `buffer` soft-wrapped into [`INDENT_AREA`], focused, indenting
/// continuation rows as `indent` says, and return the painted target.
fn wrapped_indent(buffer: &TextBuffer, state: &mut EditorState, indent: WrappingIndent) -> Buffer {
    let mut target = Buffer::empty(INDENT_AREA);
    Editor::new(buffer)
        .word_wrap(true)
        .wrapping_indent(indent)
        .focused(true)
        .render(INDENT_AREA, &mut target, state);
    target
}

#[test]
fn continuation_rows_paint_after_the_lines_own_indent() {
    let buffer = TextBuffer::from_text(INDENTED);
    let target = wrapped_indent(&buffer, &mut EditorState::new(), WrappingIndent::Same);
    let rows: Vec<String> = (0..3)
        .map(|y| row_text(&target, ONE_LINE_GUTTER, y, 10))
        .collect();
    // The continuation rows give two cells to the indent, so `bbbb cccc`
    // no longer fits the second row's eight.
    assert_eq!(rows, ["  aaaa    ", "  bbbb    ", "  cccc    "]);
    let unindented = wrapped_indent(&buffer, &mut EditorState::new(), WrappingIndent::None);
    assert_eq!(row_text(&unindented, ONE_LINE_GUTTER, 1, 10), "bbbb cccc ");
}

#[test]
fn the_caret_on_a_continuation_row_is_drawn_past_its_indent() {
    let buffer = TextBuffer::from_text(INDENTED);
    let mut state = EditorState::new();
    // Column 7 starts the second row (`bbbb`); column 9 is two cells in.
    for (col, cell) in [(7, (ONE_LINE_GUTTER + 2, 1)), (9, (ONE_LINE_GUTTER + 4, 1))] {
        state.place_caret(LineCol::new(0, col));
        let target = wrapped_indent(&buffer, &mut state, WrappingIndent::Same);
        assert_eq!(
            state.primary_caret_cell(INDENT_AREA, &buffer, &[]),
            Some(cell)
        );
        assert!(
            target[cell].modifier.contains(Modifier::REVERSED),
            "the painted caret is the cell `primary_caret_cell` reports"
        );
    }
}

#[test]
fn a_click_on_a_continuation_row_resolves_past_its_indent() {
    let buffer = TextBuffer::from_text(INDENTED);
    let mut state = EditorState::new();
    wrapped_indent(&buffer, &mut state, WrappingIndent::Same);
    let click = |x: u16, y: u16| state.pos_at(INDENT_AREA, &buffer, &[], ONE_LINE_GUTTER + x, y);
    // On the `b` two cells past the indent: column 9.
    assert_eq!(click(4, 1), LineCol::new(0, 9));
    // Inside the indent: the row's first column, never the row above.
    assert_eq!(click(0, 1), LineCol::new(0, 7));
    assert_eq!(click(1, 1), LineCol::new(0, 7));
    // The third row starts at column 12, painted from cell 2.
    assert_eq!(click(3, 2), LineCol::new(0, 13));
    // The first row is not indented: cell 3 is column 3.
    assert_eq!(click(3, 0), LineCol::new(0, 3));
}

#[test]
fn vertical_motion_keeps_the_screen_column_across_an_indent() {
    let buffer = TextBuffer::from_text(INDENTED);
    let mut state = EditorState::new();
    wrapped_indent(&buffer, &mut state, WrappingIndent::Same);
    // Column 4 is cell 4 of the first row, which is cell 2 of the
    // continuation rows' text: columns 9 and 14.
    state.set_caret(&buffer, LineCol::new(0, 4));
    let mut path = Vec::new();
    for down in [true, true, false, false] {
        if down {
            state.extend_down(&buffer);
        } else {
            state.extend_up(&buffer);
        }
        path.push(state.cursor().col);
    }
    assert_eq!(path, [9, 14, 9, 4]);
    // A goal inside the indent lands on the continuation row's start.
    state.set_caret(&buffer, LineCol::new(0, 1));
    state.extend_down(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
}

/// The `(start, indent)` of each row `text` wraps into at `width` cells,
/// capped at `column`, indenting continuations as `indent` says.
fn rows_of(text: &str, width: u32, column: Option<u32>, indent: WrappingIndent) -> Vec<(u32, u32)> {
    let buffer = TextBuffer::from_text(text);
    let hints = crate::hint::HintIndex::default();
    let blocks = crate::hint::BlockIndex::default();
    let layout = crate::visual::Layout {
        width,
        tab_width: 4,
        unwrapped_lines: &[],
        hints: &hints,
        blocks: &blocks,
        wrap: true,
        wrap_column: column,
        wrapping_indent: indent,
    };
    crate::visual::visual_ranges(&buffer, 0, layout)
        .iter()
        .map(|range| (range.start, range.indent))
        .collect()
}

#[test]
fn each_wrapping_indent_mode_measures_the_leading_whitespace() {
    let text = "  word word word word word";
    assert_eq!(
        rows_of(text, 20, None, WrappingIndent::None),
        [(0, 0), (17, 0)]
    );
    assert_eq!(
        rows_of(text, 20, None, WrappingIndent::Same),
        [(0, 0), (17, 2)]
    );
    // One tab stop deeper: 2 + 4.
    assert_eq!(
        rows_of(text, 20, None, WrappingIndent::Indent),
        [(0, 0), (17, 6)]
    );
    // A leading tab expands to its stop, and the first row keeps none.
    assert_eq!(
        rows_of("\tword word word word", 12, None, WrappingIndent::Same),
        [(0, 0), (6, 4), (11, 4), (16, 4)]
    );
}

#[test]
fn a_deep_indent_is_capped_at_half_the_wrap_width() {
    // Twelve leading spaces in a 10-cell row would leave a continuation no
    // room at all; half the row is kept for text.
    let rows = rows_of("            abcdefghijkl", 10, None, WrappingIndent::Same);
    assert!(rows.len() > 1, "{rows:?}");
    assert!(
        rows.iter().skip(1).all(|&(_, indent)| indent == 5),
        "{rows:?}"
    );
    // A one-cell view has no half to give.
    let rows = rows_of("  abc", 1, None, WrappingIndent::Indent);
    assert!(rows.iter().all(|&(_, indent)| indent == 0), "{rows:?}");
}

#[test]
fn the_wrap_column_narrows_but_never_widens_the_row() {
    let text = "aaaa bbbb cccc dddd eeee";
    // Narrower than the viewport: rows break at the column.
    assert_eq!(
        rows_of(text, 20, Some(10), WrappingIndent::None),
        [(0, 0), (10, 0), (20, 0)]
    );
    // Wider than the viewport, or zero: the viewport decides.
    let viewport = rows_of(text, 20, None, WrappingIndent::None);
    assert_eq!(viewport, [(0, 0), (20, 0)]);
    assert_eq!(rows_of(text, 20, Some(80), WrappingIndent::None), viewport);
    assert_eq!(rows_of(text, 20, Some(0), WrappingIndent::None), viewport);
}

#[test]
fn a_wrap_column_paints_and_hit_tests_at_its_column() {
    let buffer = TextBuffer::from_text("aaaa bbbb cccc dddd");
    let area = Rect::new(0, 0, ONE_LINE_GUTTER + 20, 3);
    let mut state = EditorState::new();
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .wrap_column(Some(10))
        .render(area, &mut target, &mut state);
    let rows: Vec<String> = (0..2)
        .map(|y| row_text(&target, ONE_LINE_GUTTER, y, 20))
        .collect();
    assert_eq!(rows, ["aaaa bbbb           ", "cccc dddd           "]);
    // Past the column on the first row: that row's end, not the next row.
    let pos = state.pos_at(area, &buffer, &[], ONE_LINE_GUTTER + 15, 0);
    assert_eq!(pos, LineCol::new(0, 10));
    let pos = state.pos_at(area, &buffer, &[], ONE_LINE_GUTTER + 2, 1);
    assert_eq!(pos, LineCol::new(0, 12));
}
