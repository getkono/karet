//! `pulldown-cmark` events → the [`MarkdownDocument`] render model.

use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use crate::Block;
use crate::Inline;
use crate::MarkdownDocument;

/// A container element currently being built.
enum Frame {
    Quote(Vec<Block>),
    List(Vec<Vec<Block>>),
    Item(Vec<Block>),
    /// A paragraph. `implicit` marks one we opened ourselves to hold loose inlines — a
    /// *tight* list item emits its text with no `Start(Paragraph)` around it.
    Paragraph {
        content: Vec<Inline>,
        implicit: bool,
    },
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Link {
        href: String,
        text: String,
    },
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
}

/// Whether `frame` is the one `tag` closes.
fn closes(frame: &Frame, tag: TagEnd) -> bool {
    matches!(
        (frame, tag),
        (Frame::Paragraph { .. }, TagEnd::Paragraph)
            | (Frame::Heading { .. }, TagEnd::Heading(_))
            | (Frame::Quote(_), TagEnd::BlockQuote(_))
            | (Frame::CodeBlock { .. }, TagEnd::CodeBlock)
            | (Frame::List(_), TagEnd::List(_))
            | (Frame::Item(_), TagEnd::Item)
            | (Frame::Emphasis(_), TagEnd::Emphasis)
            | (Frame::Strong(_), TagEnd::Strong)
            | (Frame::Link { .. }, TagEnd::Link | TagEnd::Image)
    )
}

/// Parse `source` into the render model.
pub(crate) fn parse(source: &str) -> MarkdownDocument {
    let mut builder = Builder::default();
    // Only CommonMark: the model has no table/footnote/strikethrough shape to hold the
    // extensions, so enabling them would produce events we would silently drop.
    for event in Parser::new_ext(source, Options::empty()) {
        builder.event(&event);
    }
    builder.finish()
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    stack: Vec<Frame>,
}

impl Builder {
    fn finish(mut self) -> MarkdownDocument {
        // Unbalanced input (never produced by pulldown-cmark, but cheap to survive):
        // close whatever is still open so no content is lost.
        while !self.stack.is_empty() {
            self.close();
        }
        MarkdownDocument {
            blocks: self.blocks,
        }
    }

    fn event(&mut self, event: &Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(*tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.inline(Inline::Code(code.to_string())),
            Event::SoftBreak => self.text(" "),
            // A hard break ends the line; the wrapper honors an embedded newline.
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.block(Block::Rule),
            // Inline/block HTML, math and footnotes have no place in the model; their
            // text still arrives as `Event::Text` where it matters.
            _ => {},
        }
    }

    fn start(&mut self, tag: &Tag<'_>) {
        let frame = match tag {
            Tag::Paragraph => Frame::Paragraph {
                content: Vec::new(),
                implicit: false,
            },
            Tag::Heading { level, .. } => Frame::Heading {
                level: heading_level(*level),
                content: Vec::new(),
            },
            Tag::BlockQuote(_) => Frame::Quote(Vec::new()),
            Tag::CodeBlock(kind) => Frame::CodeBlock {
                lang: fence_language(kind),
                code: String::new(),
            },
            Tag::List(_) => Frame::List(Vec::new()),
            Tag::Item => Frame::Item(Vec::new()),
            Tag::Emphasis => Frame::Emphasis(Vec::new()),
            Tag::Strong => Frame::Strong(Vec::new()),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => Frame::Link {
                href: dest_url.to_string(),
                text: String::new(),
            },
            _ => return, // tables, footnotes, HTML blocks: no model shape
        };
        self.stack.push(frame);
    }

    fn end(&mut self, tag: TagEnd) {
        // An unmodelled tag (a table, a footnote) pushed no frame; closing on it would
        // tear down an unrelated one.
        if !self.stack.iter().any(|frame| closes(frame, tag)) {
            return;
        }
        // Close inward-out until the tag's own frame goes: `End(Item)` on a tight list
        // must first close the paragraph we implicitly opened inside it.
        while let Some(top) = self.stack.last() {
            let target = closes(top, tag);
            self.close();
            if target {
                break;
            }
        }
    }

    /// Pop the innermost frame and attach it to its parent.
    fn close(&mut self) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        match frame {
            Frame::Paragraph { content, .. } => self.block(Block::Paragraph(content)),
            Frame::Heading { level, content } => self.block(Block::Heading { level, content }),
            Frame::Quote(blocks) => self.block(Block::Quote(blocks)),
            Frame::CodeBlock { lang, code } => self.block(Block::CodeBlock { lang, code }),
            Frame::List(items) => self.block(Block::List(items)),
            Frame::Item(blocks) => match self.stack.last_mut() {
                Some(Frame::List(items)) => items.push(blocks),
                // An item outside a list: keep its content rather than drop it.
                _ => self.blocks.extend(blocks),
            },
            Frame::Emphasis(content) => self.inline(Inline::Emphasis(content)),
            Frame::Strong(content) => self.inline(Inline::Strong(content)),
            Frame::Link { href, text } => self.inline(Inline::Link { text, href }),
        }
    }

    /// Route text: inside a code block it is raw source, elsewhere it is an inline.
    fn text(&mut self, text: &str) {
        if let Some(Frame::CodeBlock { code, .. }) = self.stack.last_mut() {
            code.push_str(text);
        } else {
            self.inline(Inline::Text(text.to_owned()));
        }
    }

    /// Append an inline to the innermost inline container, opening an implicit paragraph
    /// when the inline lands straight inside a block container (a tight list item).
    fn inline(&mut self, inline: Inline) {
        match self.stack.last_mut() {
            Some(
                Frame::Paragraph { content, .. }
                | Frame::Heading { content, .. }
                | Frame::Emphasis(content)
                | Frame::Strong(content),
            ) => content.push(inline),
            // A link's label is flattened to text: the model carries no nested inlines
            // inside a link.
            Some(Frame::Link { text, .. }) => flatten_into(&inline, text),
            _ => self.stack.push(Frame::Paragraph {
                content: vec![inline],
                implicit: true,
            }),
        }
    }

    /// Append a block to the innermost block container, or to the document root.
    fn block(&mut self, block: Block) {
        // A block cannot sit inside a paragraph: close the implicit one first so this
        // block becomes its sibling rather than escaping to the document root.
        if matches!(
            self.stack.last(),
            Some(Frame::Paragraph { implicit: true, .. })
        ) {
            self.close();
        }
        match self.stack.last_mut() {
            Some(Frame::Quote(blocks) | Frame::Item(blocks)) => blocks.push(block),
            _ => self.blocks.push(block),
        }
    }
}

/// Append an inline's plain text to `out`, discarding its structure.
fn flatten_into(inline: &Inline, out: &mut String) {
    match inline {
        Inline::Text(t) | Inline::Code(t) => out.push_str(t),
        Inline::Emphasis(children) | Inline::Strong(children) => {
            for child in children {
                flatten_into(child, out);
            }
        },
        Inline::Link { text, .. } => out.push_str(text),
    }
}

/// The fence's info string, lowercased and trimmed to its first word (` ```rust,no_run `
/// names rust). `None` for an indented block or a bare fence.
fn fence_language(kind: &CodeBlockKind<'_>) -> Option<String> {
    let CodeBlockKind::Fenced(info) = kind else {
        return None;
    };
    let name = info
        .split(|c: char| c.is_whitespace() || c == ',')
        .next()
        .unwrap_or("")
        .trim();
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
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
            Some(Block::List(items)) => items.len(),
            _ => 0,
        };
        assert_eq!(items, 2);
        assert!(matches!(doc.blocks.get(1), Some(Block::Quote(_))));
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
            Some(Block::List(items)) => items.clone(),
            _ => Vec::new(),
        };
        assert_eq!(items.len(), 2);
        assert_eq!(
            items.first().map(Vec::as_slice),
            Some(&[Block::Paragraph(vec![Inline::Text("one".to_owned())])][..])
        );
        assert_eq!(doc.blocks.len(), 1, "nothing may escape to the root");
    }

    #[test]
    fn a_block_inside_a_tight_item_stays_a_sibling_of_its_text() {
        let doc = parse("- one\n\n  ```\n  x\n  ```\n");
        let items = match doc.blocks.first() {
            Some(Block::List(items)) => items.clone(),
            _ => Vec::new(),
        };
        let first = items.first().cloned().unwrap_or_default();
        assert_eq!(first.len(), 2, "text and code block, both inside the item");
        assert!(matches!(first.first(), Some(Block::Paragraph(_))));
        assert!(matches!(first.get(1), Some(Block::CodeBlock { .. })));
    }

    #[test]
    fn empty_source_yields_no_blocks() {
        assert!(parse("").blocks.is_empty());
    }
}
