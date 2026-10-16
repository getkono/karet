//! Virtual lines: whole rows above or below a buffer line that the row walk
//! counts everywhere — painting, scrolling, reveal, hit-testing, motion and
//! folds — plus what that one walk, now shared by wrapped and unwrapped
//! views, does to folds in either.

use super::*;

/// The gutter of a document with fewer than ten lines: marker, digit, fold.
const GUTTER: u16 = 3;

/// The row `y` as painted.
fn row(target: &Buffer, y: u16) -> String {
    (target.area.x..target.area.right())
        .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// Every row of `target` as painted, trailing blanks trimmed.
fn rows(target: &Buffer) -> Vec<String> {
    (target.area.y..target.area.bottom())
        .map(|y| row(target, y).trim_end().to_owned())
        .collect()
}

fn above(line: u32, text: &str) -> VirtualLine {
    VirtualLine::above(line, text, ThemeRole::Muted)
}

fn below(line: u32, text: &str) -> VirtualLine {
    VirtualLine::below(line, text, ThemeRole::Muted)
}

/// One frame of `buffer` into `area` with `lines`, soft wrapped when `wrap`.
fn frame(
    buffer: &TextBuffer,
    lines: &[VirtualLine],
    folds: &[Fold],
    wrap: bool,
    area: Rect,
    state: &mut EditorState,
) -> Buffer {
    let mut target = Buffer::empty(area);
    Editor::new(buffer)
        .virtual_lines(lines)
        .folds(folds)
        .word_wrap(wrap)
        .render(area, &mut target, state);
    target
}

#[test]
fn virtual_lines_take_rows_of_their_own_around_their_line() {
    let buffer = TextBuffer::from_text("fn a\nfn b\n");
    let lines = [
        below(0, "B1"),
        above(0, "A1"),
        above(0, "A2"),
        below(0, "B2"),
        above(1, "lens"),
    ];
    let area = Rect::new(0, 0, 12, 8);
    let mut state = EditorState::new();
    let target = frame(&buffer, &lines, &[], false, area, &mut state);

    // Above rows top first, then the line, then the rows below, each in the
    // order supplied; the gutter beside a virtual line is blank.
    assert_eq!(
        rows(&target),
        [
            "   A1", "   A2", " 1 fn a", "   B1", "   B2", "   lens", " 2 fn b", " 3"
        ]
    );
    let lens = target[(GUTTER, 5)].style().fg;
    assert_eq!(
        lens,
        Some(Theme::dark().role(ThemeRole::Muted).to_ratatui()),
        "a virtual line paints in its role's color"
    );
}

#[test]
fn a_click_on_a_virtual_line_is_routed_to_it_and_its_owner() {
    let buffer = TextBuffer::from_text("alpha\nbeta\n");
    let lines = [above(1, "2 references"), below(0, "^ here")];
    let area = Rect::new(0, 0, 20, 5);
    let mut state = EditorState::new();
    frame(&buffer, &lines, &[], false, area, &mut state);

    // Rows: alpha, `^ here`, `2 references`, beta, (line 2).
    assert_eq!(state.virtual_line_at(GUTTER + 1, 1), Some(1));
    assert_eq!(state.virtual_line_at(GUTTER + 15, 2), Some(0));
    assert_eq!(state.virtual_line_at(0, 2), None, "the gutter beside it");
    assert_eq!(state.virtual_line_at(GUTTER, 0), None, "a text row");
    assert_eq!(
        state.virtual_line_rect(0),
        Some(Rect::new(GUTTER, 2, 17, 1))
    );

    // The caret goes to the owner: the end of the line a row below belongs
    // to, the start of the one a row above leads into.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 2, 1),
        LineCol::new(0, 5)
    );
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 2, 2),
        LineCol::new(1, 0)
    );
    // The text rows past them resolve to their own line and column.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 2, 3),
        LineCol::new(1, 2)
    );
    let gutter = state.gutter_hit_at(area, &buffer, &[], 0, 2);
    assert_eq!(gutter.map(|hit| hit.line), Some(1));
}

#[test]
fn the_caret_is_drawn_below_the_rows_above_its_line() {
    let buffer = TextBuffer::from_text("one\ntwo\n");
    let lines = [above(0, "lens"), below(0, "note")];
    let area = Rect::new(0, 0, 20, 4);
    let mut state = EditorState::new();
    frame(&buffer, &lines, &[], false, area, &mut state);
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(0, 2)),
        Some((GUTTER + 2, 1))
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(1, 1)),
        Some((GUTTER + 1, 3))
    );
}

#[test]
fn a_collapsed_fold_hides_the_virtual_lines_of_the_lines_it_hides() {
    let buffer = TextBuffer::from_text("head\ninner\nlast\ntail\n");
    let lines = [
        below(1, "hidden"),
        above(2, "hidden too"),
        above(3, "shown"),
    ];
    let folds = [Fold {
        start: 0,
        end: 2,
        collapsed: true,
    }];
    let area = Rect::new(0, 0, 20, 4);
    let mut state = EditorState::new();
    let target = frame(&buffer, &lines, &folds, false, area, &mut state);
    assert_eq!(rows(&target), [" 1▸head ⋯", "   shown", " 4 tail", " 5"]);
    assert_eq!(state.virtual_line_rect(0), None);
    assert_eq!(state.virtual_line_rect(1), None);
    assert_eq!(
        state.pos_at(area, &buffer, &folds, GUTTER, 2),
        LineCol::new(3, 0)
    );
}

#[test]
fn scrolling_walks_virtual_rows_one_at_a_time() {
    let buffer = TextBuffer::from_text("a\nb\n");
    let lines = [above(1, "x"), above(1, "y")];
    for wrap in [false, true] {
        let area = Rect::new(0, 0, 10, 2);
        let mut state = EditorState::new();
        frame(&buffer, &lines, &[], wrap, area, &mut state);
        state.scroll_rows(&buffer, &[], wrap, 2);
        let target = frame(&buffer, &lines, &[], wrap, area, &mut state);
        assert_eq!(rows(&target), ["   y", " 2 b"], "wrap: {wrap}");
        state.scroll_rows(&buffer, &[], wrap, -1);
        let target = frame(&buffer, &lines, &[], wrap, area, &mut state);
        assert_eq!(rows(&target), ["   x", "   y"], "wrap: {wrap}");
    }
}

#[test]
fn revealing_a_line_counts_virtual_rows_and_shows_the_rows_above_it() {
    let buffer = TextBuffer::from_text("l0\nl1\nl2\nl3\n");
    let lines = [above(0, "lens 0"), below(1, "n1"), below(1, "n2")];
    for wrap in [false, true] {
        let area = Rect::new(0, 0, 12, 3);
        let mut state = EditorState::new();
        frame(&buffer, &lines, &[], wrap, area, &mut state);

        // Line 2 is the fifth row: the view scrolls until it is the last one.
        state.goto(&buffer, LineCol::new(2, 0));
        let target = frame(&buffer, &lines, &[], wrap, area, &mut state);
        assert_eq!(rows(&target), ["   n1", "   n2", " 3 l2"], "wrap: {wrap}");

        // Back to the top: the lens above line 0 comes back into view too.
        state.goto(&buffer, LineCol::new(0, 0));
        let target = frame(&buffer, &lines, &[], wrap, area, &mut state);
        assert_eq!(
            rows(&target),
            ["   lens 0", " 1 l0", " 2 l1"],
            "wrap: {wrap}"
        );
    }
}

#[test]
fn a_block_taller_than_the_viewport_never_pushes_the_caret_off_screen() {
    let buffer = TextBuffer::from_text("l0\nl1\nl2\n");
    let lines: Vec<VirtualLine> = (0..5).map(|n| above(2, &format!("v{n}"))).collect();
    let area = Rect::new(0, 0, 10, 3);
    let mut state = EditorState::new();
    frame(&buffer, &lines, &[], false, area, &mut state);
    state.goto(&buffer, LineCol::new(2, 0));
    let target = frame(&buffer, &lines, &[], false, area, &mut state);
    assert_eq!(rows(&target), ["   v3", "   v4", " 3 l2"]);
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((GUTTER, 2))
    );
}

#[test]
fn vertical_motion_steps_over_virtual_lines() {
    let buffer = TextBuffer::from_text("abc\ndef\n");
    let lines = [below(0, "between"), above(1, "lens")];
    for wrap in [false, true] {
        let area = Rect::new(0, 0, 20, 5);
        let mut state = EditorState::new();
        frame(&buffer, &lines, &[], wrap, area, &mut state);
        state.place_caret(LineCol::new(0, 2));
        state.move_down(&buffer);
        assert_eq!(state.cursor(), LineCol::new(1, 2), "wrap: {wrap}");
        state.move_up(&buffer);
        assert_eq!(state.cursor(), LineCol::new(0, 2), "wrap: {wrap}");
    }
}

#[test]
fn rows_below_a_wrapped_line_follow_its_last_row() {
    let buffer = TextBuffer::from_text("alpha beta gamma\nz\n");
    let lines = [below(0, "note"), above(0, "lens")];
    let area = Rect::new(0, 0, 10, 6); // 7 content cells.
    let mut state = EditorState::new();
    let target = frame(&buffer, &lines, &[], true, area, &mut state);
    assert_eq!(
        rows(&target),
        [
            "   lens", " 1 alpha", "   beta", "   gamma", "   note", " 2 z"
        ]
    );
    // The continuation row still maps clicks into its own columns.
    assert_eq!(
        state.pos_at(area, &buffer, &[], GUTTER + 1, 2),
        LineCol::new(0, 7)
    );
    state.place_caret(LineCol::new(0, 12));
    assert_eq!(
        state.primary_caret_cell(area, &buffer, &[]),
        Some((GUTTER + 1, 3))
    );
}

#[test]
fn a_virtual_line_scrolls_horizontally_and_is_cut_at_the_edge() {
    let buffer = TextBuffer::from_text("0123456789abcdef\n");
    let lines = [below(0, "ABCDEFGHIJKLMNOP")];
    let area = Rect::new(0, 0, 10, 2); // 7 content cells.
    let mut state = EditorState::new();
    let target = frame(&buffer, &lines, &[], false, area, &mut state);
    assert_eq!(row(&target, 1), "   ABCDEF…");
    state.scroll_to_column(4);
    let target = frame(&buffer, &lines, &[], false, area, &mut state);
    assert_eq!(row(&target, 0), " 1 456789a");
    assert_eq!(row(&target, 1), "   EFGHIJ…");
}

#[test]
fn a_header_with_rows_above_it_sticks_only_once_its_own_row_scrolls_off() {
    let buffer = TextBuffer::from_text("fn f() {\n  a\n  b\n  c\n}\n");
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 4,
    }]);
    let lines = [above(0, "lens"), below(0, "note")];
    let area = Rect::new(0, 0, 14, 3);
    let render = |state: &mut EditorState| {
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .virtual_lines(&lines)
            .semantic_blocks(&blocks)
            .sticky_scroll(true)
            .render(area, &mut target, state);
        target
    };
    let mut state = EditorState::new();
    render(&mut state);
    // Top on the lens: the header's own row is in view, so nothing sticks.
    let target = render(&mut state);
    assert_eq!(row(&target, 0).trim_end(), "   lens");
    // Top on the note below the header: the header is pinned above it.
    state.scroll_rows(&buffer, &[], false, 2);
    let target = render(&mut state);
    assert_eq!(row(&target, 0).trim_end(), " 1 fn f() {");
    assert_eq!(row(&target, 1).trim_end(), "   note");
}

#[test]
fn overflow_row_scrolling_steps_over_a_collapsed_fold_as_one_row() {
    let buffer = TextBuffer::from_text("head\na\nb\nc\ntail\n");
    let folds = [Fold {
        start: 0,
        end: 3,
        collapsed: true,
    }];
    let mut state = EditorState::new();
    state.scroll_rows(&buffer, &folds, false, 1);
    assert_eq!(state.scroll_line, 4, "the fold's hidden lines take no rows");
    state.scroll_rows(&buffer, &folds, false, -1);
    assert_eq!(state.scroll_line, 0);
    state.scroll_rows(&buffer, &folds, false, i32::MAX);
    assert_eq!(state.scroll_line, 5, "clamped to the last line");
}

#[test]
fn an_unwrapped_reveal_keeps_scroll_off_in_rows_not_lines() {
    let text: String = (0..20).map(|n| format!("l{n}\n")).collect();
    let buffer = TextBuffer::from_text(&text);
    let lines = [above(11, "a"), above(11, "b")];
    let area = Rect::new(0, 0, 20, 8);
    let mut state = EditorState::new();
    let paint = |state: &mut EditorState| {
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .virtual_lines(&lines)
            .scroll_off(3)
            .render(area, &mut target, state);
        target
    };
    state.scroll_line = 10;
    state.place_caret(LineCol::new(12, 0));
    paint(&mut state);

    // Line 11's text row is the fourth row: already three rows from the top.
    state.move_up(&buffer);
    state.reveal(&buffer, state.cursor());
    assert_eq!(state.scroll_line, 10, "the estimate must not scroll");
    let target = paint(&mut state);
    assert_eq!(state.scroll_line, 10, "nor the render");
    assert_eq!(row(&target, 3).trim_end(), " 12 l11");

    // One more line up lands on the top row; the render restores the margin.
    state.move_up(&buffer);
    state.reveal(&buffer, state.cursor());
    paint(&mut state);
    assert_eq!(state.scroll_line, 7);
}

#[test]
fn a_caret_hidden_in_a_fold_is_revealed_through_its_header_wrapped_or_not() {
    let buffer = TextBuffer::from_text("l0\nl1\nhead\nin\ntail\nl5\n");
    let folds = [Fold {
        start: 2,
        end: 3,
        collapsed: true,
    }];
    for wrap in [false, true] {
        let area = Rect::new(0, 0, 10, 2);
        let mut state = EditorState::new();
        frame(&buffer, &[], &folds, wrap, area, &mut state);
        state.scroll_to_line(5);
        state.goto(&buffer, LineCol::new(3, 1));
        let target = frame(&buffer, &[], &folds, wrap, area, &mut state);
        assert_eq!(row(&target, 0).trim_end(), " 3▸head ⋯", "wrap: {wrap}");
    }
}
