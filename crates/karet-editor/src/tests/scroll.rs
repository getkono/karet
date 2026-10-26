//! Scrolling the viewport without moving the caret: centering, paging,
//! absolute line and column positions, and the extents a scrollbar reads.

use super::*;

#[test]
fn center_on_and_scroll_paging_move_viewport_only() {
    let mut state = EditorState::new();
    state.frame.height = 10;
    state.center_on(50);
    assert_eq!(state.scroll_line, 45, "line centered in a 10-row viewport");
    // Scroll-only paging moves the viewport without touching the cursor.
    state.scroll_page_up();
    assert_eq!(state.scroll_line, 35);
    state.scroll_page_down();
    assert_eq!(state.scroll_line, 45);
    assert_eq!(state.cursor().line, 0, "paging never moves the cursor");
    // Centering near the top saturates at 0.
    state.center_on(2);
    assert_eq!(state.scroll_line, 0);
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
        assert_eq!(state.scroll_line, 120, "word_wrap = {wrap}");
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
    assert_eq!(overflow.scroll_col, 120);

    // A soft-wrapped view has no horizontal axis at all, so it stays pinned at zero
    // rather than sliding text out from under a bar that was never reserved.
    let mut wrapped = EditorState::new();
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut Buffer::empty(area), &mut wrapped);
    wrapped.scroll_to_column(120);
    assert_eq!(wrapped.scroll_col, 0);
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
    assert!(state.longest_col() >= state.scroll_col + 40);
}
