//! Very long lines — minified files: what a row paints is unchanged, while the
//! work behind it is bounded by what the viewport shows, not by the line.

use karet_core::BytePos;
use karet_core::Change;
use karet_core::TextEdit;
use karet_core::TokenId;

use crate::text::*;
use crate::visual::horizontal_extent;
use crate::*;

/// A highlight of bytes `[start, end)` as token `token`.
fn hs(start: usize, end: usize, token: u16) -> HighlightSpan {
    HighlightSpan {
        span: karet_core::Span {
            start: BytePos(start),
            end: BytePos(end),
        },
        token: TokenId(token),
    }
}

/// `len` chars cycling through the alphabet, so every window is checkable.
fn alphabet(len: usize) -> String {
    (b'a'..=b'z').cycle().take(len).map(char::from).collect()
}

/// Row `y` of `target` as a string, one char per cell.
fn row(target: &Buffer, y: u16) -> String {
    (0..target.area.width)
        .map(|x| target[(x, y)].symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[test]
fn token_lookup_bisects_to_the_covering_span() {
    let hl = [hs(0, 2, 1), hs(2, 2, 9), hs(4, 8, 2), hs(10, 11, 3)];
    assert_eq!(token_at(0, &hl), Some(TokenId(1)));
    assert_eq!(token_at(1, &hl), Some(TokenId(1)));
    // A gap, and the empty span sitting in it, claim nothing.
    assert_eq!(token_at(2, &hl), None);
    assert_eq!(token_at(3, &hl), None);
    assert_eq!(token_at(4, &hl), Some(TokenId(2)));
    assert_eq!(token_at(7, &hl), Some(TokenId(2)));
    assert_eq!(token_at(8, &hl), None);
    assert_eq!(token_at(10, &hl), Some(TokenId(3)));
    assert_eq!(token_at(11, &hl), None);
    assert_eq!(token_at(0, &[]), None);
}

#[test]
fn a_line_prefix_copies_only_the_columns_asked_for() {
    let buffer = TextBuffer::from_text("héllo wörld\r\nab\n");
    assert_eq!(line_chars_prefix(&buffer, 0, 3), vec!['h', 'é', 'l']);
    // A cap past the end yields the whole line, without its line break.
    let whole: String = line_chars_prefix(&buffer, 0, 100).into_iter().collect();
    assert_eq!(whole, "héllo wörld");
    assert!(line_chars_prefix(&buffer, 1, 0).is_empty());
    assert!(line_chars_prefix(&buffer, 9, 5).is_empty(), "past the end");
}

#[test]
fn line_length_counts_chars_without_the_line_break() {
    let buffer = TextBuffer::from_text("héllo wörld\r\nab\n");
    assert_eq!(line_len(&buffer, 0), 11);
    assert_eq!(line_len(&buffer, 1), 2);
    assert_eq!(line_len(&buffer, 2), 0, "the empty last line");
    assert_eq!(line_len(&buffer, 9), 0, "past the end");
}

#[test]
fn a_far_scrolled_minified_line_paints_its_window_in_its_colours() {
    // 200k chars, one highlight span per char: a row that scanned every span
    // for every char it walked would cost 4e10 steps here.
    let text = alphabet(200_000);
    let buffer = TextBuffer::from_text(&text);
    let spans = (0..text.len())
        .map(|at| hs(at, at + 1, u16::try_from(at % 2).unwrap_or(0)))
        .collect();
    let highlights = Highlights::from_sorted_spans(spans);
    let theme = Theme::dark();
    let mut state = EditorState::new();
    state.scroll_col = 150_003;
    let area = Rect::new(0, 0, 20, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .highlights(&highlights)
        .theme(&theme)
        .render(area, &mut target, &mut state);

    // A one-line buffer has a three-cell gutter, leaving seventeen.
    assert_eq!(row(&target, 0), format!(" 1 {}", &text[150_003..150_020]));
    for x in 3..area.width {
        let at = 150_003 + usize::from(x - 3);
        let token = TokenId(u16::try_from(at % 2).unwrap_or(0));
        assert_eq!(
            target[(x, 0)].fg,
            theme.color(token).to_ratatui(),
            "cell {x} keeps byte {at}'s colour"
        );
    }
}

#[test]
fn a_long_line_scrolled_to_its_end_still_closes_with_its_trailing_hint() {
    let text = alphabet(10_000);
    let buffer = TextBuffer::from_text(&text);
    let hints = [VirtualText::inline(0, 10_000, "END", ThemeRole::InlayHint)];
    let mut state = EditorState::new();
    state.scroll_col = 9_990;
    let area = Rect::new(0, 0, 20, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .virtual_text(&hints)
        .render(area, &mut target, &mut state);
    assert_eq!(row(&target, 0), format!(" 1 {}END    ", &text[9_990..]));
}

#[test]
fn a_row_cut_by_the_right_edge_never_paints_the_line_end() {
    // The row ends at the viewport edge, long before the line does, so neither
    // the trailing hint nor the selected line break belongs on it.
    let text = alphabet(10_000);
    let buffer = TextBuffer::from_text(&format!("{text}\n"));
    let hints = [VirtualText::inline(0, 10_000, "END", ThemeRole::InlayHint)];
    let mut state = EditorState::new();
    state.select_all(&buffer);
    let area = Rect::new(0, 0, 20, 2);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .virtual_text(&hints)
        .render(area, &mut target, &mut state);
    assert_eq!(row(&target, 0), format!(" 1 {}", &text[..17]));
    assert_eq!(state.virtual_text_rect(0), None, "the hint is off screen");
}

#[test]
fn a_deep_continuation_of_a_wrapped_long_line_paints_its_slice() {
    let text = alphabet(5_000);
    let buffer = TextBuffer::from_text(&text);
    let mut state = EditorState::new();
    state.scroll_subrow = 100;
    let area = Rect::new(0, 0, 20, 2);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut target, &mut state);
    // Seventeen chars a row, so row 100 starts at char 1700.
    assert_eq!(row(&target, 0), format!("   {}", &text[1_700..1_717]));
    assert_eq!(row(&target, 1), format!("   {}", &text[1_717..1_734]));
}

#[test]
fn the_extent_cache_follows_edits_and_geometry() {
    let mut buffer = TextBuffer::from_text("abc\n\tx\n");
    let mut cache = ExtentCache::default();
    let measured = |buffer: &TextBuffer, line: u32, width: u32, tab: u16| {
        horizontal_extent(&line_chars(buffer, line), width, tab, Default::default())
    };
    assert_eq!(cache.plain(&buffer, 0, 10, 4), measured(&buffer, 0, 10, 4));
    assert_eq!(cache.plain(&buffer, 1, 10, 4), measured(&buffer, 1, 10, 4));
    // A new tab width or viewport width re-measures.
    assert_eq!(cache.plain(&buffer, 1, 10, 8), measured(&buffer, 1, 10, 8));
    assert_eq!(cache.plain(&buffer, 0, 2, 8), measured(&buffer, 0, 2, 8));
    assert_eq!(cache.plain(&buffer, 7, 10, 4), 0, "past the end");

    // So does an edit that keeps the line count and byte length the same.
    let edit = TextEdit {
        range: Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 3),
        },
        new_text: "\t\t\t".to_string(),
    };
    assert!(
        buffer
            .apply_simple(&Change::new(buffer.version(), vec![edit]))
            .is_ok()
    );
    assert_eq!(cache.plain(&buffer, 0, 2, 8), measured(&buffer, 0, 2, 8));
}

#[test]
fn horizontal_scroll_clamps_to_a_line_lengthened_since_the_last_scan() {
    let mut buffer = TextBuffer::from_text("abc\n");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 13, 2); // a three-cell gutter, ten cells of text
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    state.scroll_columns(&buffer, i32::MAX);
    assert_eq!(state.scroll_col, 0, "the line fits");

    let edit = TextEdit {
        range: Range {
            start: LineCol::new(0, 3),
            end: LineCol::new(0, 3),
        },
        new_text: "d".repeat(37),
    };
    assert!(
        buffer
            .apply_simple(&Change::new(buffer.version(), vec![edit]))
            .is_ok()
    );
    state.scroll_columns(&buffer, i32::MAX);
    assert_eq!(
        state.scroll_col, 31,
        "40 chars and the caret slot, 10 shown"
    );
}
