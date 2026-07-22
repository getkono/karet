use karet_core::BytePos;
use karet_core::TokenId;

use super::text::*;
use super::*;

#[test]
fn inline_text_decoration_renders_after_the_line() {
    let buffer = TextBuffer::from_text("let answer = 42;\n");
    let Ok(range) = Range::new(LineCol::new(0, 0), LineCol::new(0, 1)) else {
        return;
    };
    let decoration = Decoration {
        range,
        kind: DecorationKind::InlineText {
            text: "  Ada, initial".to_string(),
            before: false,
        },
        role: Some(ThemeRole::Muted),
    };
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 40, 1);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .decorations(&[decoration])
        .render(area, &mut target, &mut state);
    let rendered: String = (0..area.width)
        .map(|x| target[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(rendered.contains("Ada, initial"));
}

#[test]
fn merge_conflict_decorations_render_section_backgrounds() {
    let buffer = TextBuffer::from_text("<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> topic\n");
    let decorations = crate::conflict_decorations(&buffer.text());
    let theme = Theme::dark();
    let area = Rect::new(0, 0, 32, 5);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer)
        .decorations(&decorations)
        .theme(&theme)
        .focused(false)
        .render(area, &mut target, &mut EditorState::new());

    let ours = (0..area.width)
        .find(|x| target[(*x, 1)].symbol() == "o")
        .unwrap_or_default();
    let theirs = (0..area.width)
        .find(|x| target[(*x, 3)].symbol() == "t")
        .unwrap_or_default();
    assert_eq!(
        target[(ours, 1)].bg,
        theme.role(ThemeRole::DiffModified).to_ratatui()
    );
    assert_eq!(
        target[(theirs, 3)].bg,
        theme.role(ThemeRole::DiffAdded).to_ratatui()
    );
}

#[test]
fn editor_builder_collects_layers() {
    let buffer = TextBuffer::from_text("fn main() {}");
    let _editor = Editor::new(&buffer).diagnostics(&[]).decorations(&[]);
    assert_eq!(EditorState::new().scroll_line, 0);
}

#[test]
fn token_style_uses_highlight_then_default() {
    let theme = Theme::dark();
    let default_fg = theme.role(ThemeRole::Foreground);
    let hl = [HighlightSpan {
        span: karet_core::Span {
            start: BytePos(0),
            end: BytePos(2),
        },
        token: TokenId(0),
    }];
    assert_eq!(
        token_style(1, &hl, &theme, default_fg).fg,
        Some(theme.color(TokenId(0)).to_ratatui())
    );
    assert_eq!(
        token_style(5, &hl, &theme, default_fg).fg,
        Some(default_fg.to_ratatui())
    );
}

#[test]
fn token_style_applies_markup_emphasis() {
    use karet_core::StandardToken;
    let theme = Theme::dark();
    let default_fg = theme.role(ThemeRole::Foreground);
    let hl = [HighlightSpan {
        span: karet_core::Span {
            start: BytePos(0),
            end: BytePos(4),
        },
        token: StandardToken::MarkupBold.id(),
    }];
    // A bold markup span renders bold, not merely recolored.
    let style = token_style(2, &hl, &theme, default_fg);
    assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
    // Unhighlighted text carries no modifier.
    let plain = token_style(9, &hl, &theme, default_fg);
    assert!(plain.add_modifier.is_empty());
}

#[test]
fn scroll_to_keeps_cursor_in_view() {
    let mut state = EditorState::new();
    state.last_height = 10;
    state.scroll_to(LineCol::new(25, 0));
    let vp = state.viewport();
    assert!(vp.start.line <= 25 && 25 < vp.end.line);
    state.scroll_to(LineCol::new(0, 0));
    assert_eq!(state.scroll_line, 0);
}

#[test]
fn motions_clamp_to_buffer() {
    let buffer = TextBuffer::from_text("ab\ncde\nf");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.move_down(&buffer);
    state.move_down(&buffer);
    state.move_down(&buffer); // past the end clamps to the last line
    assert_eq!(state.cursor().line, 2);
    state.goto(&buffer, LineCol::new(1, 99)); // col clamps to the line length
    assert_eq!(state.cursor(), LineCol::new(1, 3));
}

#[test]
fn pos_at_accounts_for_gutter_and_scroll() {
    let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
    let mut state = EditorState::new();
    state.last_height = 3;
    let area = Rect::new(0, 0, 20, 3);
    // gutter = marker(1) + 1 digit + space = 3; column 5 -> content col 2.
    assert_eq!(state.pos_at(area, &buffer, &[], 5, 0), LineCol::new(0, 2));
    // A click past the line end clamps to the line length.
    assert_eq!(state.pos_at(area, &buffer, &[], 100, 0), LineCol::new(0, 5));
    // Vertical scroll shifts the mapped line.
    state.scroll_line = 1;
    assert_eq!(state.pos_at(area, &buffer, &[], 3, 0), LineCol::new(1, 0));
}

#[test]
fn pos_at_skips_collapsed_fold_interiors() {
    let buffer = TextBuffer::from_text("l0\nl1\nl2\nl3\nl4");
    let mut state = EditorState::new();
    state.last_height = 5;
    let area = Rect::new(0, 0, 20, 5);
    // Collapse lines 1..=3 under a fold headered on line 0. Visible order is now
    // l0, l4 — so screen row 1 maps to buffer line 4, not line 1.
    let folds = [Fold {
        start: 0,
        end: 3,
        collapsed: true,
    }];
    assert_eq!(
        state.pos_at(area, &buffer, &folds, 3, 0),
        LineCol::new(0, 0)
    );
    assert_eq!(
        state.pos_at(area, &buffer, &folds, 3, 1),
        LineCol::new(4, 0)
    );
}

#[test]
fn word_wrap_renders_continuations_and_maps_clicks() {
    let buffer = TextBuffer::from_text("alpha beta gamma");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 10, 3); // 3-cell gutter, 7 content cells.
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .focused(true)
        .render(area, &mut buf, &mut state);

    let painted = |row| {
        (0..area.width)
            .map(|x| buf[(x, row)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>()
    };
    assert!(painted(0).contains("alpha"));
    assert!(painted(1).contains("beta"));
    assert!(painted(2).contains("gamma"));
    assert_eq!(state.pos_at(area, &buffer, &[], 4, 1), LineCol::new(0, 7));

    state.place_caret(LineCol::new(0, 8));
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((5, 1)));
}

#[test]
fn wrapped_row_scrolling_walks_continuations_before_lines() {
    let buffer = TextBuffer::from_text("alpha beta gamma\ntail");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 10, 2);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut buf, &mut state);

    state.scroll_rows(&buffer, &[], true, 1);
    assert_eq!((state.scroll_line, state.scroll_subrow), (0, 1));
    state.scroll_rows(&buffer, &[], true, 2);
    assert_eq!((state.scroll_line, state.scroll_subrow), (1, 0));
    state.scroll_rows(&buffer, &[], true, -1);
    assert_eq!((state.scroll_line, state.scroll_subrow), (0, 2));
}

#[test]
fn overflow_scrolling_and_cursor_margin_are_clamped() {
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 25, 1); // 22 content cells, 10-cell margin.
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut buf, &mut state);

    state.goto(&buffer, LineCol::new(0, 15));
    assert_eq!(state.scroll_col, 4);
    state.goto(&buffer, LineCol::new(0, 2));
    assert_eq!(state.scroll_col, 0);

    state.scroll_columns(&buffer, 3);
    assert_eq!(state.scroll_col, 3);
    state.scroll_columns(&buffer, i32::MAX);
    assert_eq!(state.scroll_col, 5, "longest line clamps manual scrolling");
}

#[test]
fn overflow_caret_clamps_to_horizontal_edges() {
    let buffer = TextBuffer::from_text("abcdefghijklmnopqrstuvwxyz");
    let mut state = EditorState::new();
    let area = Rect::new(0, 0, 10, 1); // 3-cell gutter, 7 content cells.
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut buf, &mut state);
    state.scroll_col = 10;

    state.place_caret(LineCol::new(0, 2));
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((3, 0)));
    state.place_caret(LineCol::new(0, 22));
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((9, 0)));

    Editor::new(&buffer)
        .word_wrap(true)
        .render(area, &mut buf, &mut state);
    state.scroll_columns(&buffer, 3);
    assert_eq!(
        state.scroll_col, 0,
        "wrapped views never scroll horizontally"
    );
}

#[test]
fn selection_range_normalizes_and_clears() {
    let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
    let mut state = EditorState::new();
    state.last_height = 3;
    assert_eq!(state.selection_range(), None);
    state.set_caret(&buffer, LineCol::new(0, 2));
    assert_eq!(
        state.selection_range(),
        None,
        "a bare caret is not a selection"
    );
    state.extend_to(&buffer, LineCol::new(1, 1));
    assert_eq!(
        state.selection_range(),
        Some(Range {
            start: LineCol::new(0, 2),
            end: LineCol::new(1, 1),
        })
    );
    // Dragging back above the anchor normalizes start <= end.
    state.extend_to(&buffer, LineCol::new(0, 0));
    assert_eq!(
        state.selection_range(),
        Some(Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 2),
        })
    );
    state.clear_selection();
    assert_eq!(state.selection_range(), None);
}

#[test]
fn render_draws_gutter_and_cursor_line() {
    let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
    let theme = Theme::dark();
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(1, 0));
    let area = Rect::new(0, 0, 20, 3);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .theme(&theme)
        .render(area, &mut buf, &mut state);

    let row0: String = (0..20)
        .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(row0.contains('1'), "gutter line number missing: {row0:?}");
    assert!(row0.contains("alpha"), "content missing: {row0:?}");
    // The cursor row (line 1) carries the cursor-line background.
    assert_eq!(
        buf[(0, 1)].bg,
        theme.role(ThemeRole::CursorLine).to_ratatui()
    );
    // A non-cursor row uses the editor background.
    assert_eq!(
        buf[(0, 0)].bg,
        theme.role(ThemeRole::Background).to_ratatui()
    );
}

#[test]
fn line_word_and_doc_motions() {
    let buffer = TextBuffer::from_text("foo bar\nbaz");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.move_line_end(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
    state.move_line_start(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 0));
    // Word-right lands at the end of each word, then wraps to the next line.
    state.move_word_right(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 3));
    state.move_word_right(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
    state.move_word_right(&buffer);
    assert_eq!(state.cursor(), LineCol::new(1, 0));
    // Word-left from column 0 wraps to the previous line's end.
    state.move_word_left(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 7));
    state.move_doc_end(&buffer);
    assert_eq!(state.cursor(), LineCol::new(1, 3));
    state.move_doc_start(&buffer);
    assert_eq!(state.cursor(), LineCol::new(0, 0));
}

#[test]
fn select_all_spans_the_whole_buffer() {
    let buffer = TextBuffer::from_text("ab\ncde");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.select_all(&buffer);
    assert_eq!(
        state.selection_range(),
        Some(Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(1, 3),
        })
    );
}

#[test]
fn render_draws_a_caret_for_every_cursor() {
    let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
    let theme = Theme::dark();
    let mut state = EditorState::new();
    state.set_carets(&[LineCol::new(0, 0), LineCol::new(2, 0)]);
    assert!(state.has_multiple_cursors());
    let area = Rect::new(0, 0, 20, 3);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .theme(&theme)
        .focused(true)
        .render(area, &mut buf, &mut state);
    // Gutter is 1 marker + 1 digit + 1 space = 3; both caret rows get a caret cell.
    let gutter = 3;
    assert!(buf[(gutter, 0)].modifier.contains(Modifier::REVERSED));
    assert!(buf[(gutter, 2)].modifier.contains(Modifier::REVERSED));
    // The caret-free middle line has no reversed cell.
    let row1_caret = (0..area.width).any(|x| buf[(x, 1)].modifier.contains(Modifier::REVERSED));
    assert!(!row1_caret, "line 1 has no caret");
}

#[test]
fn cell_caret_can_be_suppressed_while_focused() {
    let buffer = TextBuffer::from_text("abc\n");
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(0, 1));
    let area = Rect::new(0, 0, 8, 2);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .focused(true)
        .cell_caret(false)
        .render(area, &mut buf, &mut state);
    let any_caret = (0..area.width)
        .any(|x| (0..area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED)));
    assert!(!any_caret);
}

#[test]
fn primary_caret_cell_matches_rendered_gutter_geometry() {
    let buffer = TextBuffer::from_text("abc\n");
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(0, 2));
    let area = Rect::new(10, 5, 20, 4);
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), Some((15, 5)));
}

#[test]
fn screen_cell_maps_arbitrary_visible_positions() {
    let buffer = TextBuffer::from_text("abc\ndef\n");
    let mut state = EditorState::new();
    let area = Rect::new(10, 5, 20, 4);
    let mut target = Buffer::empty(area);
    Editor::new(&buffer).render(area, &mut target, &mut state);
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(1, 3)),
        Some((16, 6))
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(8, 0)),
        None
    );
    assert_eq!(
        state.screen_cell(area, &buffer, &[], LineCol::new(0, 30)),
        None
    );
}

#[test]
fn set_carets_preserves_count_and_merges_coincident() {
    let mut state = EditorState::new();
    state.set_carets(&[LineCol::new(0, 0), LineCol::new(1, 2)]);
    assert_eq!(state.cursors().selections.len(), 2);
    // Two carets at the same spot collapse back to one.
    state.set_carets(&[LineCol::new(3, 3), LineCol::new(3, 3)]);
    assert!(!state.has_multiple_cursors());
    assert_eq!(state.cursor(), LineCol::new(3, 3));
}

#[test]
fn add_caret_below_clamps_to_short_line() {
    let buffer = TextBuffer::from_text("longline\nab");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 6));
    state.add_caret_below(&buffer);
    let heads: Vec<LineCol> = state.cursors().selections.iter().map(|s| s.head).collect();
    assert_eq!(heads, vec![LineCol::new(0, 6), LineCol::new(1, 2)]);
}

#[test]
fn add_caret_above_is_noop_on_the_top_line() {
    let buffer = TextBuffer::from_text("ab\ncd");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 1));
    state.add_caret_above(&buffer);
    assert!(!state.has_multiple_cursors());
}

#[test]
fn add_caret_toggles_a_coincident_caret() {
    let buffer = TextBuffer::from_text("abcdef");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 0));
    state.add_caret(&buffer, LineCol::new(0, 3));
    assert_eq!(state.cursors().selections.len(), 2);
    // Alt-adding at the same spot removes it, leaving the original.
    state.add_caret(&buffer, LineCol::new(0, 3));
    assert!(!state.has_multiple_cursors());
    assert_eq!(state.cursor(), LineCol::new(0, 0));
}

#[test]
fn add_next_occurrence_selects_word_then_next_match() {
    let buffer = TextBuffer::from_text("foo bar foo");
    let mut state = EditorState::new();
    state.last_height = 4;
    state.place_caret(LineCol::new(0, 1)); // inside the first "foo"
    state.add_next_occurrence(&buffer);
    assert_eq!(
        state.selection_range(),
        Some(Range {
            start: LineCol::new(0, 0),
            end: LineCol::new(0, 3),
        })
    );
    state.add_next_occurrence(&buffer);
    assert!(state.has_multiple_cursors());
    assert!(state.selection_ranges().contains(&Range {
        start: LineCol::new(0, 8),
        end: LineCol::new(0, 11),
    }));
}

#[test]
fn word_bounds_spans_the_word_under_pos() {
    let buffer = TextBuffer::from_text("foo bar");
    assert_eq!(
        word_bounds(&buffer, LineCol::new(0, 5)),
        (LineCol::new(0, 4), LineCol::new(0, 7))
    );
}

#[test]
fn read_only_suppresses_cursor_line_and_caret() {
    let buffer = TextBuffer::from_text("alpha\nbeta\ngamma");
    let theme = Theme::dark();
    let mut state = EditorState::new();
    state.place_caret(LineCol::new(1, 0));
    let area = Rect::new(0, 0, 20, 3);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .theme(&theme)
        .focused(true) // focused, but read-only must still hide the caret
        .read_only(true)
        .render(area, &mut buf, &mut state);

    // The cursor's line carries the plain background, not the cursor-line color.
    assert_eq!(
        buf[(0, 1)].bg,
        theme.role(ThemeRole::Background).to_ratatui(),
        "read-only mode must not highlight the cursor line"
    );
    // No caret cell is drawn anywhere.
    let any_caret = (0..area.width)
        .any(|x| (0..area.height).any(|y| buf[(x, y)].modifier.contains(Modifier::REVERSED)));
    assert!(!any_caret, "read-only mode must not draw a caret");
}

#[test]
fn sticky_scroll_pins_the_active_chain_and_collapses_signatures() {
    let buffer = TextBuffer::from_text(
        "class A {\n  void run(\n      int value\n  ) {\n    value++;\n  }\n}\n",
    );
    let blocks = SemanticBlocks::new(vec![
        SemanticBlock {
            header_start: 0,
            header_end: 0,
            scope_end: 6,
        },
        SemanticBlock {
            header_start: 1,
            header_end: 3,
            scope_end: 5,
        },
    ]);
    let mut state = EditorState::new();
    state.scroll_line = 4;
    let area = Rect::new(0, 0, 24, 4);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .semantic_blocks(&blocks)
        .sticky_scroll(true)
        .render(area, &mut buf, &mut state);

    let row = |y| {
        (0..area.width)
            .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
            .collect::<String>()
    };
    assert!(row(0).contains("class A"));
    assert!(row(1).contains("void run"));
    assert_eq!(buf[(area.right() - 1, 1)].symbol(), "\u{2026}");
    assert!(
        row(2).contains("value++"),
        "one live content row is retained"
    );
    assert_eq!(state.pos_at(area, &buffer, &[], 4, 0).line, 0);
    assert_eq!(state.pos_at(area, &buffer, &[], 4, 1).line, 1);
    assert_eq!(state.pos_at(area, &buffer, &[], 4, 2).line, 4);
}

#[test]
fn disabled_sticky_scroll_preserves_the_original_viewport() {
    let buffer = TextBuffer::from_text("header\none\ntwo\nthree\n");
    let blocks = SemanticBlocks::new(vec![SemanticBlock {
        header_start: 0,
        header_end: 0,
        scope_end: 3,
    }]);
    let mut state = EditorState::new();
    state.scroll_line = 2;
    let area = Rect::new(0, 0, 16, 2);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .semantic_blocks(&blocks)
        .sticky_scroll(false)
        .render(area, &mut buf, &mut state);
    let first: String = (0..area.width)
        .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(first.contains("two"));
    assert!(state.sticky_rows.is_empty());
    assert_eq!(state.last_height, area.height);
}

#[test]
fn a_tiny_viewport_keeps_the_deepest_header_and_one_content_row() {
    let buffer = TextBuffer::from_text("outer\ninner\ndeep\nbody\n");
    let blocks = SemanticBlocks::new(vec![
        SemanticBlock {
            header_start: 0,
            header_end: 0,
            scope_end: 3,
        },
        SemanticBlock {
            header_start: 1,
            header_end: 1,
            scope_end: 3,
        },
        SemanticBlock {
            header_start: 2,
            header_end: 2,
            scope_end: 3,
        },
    ]);
    let mut state = EditorState::new();
    state.scroll_line = 3;
    let area = Rect::new(0, 0, 16, 2);
    let mut buf = Buffer::empty(area);
    Editor::new(&buffer)
        .semantic_blocks(&blocks)
        .sticky_scroll(true)
        .render(area, &mut buf, &mut state);
    assert_eq!(state.sticky_rows, [2]);
    assert_eq!(state.last_height, 1);
    assert_eq!(state.primary_caret_cell(area, &buffer, &[]), None);
}

#[test]
fn center_on_and_scroll_paging_move_viewport_only() {
    let mut state = EditorState::new();
    state.last_height = 10;
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
