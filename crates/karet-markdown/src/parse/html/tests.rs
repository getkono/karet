use crate::Alignment;
use crate::Block;
use crate::ImageRef;
use crate::Inline;
use crate::ListItem;
use crate::MarkdownDocument;
use crate::parse::parse;

fn text(text: &str) -> Inline {
    Inline::Text(text.to_owned())
}

fn paragraph(inlines: Vec<Inline>) -> Block {
    Block::Paragraph(inlines)
}

fn centered(blocks: Vec<Block>) -> Block {
    Block::Aligned {
        align: Alignment::Center,
        blocks,
    }
}

/// Every piece of plain text in `blocks`, concatenated in order.
fn all_text(blocks: &[Block]) -> String {
    fn inlines(content: &[Inline], out: &mut String) {
        for inline in content {
            match inline {
                Inline::Text(t) | Inline::Code(t) => out.push_str(t),
                Inline::Emphasis(c) | Inline::Strong(c) | Inline::Strikethrough(c) => {
                    inlines(c, out);
                },
                Inline::Link { text, .. } => out.push_str(text),
                Inline::Image(image) => out.push_str(&image.alt),
            }
        }
    }
    let mut out = String::new();
    for block in blocks {
        match block {
            Block::Paragraph(c) | Block::Heading { content: c, .. } => inlines(c, &mut out),
            Block::Quote(b) | Block::Aligned { blocks: b, .. } => out.push_str(&all_text(b)),
            Block::List { items, .. } => {
                for item in items {
                    out.push_str(&all_text(&item.blocks));
                }
            },
            Block::CodeBlock { code, .. } => out.push_str(code),
            _ => {},
        }
        out.push('|');
    }
    out
}

fn lines(doc: &MarkdownDocument) -> Vec<usize> {
    (0..doc.blocks.len())
        .filter_map(|index| doc.block_line(index))
        .collect()
}

#[test]
fn a_centered_paragraph_of_one_image_is_an_aligned_image() {
    let doc = parse(
        "<p align=\"center\">\n  <img src=\"logo.png\" alt=\"Logo\" width=\"200px\">\n</p>\n",
    );
    assert_eq!(
        doc.blocks,
        vec![centered(vec![paragraph(vec![Inline::Image(ImageRef {
            alt: "Logo".to_owned(),
            src: "logo.png".to_owned(),
            width: Some(200),
            ..ImageRef::default()
        }),])])]
    );
}

#[test]
fn an_image_keeps_its_title_and_height_and_ignores_a_percentage() {
    let doc = parse("<img src=\"a.png\" title=\"T\" width=\"50%\" height=\"80\">\n");
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Paragraph(inlines)) if inlines.first() == Some(&Inline::Image(ImageRef {
            src: "a.png".to_owned(),
            title: Some("T".to_owned()),
            height: Some(80),
            ..ImageRef::default()
        }))
    ));
}

#[test]
fn an_image_without_a_source_shows_its_alt_text() {
    assert_eq!(
        parse("<img alt=\"nothing here\">\n").blocks,
        vec![paragraph(vec![text("nothing here")])]
    );
    assert!(parse("<img>\n").blocks.is_empty());
}

#[test]
fn a_linked_html_image_stays_an_image() {
    let doc = parse("<a href=\"https://ci\"><img src=\"badge.svg\" alt=\"ci\"></a>\n");
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Paragraph(inlines))
            if matches!(inlines.first(), Some(Inline::Image(ImageRef { link: Some(link), .. })) if link == "https://ci")
    ));
}

#[test]
fn center_and_right_alignment_are_honoured_and_left_declares_nothing() {
    assert_eq!(
        parse("<center>hi</center>\n").blocks,
        vec![centered(vec![paragraph(vec![text("hi")])])]
    );
    assert_eq!(
        parse("<div align=RIGHT>hi</div>\n").blocks,
        vec![Block::Aligned {
            align: Alignment::Right,
            blocks: vec![paragraph(vec![text("hi")])],
        }]
    );
    assert_eq!(
        parse("<div align=\"left\">hi</div>\n").blocks,
        vec![paragraph(vec![text("hi")])]
    );
}

#[test]
fn a_container_spanning_markdown_aligns_each_block_and_anchors_each_on_its_line() {
    let doc = parse("<div align=\"center\">\n\n# Title\n\n![x](y.png)\n\n</div>\n\ntail\n");
    assert!(matches!(
        doc.blocks.as_slice(),
        [
            Block::Aligned { blocks: a, .. },
            Block::Aligned { blocks: b, .. },
            Block::Paragraph(_),
        ] if matches!(a.as_slice(), [Block::Heading { level: 1, .. }])
            && matches!(b.as_slice(), [Block::Paragraph(_)])
    ));
    assert_eq!(lines(&doc), vec![2, 4, 8]);
}

#[test]
fn an_aligned_heading_is_a_heading() {
    assert_eq!(
        parse("<h2 align=\"center\">Hello <em>world</em></h2>\n").blocks,
        vec![centered(vec![Block::Heading {
            level: 2,
            content: vec![text("Hello "), Inline::Emphasis(vec![text("world")])],
        }])]
    );
    assert!(matches!(
        parse("<h3>Plain</h3>\n").blocks.first(),
        Some(Block::Heading { level: 3, .. })
    ));
}

#[test]
fn details_show_expanded_under_a_bold_summary() {
    let doc = parse("<details>\n<summary>More</summary>\n\nHidden **body**\n\n</details>\n");
    assert_eq!(all_text(&doc.blocks), "▾ More|Hidden body|");
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::Paragraph(inlines)) if matches!(inlines.first(), Some(Inline::Strong(_)))
    ));
}

#[test]
fn inline_tags_map_onto_the_inline_model() {
    let doc = parse(
        "Press <kbd>Ctrl</kbd>+<code>C</code>, <b>b</b> <i>i</i> <del>d</del> <a href=\"x\">l</a>\n",
    );
    assert!(
        matches!(doc.blocks.first(), Some(Block::Paragraph(_))),
        "unexpected shape: {:#?}",
        doc.blocks
    );
    let Some(Block::Paragraph(inlines)) = doc.blocks.first() else {
        return;
    };
    assert!(inlines.contains(&Inline::Code("Ctrl".to_owned())));
    assert!(inlines.contains(&Inline::Code("C".to_owned())));
    assert!(inlines.contains(&Inline::Strong(vec![text("b")])));
    assert!(inlines.contains(&Inline::Emphasis(vec![text("i")])));
    assert!(inlines.contains(&Inline::Strikethrough(vec![text("d")])));
    assert!(inlines.contains(&Inline::Link {
        text: "l".to_owned(),
        href: "x".to_owned(),
    }));
}

#[test]
fn a_line_break_breaks_the_line_only_inside_text() {
    assert_eq!(
        parse("a<br>b\n").blocks,
        vec![paragraph(vec![text("a"), text("\n"), text("b")])]
    );
    // Between blocks it has nothing to break.
    assert_eq!(parse("<div><br></div>\n").blocks, Vec::<Block>::new());
}

#[test]
fn an_anchor_without_a_target_keeps_its_text() {
    assert_eq!(
        parse("<a name=\"top\">Top</a>\n").blocks,
        vec![paragraph(vec![text("Top")])]
    );
}

#[test]
fn html_lists_number_from_their_start_and_close_open_items() {
    let doc = parse("<ol start=\"3\">\n<li>one\n<li>two <b>b</b>\n</ol>\n");
    assert!(
        matches!(doc.blocks.first(), Some(Block::List { .. })),
        "unexpected shape: {:#?}",
        doc.blocks
    );
    let Some(Block::List { start, items }) = doc.blocks.first() else {
        return;
    };
    assert_eq!(*start, Some(3));
    assert_eq!(items.len(), 2);
    assert_eq!(all_text(&doc.blocks), "one|two b||");
    assert!(matches!(
        parse("<ul><li>x</li></ul>\n").blocks.first(),
        Some(Block::List { start: None, items }) if items.len() == 1
    ));
}

#[test]
fn a_nested_html_list_stays_inside_its_item() {
    let doc = parse("<ul>\n<li>a\n<ul><li>b</li></ul>\n</li>\n<li>c</li>\n</ul>\n");
    assert!(
        matches!(doc.blocks.first(), Some(Block::List { .. })),
        "unexpected shape: {:#?}",
        doc.blocks
    );
    let Some(Block::List { items, .. }) = doc.blocks.first() else {
        return;
    };
    assert_eq!(items.len(), 2);
    assert!(matches!(
        items.first().map(|item| item.blocks.as_slice()),
        Some([Block::Paragraph(_), Block::List { .. }])
    ));
}

#[test]
fn content_straight_inside_a_list_gets_an_item() {
    assert_eq!(
        parse("<ul>loose</ul>\n").blocks,
        vec![Block::List {
            start: None,
            items: vec![ListItem {
                task: None,
                blocks: vec![paragraph(vec![text("loose")])],
            }],
        }]
    );
}

#[test]
fn a_blockquote_and_a_rule_are_their_markdown_equivalents() {
    assert_eq!(
        parse("<blockquote>q</blockquote>\n").blocks,
        vec![Block::Quote(vec![paragraph(vec![text("q")])])]
    );
    assert_eq!(parse("<hr>\n").blocks, vec![Block::Rule]);
}

#[test]
fn comments_are_hidden_and_raw_elements_dropped_with_their_content() {
    assert_eq!(
        all_text(&parse("<!-- hidden -->\n\nshown\n").blocks),
        "shown|"
    );
    assert_eq!(
        all_text(&parse("<script>\nalert(1)\n</script>\n\npara\n").blocks),
        "para|"
    );
    assert_eq!(
        all_text(&parse("<style>p { x: y }</style>\n\npara\n").blocks),
        "para|"
    );
    // Inline, the content between the tags is markdown text — dropped all the same.
    assert_eq!(
        all_text(&parse("a <script>alert(1)</script> b <iframe>x</iframe> c\n").blocks),
        "a  b  c|"
    );
}

#[test]
fn an_unclosed_inline_raw_element_ends_with_its_paragraph() {
    assert_eq!(
        all_text(&parse("a <script>lost\n\nkept\n").blocks),
        "a|kept|"
    );
}

#[test]
fn unknown_tags_drop_their_markup_and_keep_their_text() {
    assert_eq!(
        parse("<span class=x>a</span> <sup>2</sup> <foo-bar>c</foo-bar>\n").blocks,
        vec![paragraph(vec![
            text("a"),
            text(" "),
            text("2"),
            text(" "),
            text("c"),
        ])]
    );
}

#[test]
fn an_html_table_reads_as_text_a_row_per_line() {
    let doc = parse("<table>\n<tr><td>a</td><td>b</td></tr>\n<tr><td>c</td></tr>\n</table>\n");
    assert_eq!(all_text(&doc.blocks), "a b|c|");
}

#[test]
fn stray_and_mismatched_tags_lose_no_content() {
    for (source, expected) in [
        ("</div>text</p>\n", "text|"),
        ("<b>never closed\n\nnext\n", "never closed|next|"),
        ("**a <b>b** c</b> d\n", "a b c d|"),
        ("<p>one<p>two\n", "one|two|"),
        ("<div>\n\n- item\n\n</span></div>\n", "item|||"),
    ] {
        assert_eq!(all_text(&parse(source).blocks), expected, "{source:?}");
    }
}

#[test]
fn deep_nesting_is_survivable() {
    let source = format!(
        "{}deep{}\n",
        "<div>".repeat(10_000),
        "</div>".repeat(10_000)
    );
    assert_eq!(all_text(&parse(&source).blocks), "deep|");
    let source = format!("{}\n\ndeep\n", "<div align=center>\n".repeat(2_000));
    assert!(!parse(&source).blocks.is_empty());
}

#[test]
fn block_lines_stay_parallel_to_blocks_on_html() {
    for source in [
        "<div>",
        "</div>",
        "<p align=center>\n\n# a\n\n<h1>b",
        "<ul><li><ol><li>x",
        "> <div align=right>\n> q\n\n- <b>x\n",
        "<details><summary>s",
        "<a href=x><img src=y>",
        "| <b>a</b> |\n| - |\n| <img src=x> |\n",
    ] {
        let doc = parse(source);
        assert_eq!(lines(&doc).len(), doc.blocks.len(), "{source:?}");
    }
}

#[test]
fn parsing_is_deterministic() {
    let source = "<div align=center>\n\n<img src=a.png>\n\n**x <i>y</i>**\n\n</div>\n";
    assert_eq!(parse(source).blocks, parse(source).blocks);
}

#[test]
fn an_inline_element_closed_at_the_top_level_leaves_its_paragraph_open() {
    // `</a>` hands its image to a fresh paragraph; the next image joins that paragraph.
    let doc = parse(
        "<p align=\"center\">\n  <a href=\"https://ci\"><img src=\"a.svg\" alt=\"A\"></a>\n  <img src=\"b.svg\" alt=\"B\">\n</p>\n",
    );
    assert!(
        matches!(
            doc.blocks.as_slice(),
            [Block::Aligned { blocks, .. }]
                if matches!(blocks.as_slice(), [Block::Paragraph(inlines)] if inlines.len() == 3)
        ),
        "{:#?}",
        doc.blocks
    );
    assert_eq!(all_text(&parse("<b>x</b> y\n").blocks), "x y|");
}
