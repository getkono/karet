//! Scrolling the viewport without moving the caret: centering, paging,
//! absolute line and column positions, and the extents a scrollbar reads.

use super::*;

#[test]
fn center_on_and_scroll_paging_move_viewport_only() {
    let mut state = EditorState::new();
    state.frame.height = 10;
    state.center_on(50);
    assert_eq!(
        state.scroll_line(),
        45,
        "line centered in a 10-row viewport"
    );
    // Scroll-only paging moves the viewport without touching the cursor.
    state.scroll_page_up();
    assert_eq!(state.scroll_line(), 35);
    state.scroll_page_down();
    assert_eq!(state.scroll_line(), 45);
    assert_eq!(state.cursor().line, 0, "paging never moves the cursor");
    // Centering near the top saturates at 0.
    state.center_on(2);
    assert_eq!(state.scroll_line(), 0);
}

/// A scroll that moves the viewport without moving the caret.
type ScrollOp = fn(&mut EditorState);

#[test]
fn centering_and_paging_outlast_a_pending_caret_reveal() {
    // A caret move owes a reveal on the next render; a scroll asked for
    // after it (vi `zz`, a pager's page key) drops that reveal instead of
    // being undone by it.
    let buffer = numbered(100);
    let area = Rect::new(0, 0, 20, 10);
    let render = |state: &mut EditorState| {
        Editor::new(&buffer).render(area, &mut Buffer::empty(area), state);
    };
    // Each target is relative to the top the caret move left, and none
    // shows the caret on line 80.
    // The scroll and the top it should leave, given the top before it.
    type Case = (&'static str, ScrollOp, fn(u32) -> u32);
    let ops: [Case; 3] = [
        ("center_on", |s| s.center_on(30), |_| 25),
        ("scroll_page_down", EditorState::scroll_page_down, |top| {
            top + 10
        }),
        ("scroll_page_up", EditorState::scroll_page_up, |top| {
            top - 10
        }),
    ];
    for (name, op, target) in ops {
        let mut state = EditorState::new();
        render(&mut state);
        state.goto(&buffer, LineCol::new(80, 0));
        let expected = target(state.scroll_line());
        op(&mut state);
        render(&mut state);
        assert_eq!(state.scroll_line(), expected, "{name}");
        assert_eq!(state.cursor(), LineCol::new(80, 0), "{name}");
    }
}

#[test]
fn centering_and_paging_start_at_the_first_row_of_the_line() {
    // Scrolled onto a continuation row of the wrapped line 0, each of these
    // lands on a buffer line and shows it from its first row.
    let text = format!("{}\n{}", "word ".repeat(40), "end\n".repeat(7));
    let buffer = TextBuffer::from_text(&text);
    let area = Rect::new(0, 0, 20, 4);
    let render = |state: &mut EditorState| {
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .word_wrap(true)
            .render(area, &mut target, state);
        target
    };
    let ops: [(&str, ScrollOp); 2] = [
        ("center_on", |s| s.center_on(0)),
        ("scroll_page_up", EditorState::scroll_page_up),
    ];
    for (name, op) in ops {
        let mut state = EditorState::new();
        render(&mut state);
        state.scroll_rows(&buffer, &[], true, 1);
        let target = render(&mut state);
        assert_ne!(
            target[(1, 0)].symbol(),
            "1",
            "{name}: continuation row on top"
        );
        op(&mut state);
        let target = render(&mut state);
        assert_eq!(
            target[(1, 0)].symbol(),
            "1",
            "{name}: line 1's first row on top"
        );
    }
}

#[test]
fn scrolling_to_a_line_lands_there_whether_or_not_the_view_wraps() {
    // A scrollbar hands over a position, not a delta. Stepping there with
    // `scroll_rows` would walk one visual anchor per row in wrap mode — the whole
    // document re-wrapped on every mouse event of a drag.
    let long = TextBuffer::from_text(&format!("{}\n", "word ".repeat(40)).repeat(200));
    let area = Rect::new(0, 0, 40, 10);

    for wrap in [false, true] {
        let mut state = EditorState::new();
        Editor::new(&long)
            .word_wrap(wrap)
            .render(area, &mut Buffer::empty(area), &mut state);
        state.scroll_to_line(120);
        assert_eq!(state.scroll_line(), 120, "word_wrap = {wrap}");
        // The caret stays put: this scrolls the view, it does not navigate.
        assert_eq!(state.cursor().line, 0);
    }
}

#[test]
fn scrolling_to_a_column_is_ignored_by_a_wrapped_view() {
    let buffer = TextBuffer::from_text(&format!("{}\n", "x".repeat(300)));
    let area = Rect::new(0, 0, 40, 4);

    let mut overflow = EditorState::new();
    Editor::new(&buffer).render(area, &mut Buffer::empty(area), &mut overflow);
    overflow.scroll_to_column(120);
    assert_eq!(overflow.scroll_col(), 120);

    // A soft-wrapped view has no horizontal axis at all, so it stays pinned at zero
    // rather than sliding text out from under a bar that was never reserved.
    let mut wrapped = EditorState::new();
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut Buffer::empty(area), &mut wrapped);
    wrapped.scroll_to_column(120);
    assert_eq!(wrapped.scroll_col(), 0);
}

#[test]
fn visible_lines_counts_buffer_lines_not_rows() {
    // Ten short lines fill ten rows one-for-one...
    let short = TextBuffer::from_text(&"short\n".repeat(10));
    let area = Rect::new(0, 0, 40, 10);
    let mut state = EditorState::new();
    Editor::new(&short).render(area, &mut Buffer::empty(area), &mut state);
    assert_eq!(state.visible_lines(), 10);

    // ...but a soft-wrapped line occupies several rows, so the same ten rows show
    // fewer buffer lines. Measuring both the total and the viewport in buffer lines
    // is what keeps the vertical extent O(1) instead of re-wrapping the document.
    let long = TextBuffer::from_text(&format!("{}\n", "word ".repeat(40)).repeat(10));
    let mut wrapped = EditorState::new();
    Editor::new(&long)
        .word_wrap(true)
        .render(area, &mut Buffer::empty(area), &mut wrapped);
    assert!(
        wrapped.visible_lines() < 10,
        "wrapped view showed {} buffer lines in 10 rows",
        wrapped.visible_lines()
    );
    assert!(wrapped.visible_lines() >= 1);
}

#[test]
fn longest_col_is_exact_after_a_horizontal_scroll_and_never_under_reports() {
    let buffer = TextBuffer::from_text(
        "short\n{}\nshort\n"
            .replace("{}", &"x".repeat(200))
            .as_str(),
    );
    let area = Rect::new(0, 0, 40, 3);
    let mut state = EditorState::new();
    Editor::new(&buffer).render(area, &mut Buffer::empty(area), &mut state);

    // The render alone sees the long line because it is on screen.
    assert!(state.longest_col() >= 200);

    // A horizontal scroll scans the whole buffer, so the answer is exact.
    state.scroll_columns(&buffer, 5);
    assert_eq!(state.longest_col(), 200);

    // Scrolled right, the extent still covers the visible right edge — a
    // scrollbar built on it can never claim the line is fully on screen.
    Editor::new(&buffer).render(area, &mut Buffer::empty(area), &mut state);
    assert!(state.longest_col() >= state.scroll_col() + 40);
}

/// `count` numbered lines.
fn numbered(count: u32) -> TextBuffer {
    let text: String = (0..count).map(|n| format!("line {n}\n")).collect();
    TextBuffer::from_text(&text)
}

#[test]
fn a_cursor_set_from_the_pointer_keeps_a_visible_head_where_it_is() {
    // Ten rows and a three-row margin: line 8 is on screen, inside the margin.
    let buffer = numbered(100);
    let area = Rect::new(0, 0, 20, 10);
    let render = |state: &mut EditorState| {
        Editor::new(&buffer)
            .scroll_off(3)
            .render(area, &mut Buffer::empty(area), state);
    };
    let at = |line| CursorState::single(Selection::caret(LineCol::new(line, 0)));

    let mut pointer = EditorState::new();
    render(&mut pointer);
    pointer.set_cursor_state_from_pointer(&buffer, at(8));
    render(&mut pointer);
    assert_eq!(pointer.scroll_line(), 0, "a visible head does not scroll");
    assert_eq!(pointer.cursor(), LineCol::new(8, 0));
    // Off screen it scrolls by the least that shows it: no margin.
    pointer.set_cursor_state_from_pointer(&buffer, at(50));
    render(&mut pointer);
    assert_eq!(pointer.scroll_line(), 41);

    // The keyboard's counterpart keeps three rows below the caret.
    let mut keyboard = EditorState::new();
    render(&mut keyboard);
    keyboard.set_cursor_state(&buffer, at(8));
    render(&mut keyboard);
    assert_eq!(keyboard.scroll_line(), 2);
}

#[test]
fn a_scroll_set_after_restoring_cursors_survives_the_render() {
    // Restoring a saved view: the carets come back first, then the scroll
    // the reader left, which need not show them. The render keeps that
    // scroll instead of revealing the restored caret.
    let buffer = numbered(100);
    let area = Rect::new(0, 0, 20, 10);
    let mut state = EditorState::new();
    Editor::new(&buffer).render(area, &mut Buffer::empty(area), &mut state);
    let caret = CursorState::single(Selection::caret(LineCol::new(80, 0)));
    state.set_cursor_state(&buffer, caret);
    state.scroll_to_line(10);
    Editor::new(&buffer).render(area, &mut Buffer::empty(area), &mut state);
    assert_eq!(state.scroll_line(), 10);
    assert_eq!(state.cursor(), LineCol::new(80, 0));
}

#[test]
fn a_mirrored_view_shows_the_other_views_place_and_keeps_it() {
    let text: String = (0..100)
        .map(|n| format!("{n:>3} {}\n", "x".repeat(200)))
        .collect();
    let buffer = TextBuffer::from_text(&text);
    let area = Rect::new(0, 0, 40, 10);
    let render = |state: &mut EditorState| {
        Editor::new(&buffer).render(area, &mut Buffer::empty(area), state);
    };
    let mut source = EditorState::new();
    render(&mut source);
    source.scroll_to_line(30);
    source.scroll_to_column(50);

    // The mirror still owes a reveal of its own caret at the top of the
    // document; mirroring drops it, so the render keeps the mirrored place.
    let mut mirror = EditorState::new();
    render(&mut mirror);
    mirror.goto(&buffer, LineCol::new(0, 0));
    mirror.mirror_scroll(&source);
    assert_eq!((mirror.scroll_line(), mirror.scroll_col()), (30, 50));
    render(&mut mirror);
    assert_eq!((mirror.scroll_line(), mirror.scroll_col()), (30, 50));
    assert_eq!(
        mirror.cursor(),
        LineCol::new(0, 0),
        "the caret did not move"
    );
}

#[test]
fn a_mirrored_view_starts_at_the_first_row_of_the_line() {
    // The source sits on line 0 and the mirror on a continuation row of
    // it; a continuation row names a row of one view's text, so the mirror
    // shows the source's line from its first row.
    let text = format!("{}\nend\n", "word ".repeat(40));
    let buffer = TextBuffer::from_text(&text);
    let area = Rect::new(0, 0, 20, 4);
    let render = |state: &mut EditorState| {
        let mut target = Buffer::empty(area);
        Editor::new(&buffer)
            .word_wrap(true)
            .render(area, &mut target, state);
        target
    };
    let mut source = EditorState::new();
    render(&mut source);

    let mut mirror = EditorState::new();
    render(&mut mirror);
    mirror.scroll_rows(&buffer, &[], true, 1);
    assert_eq!(mirror.scroll_line(), 0, "still on line 0, one row down");
    let target = render(&mut mirror);
    assert_ne!(target[(1, 0)].symbol(), "1", "a continuation row is on top");
    mirror.mirror_scroll(&source);
    let target = render(&mut mirror);
    let gutter = target[(1, 0)].symbol().to_owned();
    assert_eq!(gutter, "1", "line 1's number is on the top row");
}
