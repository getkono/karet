use super::*;
use crate::parse;

fn lines(source: &str, width: u16) -> Vec<String> {
    wrap(&parse::parse(source), width)
        .lines
        .iter()
        .map(WrappedLine::text)
        .collect()
}

#[test]
fn words_preserves_whitespace_chunks() {
    assert_eq!(words("ab  cd"), vec!["ab", "  ", "cd"]);
    assert_eq!(words(""), Vec::<&str>::new());
    assert_eq!(words(" a"), vec![" ", "a"]);
}

#[test]
fn paragraph_wraps_at_word_boundaries() {
    assert_eq!(lines("alpha beta gamma\n", 11), vec!["alpha beta", "gamma"]);
}

#[test]
fn a_word_longer_than_the_line_is_not_split() {
    // Overflowing beats cutting a word (or a grapheme) in half.
    assert_eq!(lines("abcdefgh ij\n", 4), vec!["abcdefgh", "ij"]);
}

#[test]
fn zero_width_terminates() {
    // A degenerate viewport must not spin; it just produces narrow output.
    assert!(!lines("a b c\n", 0).is_empty());
}

#[test]
fn heading_carries_its_marker_and_token() {
    let doc = wrap(&parse::parse("## Title\n"), 40);
    let Some(first) = doc.lines.first() else {
        return;
    };
    assert_eq!(first.text(), "## Title");
    assert!(
        first
            .spans
            .iter()
            .all(|s| s.token == Some(StandardToken::MarkupHeading.id()))
    );
}

#[test]
fn emphasis_and_code_get_their_own_tokens() {
    let doc = wrap(&parse::parse("a *b* `c`\n"), 40);
    let spans: Vec<_> = doc.lines.iter().flat_map(|l| l.spans.iter()).collect();
    assert!(
        spans
            .iter()
            .any(|s| s.token == Some(StandardToken::MarkupItalic.id()) && s.text == "b")
    );
    assert!(
        spans
            .iter()
            .any(|s| s.token == Some(StandardToken::MarkupRaw.id()) && s.text == "c")
    );
}

#[test]
fn list_bullets_the_first_line_and_indents_the_rest() {
    let out = lines("- alpha beta gamma\n", 10);
    assert_eq!(out.first().map(String::as_str), Some("• alpha"));
    // Continuation lines align under the bullet's text, not its marker.
    assert_eq!(out.get(1).map(String::as_str), Some("  beta"));
}

#[test]
fn an_ordered_list_numbers_its_items_from_its_start() {
    assert_eq!(lines("1. one\n2. two\n", 20), vec!["1. one", "2. two"]);
    // The ordinals are the author's, counted up from the first.
    assert_eq!(
        lines("7. seven\n8. eight\n", 20),
        vec!["7. seven", "8. eight"]
    );
}

#[test]
fn ordered_markers_share_a_text_column_once_they_differ_in_width() {
    // `9.` and `10.` must not stagger the items' text.
    let out = lines("9. nine\n10. ten\n11. eleven\n", 20);
    assert_eq!(out, vec!["9.  nine", "10. ten", "11. eleven"]);
}

#[test]
fn an_ordered_items_continuation_aligns_under_its_text() {
    let out = lines("10. alpha beta\n", 11);
    assert_eq!(out, vec!["10. alpha", "    beta"]);
}

#[test]
fn a_nested_ordered_list_numbers_independently_of_its_parent() {
    let out = lines("- bullet\n  1. one\n  2. two\n", 20);
    assert_eq!(out, vec!["• bullet", "  1. one", "  2. two"]);
}

#[test]
fn a_nested_list_hugs_the_item_that_introduces_it() {
    // No blank line between an item's text and its sub-list…
    assert_eq!(lines("- one\n  - two\n", 20), vec!["• one", "  • two"]);
    // …but two paragraphs inside one item still break apart.
    let out = lines("- one\n\n  two\n", 20);
    assert_eq!(
        out,
        vec!["• one".to_owned(), "  ".to_owned(), "  two".to_owned()]
    );
}

#[test]
fn a_list_marker_is_structural_punctuation() {
    let doc = wrap(&parse::parse("1. one\n"), 20);
    let first = doc.lines.first().cloned().unwrap_or_default();
    assert_eq!(
        first.spans.first().and_then(|s| s.token),
        Some(StandardToken::MarkupListMarker.id())
    );
    assert_eq!(first.spans.first().map(|s| s.text.as_str()), Some("1. "));
}

/// `count` plain (non-task) list items.
fn plain_items(count: usize) -> Vec<ListItem> {
    vec![ListItem::default(); count]
}

#[test]
fn list_markers_saturate_rather_than_overflow() {
    assert_eq!(
        list_markers(Some(u64::MAX), &plain_items(2)),
        vec![format!("{}. ", u64::MAX), format!("{}. ", u64::MAX)]
    );
    assert!(
        list_markers(None, &plain_items(3))
            .iter()
            .all(|m| m == BULLET)
    );
    assert!(list_markers(Some(1), &[]).is_empty());
}

#[test]
fn a_task_items_checkbox_replaces_its_bullet_but_follows_its_ordinal() {
    assert_eq!(
        lines("- [ ] todo\n- [x] done\n- plain\n", 20),
        vec!["☐ todo", "☑ done", "• plain",]
    );
    // An ordinal carries meaning the box does not, so both are drawn.
    assert_eq!(
        lines("1. [ ] todo\n2. [x] done\n", 20),
        vec!["1. ☐ todo", "2. ☑ done",]
    );
}

#[test]
fn a_task_items_content_aligns_with_a_plain_items() {
    // The checkbox and the bullet are both two columns, so the text lines up.
    let out = lines("- [x] alpha beta\n- plain\n", 8);
    assert_eq!(out, vec!["☑ alpha", "  beta", "• plain"]);
}

#[test]
fn a_task_checkbox_is_structural_punctuation() {
    let doc = wrap(&parse::parse("- [x] done\n"), 20);
    let first = doc.lines.first().cloned().unwrap_or_default();
    assert_eq!(first.spans.first().map(|s| s.text.as_str()), Some("☑ "));
    assert_eq!(
        first.spans.first().and_then(|s| s.token),
        Some(StandardToken::MarkupListMarker.id())
    );
}

#[test]
fn quote_prefixes_every_line_with_a_gutter() {
    let out = lines("> alpha beta\n", 20);
    assert!(out.iter().all(|l| l.starts_with(QUOTE_GUTTER)));
}

#[test]
fn rule_fills_the_width() {
    let out = lines("---\n", 5);
    assert_eq!(out.first().map(String::as_str), Some("─────"));
}

#[test]
fn code_block_lines_are_raw_markup_without_a_grammar() {
    let doc = wrap(&parse::parse("```\nlet x;\n```\n"), 40);
    let Some(first) = doc.lines.first() else {
        return;
    };
    assert_eq!(first.text(), "let x;");
    assert_eq!(
        first.spans.first().and_then(|s| s.token),
        Some(StandardToken::MarkupRaw.id())
    );
}

#[test]
fn width_is_measured_in_terminal_columns() {
    // A CJK glyph is two columns wide, so only one fits in a width of 3.
    let out = lines("世 界\n", 3);
    assert_eq!(out.len(), 2, "got {out:?}");
}

const TABLE: &str = "| Left | Center | Right |\n| :--- | :----: | ----: |\n\
                         | a | bb | ccc |\n| longer cell | x | y |\n";

#[test]
fn a_table_renders_as_a_box_drawn_grid() {
    assert_eq!(
        lines(TABLE, 60),
        vec![
            "┌─────────────┬────────┬───────┐",
            "│ Left        │ Center │ Right │",
            "├─────────────┼────────┼───────┤",
            "│ a           │   bb   │   ccc │",
            "├─────────────┼────────┼───────┤",
            "│ longer cell │   x    │     y │",
            "└─────────────┴────────┴───────┘",
        ]
    );
}

#[test]
fn table_cells_honor_their_column_alignment() {
    // Row `a | bb | ccc` under `:--- | :----: | ----:`, so left, centered, right.
    let row = lines(TABLE, 60).get(3).cloned().unwrap_or_default();
    assert_eq!(row, "│ a           │   bb   │   ccc │");
}

#[test]
fn a_header_cell_is_bold_unless_the_inline_claims_its_own_token() {
    let doc = wrap(&parse::parse("| a | `c` |\n| - | - |\n| 1 | 2 |\n"), 40);
    let header = doc.lines.get(1).cloned().unwrap_or_default();
    let token = |text: &str| {
        header
            .spans
            .iter()
            .find(|s| s.text == text)
            .and_then(|s| s.token)
    };
    assert_eq!(token("a"), Some(StandardToken::MarkupBold.id()));
    assert_eq!(token("c"), Some(StandardToken::MarkupRaw.id()));
}

#[test]
fn table_borders_are_structural_punctuation() {
    let doc = wrap(&parse::parse(TABLE), 60);
    let top = doc.lines.first().cloned().unwrap_or_default();
    assert_eq!(
        top.spans.first().and_then(|s| s.token),
        Some(StandardToken::MarkupListMarker.id())
    );
}

/// Every line of the grid must be exactly as wide as every other, or the borders and
/// the cells stop lining up.
#[test]
fn every_grid_line_has_the_same_width_at_any_viewport() {
    for width in [4, 7, 12, 20, 33, 60, 200] {
        let doc = wrap(&parse::parse(TABLE), width);
        let widths: Vec<usize> = doc.lines.iter().map(WrappedLine::width).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged grid at width {width}: {widths:?}"
        );
    }
}

#[test]
fn a_narrow_table_shrinks_its_widest_column_first() {
    // The grid wants 32 columns. Given 30, `Center` and `Right` keep their content
    // and only the prose column gives ground — it is the one with room to spare.
    let out = lines(TABLE, 30);
    assert_eq!(
        out.get(1).map(String::as_str),
        Some("│ Left      │ Center │ Right │")
    );
    assert_eq!(
        out.get(5).map(String::as_str),
        Some("│ longer    │   x    │     y │")
    );
}

#[test]
fn every_body_row_is_separated_by_a_horizontal_rule() {
    let out = lines("| h |\n| - |\n| one |\n| two |\n| three |\n", 30);
    assert_eq!(
        out,
        vec![
            "┌───────┐",
            "│ h     │",
            "├───────┤",
            "│ one   │",
            "├───────┤",
            "│ two   │",
            "├───────┤",
            "│ three │",
            "└───────┘",
        ]
    );
}

#[test]
fn a_table_fits_the_viewport_it_is_given() {
    // 60 columns is more than the grid needs, so it renders at its natural width.
    let natural = wrap(&parse::parse(TABLE), 60)
        .lines
        .first()
        .map_or(0, WrappedLine::width);
    assert_eq!(natural, 32);
    // 30 columns is less, so it shrinks to fill exactly those.
    let shrunk = wrap(&parse::parse(TABLE), 30);
    assert_eq!(shrunk.lines.first().map(WrappedLine::width), Some(30));
}

#[test]
fn an_over_long_cell_word_is_broken_rather_than_overflowing_its_column() {
    let out = lines("| h |\n| - |\n| abcdefgh |\n", 9);
    // Content columns: 9 - (3*1 + 1) = 5.
    assert_eq!(
        out,
        vec![
            "┌───────┐",
            "│ h     │",
            "├───────┤",
            "│ abcde │",
            "│ fgh   │",
            "└───────┘",
        ]
    );
}

#[test]
fn split_to_width_never_exceeds_the_width_and_loses_nothing() {
    assert_eq!(split_to_width("abcdef", 2), vec!["ab", "cd", "ef"]);
    // A CJK glyph is two columns, so only one fits per chunk of three.
    assert_eq!(split_to_width("世界", 3), vec!["世", "界"]);
    // A glyph wider than the chunk is emitted alone rather than dropped.
    assert_eq!(split_to_width("世", 1), vec!["世"]);
    assert_eq!(split_to_width("", 4), Vec::<&str>::new());
}

#[test]
fn a_table_with_no_columns_draws_nothing() {
    let mut out = Vec::new();
    wrap_table(&Vec::new(), &[], &[], 20, &[], &mut out);
    assert!(out.is_empty());
}

#[test]
fn a_quoted_table_carries_the_gutter_on_every_line() {
    let out = lines("> | a |\n> | - |\n> | 1 |\n", 30);
    assert!(!out.is_empty());
    assert!(out.iter().all(|l| l.starts_with(QUOTE_GUTTER)), "{out:?}");
}

#[test]
fn a_degenerate_width_still_terminates_and_stays_aligned() {
    let doc = wrap(&parse::parse(TABLE), 0);
    let widths: Vec<usize> = doc.lines.iter().map(WrappedLine::width).collect();
    assert!(widths.windows(2).all(|w| w[0] == w[1]), "{widths:?}");
}

#[test]
fn a_table_anchors_like_any_other_top_level_block() {
    let doc = wrapped("para\n\n| a |\n| - |\n| 1 |\n", 30);
    assert_eq!(doc.anchors.len(), 2);
    assert_eq!(doc.anchors.get(1).map(|a| a.source_line), Some(2));
}

#[test]
fn blocks_are_separated_by_a_blank_line_with_no_trailing_blank() {
    let out = lines("a\n\nb\n", 20);
    assert_eq!(out, vec!["a".to_owned(), String::new(), "b".to_owned()]);
}

#[cfg(feature = "highlight")]
#[test]
fn a_rust_fence_is_syntax_highlighted_end_to_end() {
    use karet_core::TokenId;

    let doc = wrap(&parse::parse("```rust\nfn main() {}\n```\n"), 40);
    let tokens: Vec<_> = doc
        .lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter_map(|s| s.token)
        .collect();
    if tokens.is_empty() {
        return; // no rust grammar compiled into this build
    }
    // `fn` is a keyword, not undifferentiated raw markup.
    assert!(
        tokens.contains(&TokenId::KEYWORD),
        "the fence should be highlighted as rust, got {tokens:?}"
    );
}

#[cfg(feature = "highlight")]
#[test]
fn an_unknown_fence_language_falls_back_to_raw_markup() {
    let doc = wrap(&parse::parse("```brainfuck\n+++.\n```\n"), 40);
    let Some(first) = doc.lines.first() else {
        return;
    };
    assert_eq!(first.text(), "+++.");
    assert_eq!(
        first.spans.first().and_then(|s| s.token),
        Some(StandardToken::MarkupRaw.id())
    );
}

fn wrapped(source: &str, width: u16) -> WrappedDocument {
    wrap(&parse::parse(source), width)
}

#[test]
fn one_anchor_per_top_level_block_at_its_first_line() {
    // "# Title" / "" / "Some text." — the anchor skips the separator blank.
    let doc = wrapped("# Title\n\nSome text.\n", 40);
    assert_eq!(
        doc.anchors,
        vec![
            Anchor {
                source_line: 0,
                wrapped_line: 0
            },
            Anchor {
                source_line: 2,
                wrapped_line: 2
            },
        ]
    );
}

#[test]
fn nested_blocks_do_not_add_anchors() {
    let doc = wrapped("- one\n  - two\n\n> quoted\n", 40);
    assert_eq!(
        doc.anchors.len(),
        2,
        "the list and the quote, nothing inside"
    );
}

#[test]
fn anchors_ascend_on_both_axes() {
    let doc = wrapped("a\n\n# b\n\n---\n\n> c\n\n- d\n", 20);
    assert!(
        doc.anchors
            .windows(2)
            .all(|w| w[0].source_line < w[1].source_line && w[0].wrapped_line < w[1].wrapped_line),
        "{:?}",
        doc.anchors
    );
}

#[test]
fn projections_hit_anchors_exactly() {
    let doc = wrapped("# Title\n\nSome text.\n", 40);
    for anchor in &doc.anchors {
        assert_eq!(
            doc.wrapped_line_for_source(anchor.source_line),
            anchor.wrapped_line
        );
        assert_eq!(
            doc.source_line_for_wrapped(anchor.wrapped_line),
            anchor.source_line
        );
    }
}

#[test]
fn an_empty_document_projects_everything_to_the_top() {
    let doc = wrapped("", 40);
    assert!(doc.anchors.is_empty());
    assert_eq!(doc.wrapped_line_for_source(7), 0);
    assert_eq!(doc.source_line_for_wrapped(7), 0);
}

#[test]
fn a_single_anchor_extends_one_for_one() {
    // One block, so there is no `hi` to interpolate against.
    let doc = wrapped("alpha\nbravo\ncharlie\n", 40);
    assert_eq!(doc.anchors.len(), 1);
    // Source lines beyond the block still map forward, clamped to the last line.
    assert_eq!(doc.wrapped_line_for_source(0), 0);
    assert_eq!(doc.source_line_for_wrapped(2), 2);
}

#[test]
fn a_source_line_below_the_first_anchor_maps_to_the_top() {
    let doc = wrapped("\n\n# Late\n", 40);
    assert_eq!(doc.anchors.first().map(|a| a.source_line), Some(2));
    assert_eq!(doc.wrapped_line_for_source(0), 0);
    assert_eq!(doc.wrapped_line_for_source(1), 0);
}

#[test]
fn a_source_line_past_the_end_clamps_to_the_last_wrapped_line() {
    let doc = wrapped("# Title\n\nSome text.\n", 40);
    let last = doc.lines.len().saturating_sub(1);
    assert_eq!(doc.wrapped_line_for_source(9_999), last);
}

#[test]
fn a_wrapped_line_past_the_end_is_not_clamped() {
    // The source's length is unknown here, so the caller clamps; we just extend.
    let doc = wrapped("# Title\n\nSome text.\n", 40);
    assert!(doc.source_line_for_wrapped(9_999) > 2);
}

#[test]
fn interpolation_lands_strictly_inside_the_block_that_owns_the_line() {
    // A paragraph on source lines 2-3 that soft-wraps into several rendered lines,
    // bracketed by headings, so both anchors exist.
    let doc = wrapped(
        "# H\n\nlorem ipsum dolor\nsit amet consectetur\n\n## Tail\n",
        12,
    );
    let para = doc.wrapped_line_for_source(2);
    let tail = doc.wrapped_line_for_source(5);
    let inner = doc.wrapped_line_for_source(3);
    assert!(
        para < inner && inner < tail,
        "source line 3 must render between the paragraph start and the tail heading: \
             {para} < {inner} < {tail}"
    );
    // And back: the interpolated row belongs to the paragraph, not the tail heading.
    let back = doc.source_line_for_wrapped(inner);
    assert!((2..5).contains(&back), "expected 2..5, got {back}");
}

#[test]
fn round_tripping_a_source_line_stays_within_its_block() {
    let doc = wrapped(
        "# H\n\nlorem ipsum dolor\nsit amet consectetur\n\n## Tail\n",
        12,
    );
    for source_line in 0..6 {
        let back = doc.source_line_for_wrapped(doc.wrapped_line_for_source(source_line));
        assert!(
            back <= source_line.max(2),
            "{source_line} round-tripped to {back}"
        );
    }
}

#[test]
fn a_zero_width_wrap_still_projects_without_panicking() {
    let doc = wrapped("# H\n\ntext\n", 0);
    let _ = doc.wrapped_line_for_source(usize::MAX);
    let _ = doc.source_line_for_wrapped(usize::MAX);
}
