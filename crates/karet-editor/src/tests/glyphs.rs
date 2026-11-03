//! Structure drawn over the text: indentation guides and the caret's block,
//! visible whitespace in each `RenderWhitespace` mode, and bracket-pair
//! colourisation.

use karet_core::BytePos;
use karet_core::Span;
use karet_core::StandardToken;

use super::*;

/// The gutter `LineNumbers::Off` keeps: content starts at this column.
const X: u16 = 2;

/// The content painted on row `y`, `width` cells from the gutter.
fn row(target: &Buffer, y: u16, width: u16) -> String {
    (X..X + width)
        .map(|x| target[(x, y)].symbol().to_owned())
        .collect()
}

fn fg(target: &Buffer, x: u16, y: u16) -> ratatui::style::Color {
    target[(X + x, y)].fg
}

fn color(theme: &Theme, role: ThemeRole) -> ratatui::style::Color {
    theme.role(role).to_ratatui()
}

/// Render `editor` over `area` with the caret at `caret`.
fn paint(editor: Editor<'_>, area: Rect, caret: LineCol) -> Buffer {
    let mut state = EditorState::new();
    state.place_caret(caret);
    let mut target = Buffer::empty(area);
    editor
        .line_numbers(LineNumbers::Off)
        .render(area, &mut target, &mut state);
    target
}

const NESTED: &str = "{\n    {\n        a\n\n        b\n    }\n}\n";

#[test]
fn guides_are_off_by_default_and_mark_each_level_of_leading_whitespace() {
    let buffer = TextBuffer::from_text(NESTED);
    let area = Rect::new(0, 0, 14, 7);
    let plain = paint(Editor::new(&buffer), area, LineCol::new(0, 0));
    assert_eq!(row(&plain, 2, 9), "        a", "off by default");

    let guided = paint(
        Editor::new(&buffer).indent_guides(true),
        area,
        LineCol::new(0, 0),
    );
    let rows: Vec<String> = (0..7).map(|y| row(&guided, y, 9)).collect();
    assert_eq!(
        rows,
        [
            "{        ",
            "│   {    ",
            "│   │   a",
            // A blank line takes the deeper indentation around it, past its
            // own end.
            "│   │    ",
            "│   │   b",
            "│   }    ",
            "}        ",
        ]
    );
}

#[test]
fn the_carets_block_draws_its_guide_in_the_active_colour() {
    let theme = Theme::dark();
    let buffer = TextBuffer::from_text(NESTED);
    let area = Rect::new(0, 0, 14, 7);
    let guide = color(&theme, ThemeRole::IndentGuide);
    let active = color(&theme, ThemeRole::LineNumberActive);
    assert_ne!(guide, active);
    let editor = || Editor::new(&buffer).theme(&theme).indent_guides(true);

    // Inside the inner block: its guide (column 4) on every line of it,
    // through the blank line; the outer guide stays plain.
    let inner = paint(editor(), area, LineCol::new(2, 8));
    for y in 2..=4 {
        assert_eq!(fg(&inner, 4, y), active, "row {y}");
        assert_eq!(fg(&inner, 0, y), guide, "row {y}");
    }
    assert_eq!(fg(&inner, 0, 1), guide, "the inner header is outside it");

    // On a header: the block it opens.
    let header = paint(editor(), area, LineCol::new(0, 0));
    for y in 1..=5 {
        assert_eq!(fg(&header, 0, y), active, "row {y}");
    }
    assert_eq!(fg(&header, 4, 2), guide);

    // Switched off, or read-only with no caret: every guide is plain.
    let off = paint(
        editor().active_indent_guide(false),
        area,
        LineCol::new(2, 8),
    );
    assert_eq!(fg(&off, 4, 2), guide);
    let read_only = paint(editor().read_only(true), area, LineCol::new(2, 8));
    assert_eq!(fg(&read_only, 4, 2), guide);
}

#[test]
fn a_blank_lines_guides_stop_where_its_end_of_line_text_starts() {
    let buffer = TextBuffer::from_text("{\n        a\n\n        b\n");
    let blame = [VirtualText {
        line: 2,
        placement: VirtualTextPlacement::EndOfLine {
            align: karet_core::EolAlign::Left,
            priority: 0,
        },
        text: "  x y z w".to_owned(),
        role: ThemeRole::Muted,
    }];
    let area = Rect::new(0, 0, 14, 4);
    let target = paint(
        Editor::new(&buffer)
            .indent_guides(true)
            .virtual_text(&blame),
        area,
        LineCol::new(0, 0),
    );
    assert_eq!(row(&target, 2, 9), "  x y z w", "no guide cuts into it");
    assert_eq!(row(&target, 1, 9), "│   │   a");
}

#[test]
fn guides_step_by_the_tab_width_and_cross_hard_tabs() {
    let buffer = TextBuffer::from_text("{\n\t\tx\n  y\n");
    let area = Rect::new(0, 0, 10, 3);
    let target = paint(
        Editor::new(&buffer).tab_width(2).indent_guides(true),
        area,
        LineCol::new(0, 0),
    );
    assert_eq!(row(&target, 1, 5), "│ │ x");
    assert_eq!(row(&target, 2, 3), "│ y");
}

#[test]
fn guides_follow_horizontal_scroll_on_text_and_blank_lines() {
    let buffer = TextBuffer::from_text("{\n        a\n\n        b\n");
    let area = Rect::new(0, 0, 10, 4);
    let mut state = EditorState::new();
    state.scroll_to_column(2);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .indent_guides(true)
        .line_numbers(LineNumbers::Off)
        .render(area, &mut target, &mut state);
    // Columns 2.. are on screen: the guide at column 4 sits two cells in.
    assert_eq!(row(&target, 1, 7), "  │   a");
    assert_eq!(row(&target, 2, 7), "  │    ");
}

#[test]
fn whitespace_modes_mark_the_spaces_and_tabs_they_select() {
    let theme = Theme::dark();
    let buffer = TextBuffer::from_text("a b  c\t \n  d \n");
    let area = Rect::new(0, 0, 14, 2);
    let render = |mode| {
        paint(
            Editor::new(&buffer).theme(&theme).render_whitespace(mode),
            area,
            LineCol::new(0, 0),
        )
    };

    let none = render(RenderWhitespace::None);
    assert_eq!(row(&none, 0, 9), "a b  c   ");

    let all = render(RenderWhitespace::All);
    assert_eq!(row(&all, 0, 9), "a·b··c→ ·");
    assert_eq!(row(&all, 1, 4), "··d·");
    assert_eq!(fg(&all, 1, 0), color(&theme, ThemeRole::IndentGuide));

    // A lone space between words stays blank; runs, tabs and line edges show.
    let boundary = render(RenderWhitespace::Boundary);
    assert_eq!(row(&boundary, 0, 9), "a b··c→ ·");
    assert_eq!(row(&boundary, 1, 4), "··d·");

    let trailing = render(RenderWhitespace::Trailing);
    assert_eq!(row(&trailing, 0, 9), "a b  c→ ·");
    assert_eq!(row(&trailing, 1, 4), "  d·");
}

#[test]
fn selection_mode_marks_only_selected_whitespace() {
    let buffer = TextBuffer::from_text("a b c d\n");
    let area = Rect::new(0, 0, 10, 1);
    let mut state = EditorState::new();
    state.set_selection(&buffer, LineCol::new(0, 2), LineCol::new(0, 6));
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .render_whitespace(RenderWhitespace::Selection)
        .line_numbers(LineNumbers::Off)
        .render(area, &mut target, &mut state);
    assert_eq!(row(&target, 0, 7), "a b·c·d");
}

#[test]
fn a_guide_wins_the_cell_a_whitespace_mark_would_take() {
    let buffer = TextBuffer::from_text("{\n     x\n");
    let area = Rect::new(0, 0, 10, 2);
    let target = paint(
        Editor::new(&buffer)
            .indent_guides(true)
            .render_whitespace(RenderWhitespace::All),
        area,
        LineCol::new(0, 0),
    );
    assert_eq!(row(&target, 1, 6), "│···│x");
}

/// Highlights marking each `(start, end)` byte range as a string.
fn strings(ranges: &[(usize, usize)]) -> Highlights {
    Highlights::from_sorted_spans(
        ranges
            .iter()
            .map(|&(start, end)| HighlightSpan {
                span: Span {
                    start: BytePos(start),
                    end: BytePos(end),
                },
                token: StandardToken::String.id(),
            })
            .collect(),
    )
}

#[test]
fn rainbow_brackets_colour_by_depth_and_skip_text_and_strays() {
    let theme = Theme::dark();
    let [gold, purple, blue] = theme.bracket_pair_colors().map(Rgba::to_ratatui);
    let text = "({[(\"(\")]}))\n";
    let buffer = TextBuffer::from_text(text);
    let highlights = strings(&[(4, 7)]);
    let area = Rect::new(0, 0, 16, 1);
    let editor = || Editor::new(&buffer).theme(&theme).highlights(&highlights);

    let plain = paint(editor(), area, LineCol::new(0, 0));
    assert_ne!(fg(&plain, 0, 0), gold, "off by default");

    let rainbow = paint(editor().rainbow_brackets(true), area, LineCol::new(0, 0));
    let expected = [
        (0, gold),
        (1, purple),
        (2, blue),
        // Depth 3 wraps back to the first colour.
        (3, gold),
        (7, gold),
        (8, blue),
        (9, purple),
        (10, gold),
    ];
    for (x, want) in expected {
        assert_eq!(fg(&rainbow, x, 0), want, "column {x}");
    }
    let string = theme.color(StandardToken::String.id()).to_ratatui();
    assert_eq!(fg(&rainbow, 5, 0), string, "a bracket in a string is text");
    let default = color(&theme, ThemeRole::Foreground);
    assert_eq!(
        fg(&rainbow, 11, 0),
        default,
        "a stray closer keeps its colour"
    );
}

#[test]
fn rainbow_depth_carries_across_lines_and_pops_to_its_own_kind() {
    let theme = Theme::dark();
    let [gold, purple, _] = theme.bracket_pair_colors().map(Rgba::to_ratatui);
    // The `[` on line 1 is never closed: the `)` pops past it to its `(`.
    let buffer = TextBuffer::from_text("{\n  ([\n  )\n}\n");
    let area = Rect::new(0, 0, 8, 4);
    let target = paint(
        Editor::new(&buffer).theme(&theme).rainbow_brackets(true),
        area,
        LineCol::new(0, 0),
    );
    assert_eq!(fg(&target, 0, 0), gold);
    assert_eq!(fg(&target, 2, 1), purple);
    assert_eq!(fg(&target, 2, 2), purple, "pairs with the `(`");
    assert_eq!(fg(&target, 0, 3), gold, "the stray `[` was dropped");
}

#[test]
fn bracket_depths_restart_when_asked_for_an_earlier_line() {
    let buffer = TextBuffer::from_text("(\n(\n))\n");
    let mut depths = bracket::BracketDepths::new(&buffer, None);
    assert_eq!(depths.line(2, u32::MAX), [Some(1), Some(0)]);
    assert_eq!(
        depths.line(2, u32::MAX),
        [Some(1), Some(0)],
        "a repeated line"
    );
    assert_eq!(
        depths.line(1, u32::MAX),
        [Some(1)],
        "rescanned from the top"
    );
    assert_eq!(depths.line(0, u32::MAX), [Some(0)]);
}

#[test]
fn bracket_depths_read_a_painted_line_only_as_far_as_asked() {
    let buffer = TextBuffer::from_text("(()\n)\n");
    let mut depths = bracket::BracketDepths::new(&buffer, None);
    assert_eq!(depths.line(0, 2), [Some(0), Some(1)], "the row's prefix");
    assert_eq!(
        depths.line(0, 3),
        [Some(0), Some(1), Some(1)],
        "a later row"
    );
    // Moving on reads the whole of line 0: its `)` closed the inner `(`.
    assert_eq!(depths.line(1, u32::MAX), [Some(0)]);
}

#[test]
fn bracket_depths_restart_after_a_jump_past_the_scan_bound() {
    // Line 0 opens a bracket; the closer sits more than the scan bound below.
    let far = bracket::SCAN_LINES + 5;
    let text = format!("(\n{})\n", "\n".repeat(far - 1));
    let buffer = TextBuffer::from_text(&text);
    let mut depths = bracket::BracketDepths::new(&buffer, None);
    assert_eq!(depths.line(0, u32::MAX), [Some(0)]);
    let line = u32::try_from(far).unwrap_or(u32::MAX);
    assert_eq!(
        depths.line(line, u32::MAX),
        [None],
        "the opener lies beyond the restarted scan's lookback"
    );
}

#[test]
fn bracket_depths_keep_their_stack_for_a_step_of_exactly_the_scan_bound() {
    // Line 0 opens a bracket; its closer sits on line `SCAN_LINES + 1`.
    let bound = bracket::SCAN_LINES;
    let text = format!("(\n{})\n", "\n".repeat(bound));
    let buffer = TextBuffer::from_text(&text);
    let mut depths = bracket::BracketDepths::new(&buffer, None);
    // Asking for line 1 consumes line 0, so the stack holds its `(`.
    assert_eq!(depths.line(1, u32::MAX), [] as [Option<usize>; 0]);
    let last = u32::try_from(bound + 1).unwrap_or(u32::MAX);
    assert_eq!(
        depths.line(last, u32::MAX),
        [Some(0)],
        "a step of exactly the bound walks on instead of restarting"
    );
    assert_eq!(
        depths.line(last, u32::MAX),
        [Some(0)],
        "a repeated line reuses the stack instead of rescanning a shorter lookback"
    );
}

#[test]
fn trailing_whitespace_longer_than_the_tail_window_is_all_marked() {
    let text = format!("ab c{}\n", " ".repeat(70));
    let buffer = TextBuffer::from_text(&text);
    let area = Rect::new(0, 0, 80, 1);
    let target = paint(
        Editor::new(&buffer).render_whitespace(RenderWhitespace::Trailing),
        area,
        LineCol::new(0, 0),
    );
    assert_eq!(row(&target, 0, 74), format!("ab c{}", "\u{b7}".repeat(70)));
}
