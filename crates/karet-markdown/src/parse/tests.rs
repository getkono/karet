use super::*;

/// The inlines of the first block, if it is a paragraph (else empty, which fails
/// the caller's assertions informatively).
fn paragraph(doc: &MarkdownDocument) -> &[Inline] {
    match doc.blocks.first() {
        Some(Block::Paragraph(inlines)) => inlines,
        _ => &[],
    }
}

/// The language of the first block, if it is a code block.
fn code_lang(doc: &MarkdownDocument) -> Option<&str> {
    match doc.blocks.first() {
        Some(Block::CodeBlock { lang, .. }) => lang.as_deref(),
        _ => None,
    }
}

#[test]
fn parses_headings_and_paragraphs() {
    let doc = parse("# Title\n\nSome text.\n");
    assert_eq!(doc.blocks.len(), 2);
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Heading { level: 1, content }) if content == &[Inline::Text("Title".to_owned())]
    ));
    assert!(matches!(doc.blocks.get(1), Some(Block::Paragraph(_))));
}

#[test]
fn parses_inline_emphasis_strong_code_and_links() {
    let doc = parse("a *b* **c** `d` [e](http://f)\n");
    let inlines = paragraph(&doc);
    assert!(inlines.iter().any(|i| matches!(i, Inline::Emphasis(_))));
    assert!(inlines.iter().any(|i| matches!(i, Inline::Strong(_))));
    assert!(inlines.contains(&Inline::Code("d".to_owned())));
    assert!(inlines.contains(&Inline::Link {
        text: "e".to_owned(),
        href: "http://f".to_owned(),
    }));
}

#[test]
fn parses_fenced_code_block_with_language() {
    let doc = parse("```rust\nfn f() {}\n```\n");
    assert_eq!(
        doc.blocks,
        vec![Block::CodeBlock {
            lang: Some("rust".to_owned()),
            code: "fn f() {}\n".to_owned(),
        }]
    );
}

#[test]
fn fence_info_string_is_normalized_to_its_first_word() {
    // ```Rust,no_run names rust — the resolver is fed a clean language name.
    assert_eq!(code_lang(&parse("```Rust,no_run\nx\n```\n")), Some("rust"));
    // A bare fence and an indented block name nothing.
    assert_eq!(code_lang(&parse("```\nx\n```\n")), None);
    assert_eq!(code_lang(&parse("    x\n")), None);
    assert!(matches!(
        parse("    x\n").blocks.first(),
        Some(Block::CodeBlock { .. })
    ));
}

#[test]
fn parses_lists_and_quotes() {
    let doc = parse("- one\n- two\n\n> quoted\n");
    let items = match doc.blocks.first() {
        Some(Block::List { items, .. }) => items.len(),
        _ => 0,
    };
    assert_eq!(items, 2);
    assert!(matches!(doc.blocks.get(1), Some(Block::Quote(_))));
}

/// The `start` of the first block, if it is a list.
fn list_start(source: &str) -> Option<Option<u64>> {
    match parse(source).blocks.first() {
        Some(Block::List { start, .. }) => Some(*start),
        _ => None,
    }
}

/// The `task` of each item of the first block, if it is a list.
fn item_tasks(source: &str) -> Vec<Option<bool>> {
    match parse(source).blocks.first() {
        Some(Block::List { items, .. }) => items.iter().map(|item| item.task).collect(),
        _ => Vec::new(),
    }
}

#[test]
fn a_task_marker_is_lifted_off_the_items_text_and_onto_the_item() {
    assert_eq!(
        item_tasks("- [ ] todo\n- [x] done\n- plain\n"),
        vec![Some(false), Some(true), None,]
    );
    // An upper-case tick is a tick too, and an ordered item can be a task.
    assert_eq!(item_tasks("1. [X] done\n"), vec![Some(true)]);
}

#[test]
fn a_task_items_text_survives_the_marker_being_lifted_off_it() {
    let doc = parse("- [x] done\n");
    let items = match doc.blocks.first() {
        Some(Block::List { items, .. }) => items.clone(),
        _ => Vec::new(),
    };
    assert_eq!(
        items.first().map(|item| item.blocks.as_slice()),
        Some(&[Block::Paragraph(vec![Inline::Text("done".to_owned())])][..])
    );
}

#[test]
fn a_bracket_pair_that_is_not_a_task_marker_stays_text() {
    // Only a marker at the head of an item is a checkbox.
    assert_eq!(item_tasks("- not [ ] a task\n"), vec![None]);
}

#[test]
fn an_unordered_list_has_no_start_and_an_ordered_one_keeps_its_first_ordinal() {
    assert_eq!(list_start("- one\n- two\n"), Some(None));
    assert_eq!(list_start("* one\n"), Some(None));
    assert_eq!(list_start("1. one\n2. two\n"), Some(Some(1)));
    // An ordered list may begin anywhere, and the ordinal is the author's.
    assert_eq!(list_start("7. seven\n8. eight\n"), Some(Some(7)));
    assert_eq!(list_start("0. zero\n"), Some(Some(0)));
}

#[test]
fn soft_break_becomes_a_space() {
    let doc = parse("a\nb\n");
    let text: String = paragraph(&doc)
        .iter()
        .map(|i| match i {
            Inline::Text(t) => t.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(text, "a b");
}

#[test]
fn link_label_is_flattened_to_text() {
    // The model carries no nested inlines inside a link, so `*b*` becomes plain `b`.
    let doc = parse("[a *b*](http://c)\n");
    assert!(paragraph(&doc).contains(&Inline::Link {
        text: "a b".to_owned(),
        href: "http://c".to_owned(),
    }));
}

#[test]
fn tight_list_item_text_stays_inside_its_item() {
    // A tight item emits its text with no `Start(Paragraph)`; without an implicit
    // paragraph the text escapes to the document root.
    let doc = parse("- one\n- two\n");
    let items = match doc.blocks.first() {
        Some(Block::List { items, .. }) => items.clone(),
        _ => Vec::new(),
    };
    assert_eq!(items.len(), 2);
    assert_eq!(
        items.first().map(|item| item.blocks.as_slice()),
        Some(&[Block::Paragraph(vec![Inline::Text("one".to_owned())])][..])
    );
    assert_eq!(doc.blocks.len(), 1, "nothing may escape to the root");
}

#[test]
fn a_block_inside_a_tight_item_stays_a_sibling_of_its_text() {
    let doc = parse("- one\n\n  ```\n  x\n  ```\n");
    let items = match doc.blocks.first() {
        Some(Block::List { items, .. }) => items.clone(),
        _ => Vec::new(),
    };
    let first = items.first().cloned().unwrap_or_default().blocks;
    assert_eq!(first.len(), 2, "text and code block, both inside the item");
    assert!(matches!(first.first(), Some(Block::Paragraph(_))));
    assert!(matches!(first.get(1), Some(Block::CodeBlock { .. })));
}

#[test]
fn empty_source_yields_no_blocks() {
    assert!(parse("").blocks.is_empty());
}

/// The first block's table parts (else empty, which fails the caller's assertions
/// informatively).
fn table(doc: &MarkdownDocument) -> (&[Cell], &[Alignment], &[Row]) {
    match doc.blocks.first() {
        Some(Block::Table {
            header,
            alignments,
            rows,
        }) => (header, alignments, rows),
        _ => (&[], &[], &[]),
    }
}

#[test]
fn parses_a_table_with_its_header_alignments_and_rows() {
    let doc = parse("| a | b |\n| :- | -: |\n| 1 | 2 |\n| 3 | 4 |\n");
    let (header, alignments, rows) = table(&doc);
    assert_eq!(
        header,
        [
            vec![Inline::Text("a".to_owned())],
            vec![Inline::Text("b".to_owned())],
        ]
    );
    assert_eq!(alignments, [Alignment::Left, Alignment::Right]);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.first().and_then(|r| r.first()),
        Some(&vec![Inline::Text("1".to_owned())])
    );
}

#[test]
fn a_table_cell_keeps_its_inline_structure() {
    let doc = parse("| `c` | **b** |\n| - | - |\n| [l](http://x) | |\n");
    let (header, _, rows) = table(&doc);
    assert_eq!(header.first(), Some(&vec![Inline::Code("c".to_owned())]));
    assert!(matches!(
        header.get(1).and_then(|c| c.first()),
        Some(Inline::Strong(_))
    ));
    assert!(matches!(
        rows.first().and_then(|r| r.first()).and_then(|c| c.first()),
        Some(Inline::Link { .. })
    ));
    // A cell with no content is present but empty, so the row keeps its shape.
    assert_eq!(rows.first().and_then(|r| r.get(1)), Some(&Vec::new()));
}

#[test]
fn an_undeclared_column_alignment_is_none() {
    assert_eq!(
        table(&parse("| a |\n| --- |\n| 1 |\n")).1,
        [Alignment::None]
    );
}

#[test]
fn a_short_body_row_is_padded_and_a_long_one_truncated() {
    // GFM: a row's cells are matched against the header, dropping the excess.
    let doc = parse("| a | b |\n| - | - |\n| 1 |\n| 1 | 2 | 3 |\n");
    assert_eq!(
        table(&doc).2.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![2, 2]
    );
}

#[test]
fn a_table_nests_inside_a_block_quote() {
    let doc = parse("> | a |\n> | - |\n> | 1 |\n");
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Quote(blocks)) if matches!(blocks.first(), Some(Block::Table { .. }))
    ));
    assert_eq!(doc.blocks.len(), 1, "nothing may escape to the root");
}

#[test]
fn a_table_anchors_on_its_header_line() {
    assert_eq!(
        block_lines("para\n\n| a |\n| - |\n| 1 |\n\ntail\n"),
        vec![0, 2, 6]
    );
}

/// The source line of every top-level block, in order.
fn block_lines(source: &str) -> Vec<usize> {
    let doc = parse(source);
    assert_eq!(
        doc.blocks.len(),
        doc.block_lines.len(),
        "a block line must be stamped for every root block"
    );
    doc.block_lines
}

#[test]
fn top_level_blocks_remember_the_source_line_they_begin_on() {
    assert_eq!(block_lines("# Title\n\nSome text.\n"), vec![0, 2]);
}

#[test]
fn leading_and_repeated_blank_lines_are_counted() {
    assert_eq!(block_lines("\n\n# T\n"), vec![2]);
    assert_eq!(block_lines("a\n\n\n\nb\n"), vec![0, 4]);
}

#[test]
fn a_rule_anchors_on_its_own_line() {
    // `Event::Rule` pushes no frame, so its offset must be read straight off the event.
    assert_eq!(block_lines("a\n\n---\n\nb\n"), vec![0, 2, 4]);
}

#[test]
fn a_code_fence_anchors_on_its_opening_delimiter() {
    assert_eq!(block_lines("```rust\nfn f() {}\n```\n\ntext\n"), vec![0, 4]);
}

#[test]
fn only_top_level_blocks_are_anchored() {
    // The nested item on line 1 is inside the list; the list itself anchors at line 0.
    assert_eq!(block_lines("- one\n  - two\n\n> quoted\n"), vec![0, 3]);
}

#[test]
fn a_multi_line_paragraph_anchors_on_its_first_line() {
    assert_eq!(block_lines("# H\n\nsoft\nbreak\n\n## T\n"), vec![0, 2, 5]);
}

#[test]
fn block_lines_stay_parallel_to_blocks_on_adversarial_input() {
    // Each of these either opens frames it never closes, or emits events the model has
    // no shape for. `block_lines` asserts the two vectors match length.
    for source in [
        "",
        "*unbalanced\n",
        "> quote\n\n- item\n\n<div>html</div>\n\npara\n",
        "| a | b |\n| - | - |\n",
        "\n",
    ] {
        let _ = block_lines(source);
    }
}

#[test]
fn block_lines_ascend() {
    let lines = block_lines("a\n\n# b\n\n---\n\n> c\n\n- d\n");
    assert!(
        lines.windows(2).all(|w| w[0] < w[1]),
        "anchors must ascend: {lines:?}"
    );
}
