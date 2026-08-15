use super::*;

// --- list_context ---

#[test]
fn bullet_ordered_and_task_markers_parse() {
    let b = list_context("- item");
    assert_eq!(
        b,
        Some(ListContext {
            indent: String::new(),
            marker: ListMarker::Bullet('-'),
            task: None,
            content_col: 2,
            content_empty: false,
        })
    );
    let o = list_context("  12) text");
    assert_eq!(
        o.map(|c| (c.indent, c.marker, c.content_col)),
        Some((
            "  ".to_owned(),
            ListMarker::Ordered {
                number: 12,
                delimiter: ')'
            },
            6
        ))
    );
    let t = list_context("- [x] done");
    assert_eq!(t.as_ref().and_then(|c| c.task), Some(true));
    assert_eq!(t.map(|c| c.content_col), Some(6));
    let unchecked = list_context("* [ ] todo");
    assert_eq!(unchecked.and_then(|c| c.task), Some(false));
}

#[test]
fn non_lists_do_not_parse() {
    assert_eq!(list_context("plain text"), None);
    assert_eq!(list_context("1x. not a marker"), None);
    assert_eq!(list_context("-not a list"), None);
    // Digits with no delimiter are prose, not a marker.
    assert_eq!(list_context("2024 was a year"), None);
    // Absurdly long numbers are left alone.
    assert_eq!(list_context("12345678901. huge"), None);
}

#[test]
fn an_empty_item_reports_empty_content() {
    assert!(list_context("- ").is_some_and(|c| c.content_empty));
    assert!(list_context("-").is_some_and(|c| c.content_empty));
    assert!(list_context("3. ").is_some_and(|c| c.content_empty));
    assert!(list_context("- [ ] ").is_some_and(|c| c.content_empty));
    assert!(!list_context("- x").is_some_and(|c| c.content_empty));
}

// --- in_fenced_code_block ---

#[test]
fn fences_gate_their_interior_only() {
    let text = "a\n```rust\n- not a list\n```\n- list\n";
    assert!(!in_fenced_code_block(text, 0));
    assert!(!in_fenced_code_block(text, 1)); // the opener itself is outside
    assert!(in_fenced_code_block(text, 2));
    assert!(!in_fenced_code_block(text, 4));
}

#[test]
fn a_closing_fence_must_match_character_and_length() {
    // A shorter run or the other fence character does not close the block.
    let text = "````\n```\n~~~\nstill inside\n````\noutside\n";
    assert!(in_fenced_code_block(text, 3));
    assert!(!in_fenced_code_block(text, 5));
}

// --- continue_list ---

#[test]
fn enter_continues_bullets_numbers_and_tasks() {
    assert_eq!(
        continue_list("- alpha\n", 0, 7),
        Some(ListContinuation::Continue {
            insert: "- ".to_owned()
        })
    );
    assert_eq!(
        continue_list("  3. gamma\n", 0, 10),
        Some(ListContinuation::Continue {
            insert: "  4. ".to_owned()
        })
    );
    assert_eq!(
        continue_list("- [x] done\n", 0, 10),
        Some(ListContinuation::Continue {
            insert: "- [ ] ".to_owned()
        })
    );
}

#[test]
fn enter_on_an_empty_item_ends_the_list() {
    assert_eq!(
        continue_list("- alpha\n- \n", 1, 2),
        Some(ListContinuation::EndList { marker_end: 2 })
    );
    assert_eq!(
        continue_list("1. \n", 0, 3),
        Some(ListContinuation::EndList { marker_end: 3 })
    );
}

#[test]
fn enter_inside_the_marker_or_a_fence_stays_ordinary() {
    // Before the content start, Enter splits the line like anywhere else.
    assert_eq!(continue_list("- alpha\n", 0, 1), None);
    // Inside a fence, list-looking lines are code.
    assert_eq!(continue_list("```\n- alpha\n```\n", 1, 7), None);
    // A non-list line has no continuation.
    assert_eq!(continue_list("plain\n", 0, 5), None);
}

// --- renumber_ordered ---

#[test]
fn renumbering_walks_the_whole_sibling_run() {
    let text = "1. a\n1. b\n5. c\n";
    let rewrites = renumber_ordered(text, 1);
    assert_eq!(
        rewrites,
        vec![
            LineRewrite {
                line: 1,
                text: "2. b".to_owned()
            },
            LineRewrite {
                line: 2,
                text: "3. c".to_owned()
            },
        ]
    );
}

#[test]
fn renumbering_respects_run_boundaries_and_nesting() {
    let text = "para\n\n1. a\n   - nested\n3. b\n\nother\n1. new list\n";
    // Nested content stays inside the run; the blank line ends it.
    assert_eq!(
        renumber_ordered(text, 2),
        vec![LineRewrite {
            line: 4,
            text: "2. b".to_owned()
        }]
    );
    // The later list is its own run and is already correct.
    assert!(renumber_ordered(text, 7).is_empty());
}

#[test]
fn renumbering_starts_from_the_first_item_number() {
    // A run starting at 7 keeps its offset — only gaps close up.
    let text = "7. a\n9. b\n";
    assert_eq!(
        renumber_ordered(text, 0),
        vec![LineRewrite {
            line: 1,
            text: "8. b".to_owned()
        }]
    );
}

#[test]
fn bullets_and_non_lists_do_not_renumber() {
    assert!(renumber_ordered("- a\n- b\n", 0).is_empty());
    assert!(renumber_ordered("plain\n", 0).is_empty());
}

// --- toggle_task ---

#[test]
fn task_checkboxes_toggle_both_ways() {
    assert_eq!(toggle_task("- [ ] todo"), Some("- [x] todo".to_owned()));
    assert_eq!(toggle_task("- [x] done"), Some("- [ ] done".to_owned()));
    assert_eq!(
        toggle_task("  2. [X] deep"),
        Some("  2. [ ] deep".to_owned())
    );
    assert_eq!(toggle_task("- no box"), None);
    assert_eq!(toggle_task("plain"), None);
}

// --- toggle_surround ---

#[test]
fn a_selection_wraps_and_unwraps_symmetrically() {
    let on = toggle_surround("make this bold", 5, 9, "**");
    assert_eq!(
        on,
        Some(InlineToggle {
            text: "make **this** bold".to_owned(),
            start: 7,
            end: 11,
        })
    );
    // Toggling again over the (moved) selection takes the markers back off.
    let off = toggle_surround("make **this** bold", 7, 11, "**");
    assert_eq!(
        off,
        Some(InlineToggle {
            text: "make this bold".to_owned(),
            start: 5,
            end: 9,
        })
    );
}

#[test]
fn a_selection_including_the_markers_unwraps_too() {
    let off = toggle_surround("make **this** bold", 5, 13, "**");
    assert_eq!(
        off,
        Some(InlineToggle {
            text: "make this bold".to_owned(),
            start: 5,
            end: 9,
        })
    );
}

#[test]
fn an_empty_selection_expands_to_the_word_under_the_caret() {
    let on = toggle_surround("emphasize word here", 12, 12, "*");
    assert_eq!(
        on,
        Some(InlineToggle {
            text: "emphasize *word* here".to_owned(),
            start: 11,
            end: 15,
        })
    );
    // A caret on whitespace has nothing to toggle.
    assert_eq!(toggle_surround("a  b", 2, 2, "*"), None);
}

#[test]
fn strikethrough_and_code_span_markers_work() {
    assert_eq!(
        toggle_surround("gone", 0, 4, "~~").map(|t| t.text),
        Some("~~gone~~".to_owned())
    );
    assert_eq!(
        toggle_surround("~~gone~~", 2, 6, "~~").map(|t| t.text),
        Some("gone".to_owned())
    );
    assert_eq!(
        toggle_surround("call foo now", 5, 8, "`").map(|t| t.text),
        Some("call `foo` now".to_owned())
    );
}

#[test]
fn multibyte_text_keeps_character_columns() {
    let on = toggle_surround("héllo wörld", 6, 11, "**");
    assert_eq!(
        on,
        Some(InlineToggle {
            text: "héllo **wörld**".to_owned(),
            start: 8,
            end: 13,
        })
    );
}
