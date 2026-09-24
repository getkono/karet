//! `pulldown-cmark` events → the [`MarkdownDocument`] render model.

#[cfg(test)]
mod tests;

use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;

use crate::Alignment;
use crate::Block;
use crate::Cell;
use crate::ImageRef;
use crate::Inline;
use crate::ListItem;
use crate::MarkdownDocument;
use crate::Row;

/// A container element currently being built.
enum Frame {
    Quote(Vec<Block>),
    List {
        start: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        /// Set by the `TaskListMarker` event, which arrives before the item's content.
        task: Option<bool>,
        blocks: Vec<Block>,
    },
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
    Strikethrough(Vec<Inline>),
    /// A link. `image` holds an image that opened the link's label, so a linked image
    /// (`[![badge](b.svg)](https://ci)`) stays an image rather than flattening to its alt.
    Link {
        href: String,
        text: String,
        image: Option<ImageRef>,
    },
    /// An image; its alt text collects in `alt`.
    Image {
        src: String,
        title: Option<String>,
        alt: String,
    },
    CodeBlock {
        lang: Option<String>,
        code: String,
    },
    Table {
        alignments: Vec<Alignment>,
        header: Row,
        rows: Vec<Row>,
    },
    /// A table row. `head` marks the header row, which arrives as `TableHead` rather
    /// than `TableRow` and lands in the table's `header` instead of its `rows`.
    TableRow {
        cells: Row,
        head: bool,
    },
    TableCell(Cell),
}

/// Whether `frame` is the one `tag` closes.
fn closes(frame: &Frame, tag: TagEnd) -> bool {
    matches!(
        (frame, tag),
        (Frame::Paragraph { .. }, TagEnd::Paragraph)
            | (Frame::Heading { .. }, TagEnd::Heading(_))
            | (Frame::Quote(_), TagEnd::BlockQuote(_))
            | (Frame::CodeBlock { .. }, TagEnd::CodeBlock)
            | (Frame::List { .. }, TagEnd::List(_))
            | (Frame::Item { .. }, TagEnd::Item)
            | (Frame::Emphasis(_), TagEnd::Emphasis)
            | (Frame::Strong(_), TagEnd::Strong)
            | (Frame::Strikethrough(_), TagEnd::Strikethrough)
            | (Frame::Link { .. }, TagEnd::Link)
            | (Frame::Image { .. }, TagEnd::Image)
            | (Frame::Table { .. }, TagEnd::Table)
            // A header row and a body row share one frame; the two end tags never nest,
            // so either closing the row frame is unambiguous.
            | (Frame::TableRow { .. }, TagEnd::TableHead | TagEnd::TableRow)
            | (Frame::TableCell(_), TagEnd::TableCell)
    )
}

/// Parse `source` into the render model.
pub(crate) fn parse(source: &str) -> MarkdownDocument {
    let mut builder = Builder::new(source);
    // CommonMark plus the GitHub extensions the model has a shape for. The rest
    // (footnotes, math) would only produce events we silently drop.
    //
    // `into_offset_iter` pairs each event with its source byte range, which is what lets
    // a top-level block remember the line it came from (see `Builder::block_lines`).
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    for (event, span) in Parser::new_ext(source, options).into_offset_iter() {
        builder.event(&event, span.start);
    }
    builder.finish()
}

struct Builder {
    blocks: Vec<Block>,
    /// The 0-based source line each top-level block begins on; parallel to `blocks`.
    block_lines: Vec<usize>,
    stack: Vec<Frame>,
    /// The byte offset of every `\n` in the source, ascending.
    newlines: Vec<usize>,
    /// The byte offset at which the currently-open top-level block began.
    pending_start: usize,
}

impl Builder {
    fn new(source: &str) -> Self {
        Self {
            blocks: Vec::new(),
            block_lines: Vec::new(),
            stack: Vec::new(),
            newlines: source.match_indices('\n').map(|(index, _)| index).collect(),
            pending_start: 0,
        }
    }

    /// The 0-based line holding byte `offset`. A `\n` belongs to the line it ends.
    fn line_of(&self, offset: usize) -> usize {
        self.newlines.partition_point(|&newline| newline < offset)
    }

    fn finish(mut self) -> MarkdownDocument {
        // Unbalanced input (never produced by pulldown-cmark, but cheap to survive):
        // close whatever is still open so no content is lost.
        while !self.stack.is_empty() {
            self.close();
        }
        MarkdownDocument {
            blocks: self.blocks,
            block_lines: self.block_lines,
        }
    }

    fn event(&mut self, event: &Event<'_>, start: usize) {
        // An event seen with an empty stack opens the next top-level block: record where
        // it began, before any frame hides the transition. The value survives untouched
        // until that block closes, because every event in between sees a non-empty stack.
        if self.stack.is_empty() {
            self.pending_start = start;
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(*tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => self.inline(Inline::Code(code.to_string())),
            Event::SoftBreak => self.text(" "),
            // A hard break ends the line; the wrapper honors an embedded newline.
            Event::HardBreak => self.text("\n"),
            Event::Rule => self.block(Block::Rule),
            // The marker arrives inside its item, ahead of the item's content, so the
            // item frame is on top and no paragraph has opened yet.
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(*checked);
                }
            },
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
            // `Tag::List(Some(n))` is an ordered list starting at `n`; `None` is a bullet.
            Tag::List(start) => Frame::List {
                start: *start,
                items: Vec::new(),
            },
            Tag::Item => Frame::Item {
                task: None,
                blocks: Vec::new(),
            },
            Tag::Emphasis => Frame::Emphasis(Vec::new()),
            Tag::Strong => Frame::Strong(Vec::new()),
            Tag::Strikethrough => Frame::Strikethrough(Vec::new()),
            Tag::Link { dest_url, .. } => Frame::Link {
                href: dest_url.to_string(),
                text: String::new(),
                image: None,
            },
            Tag::Image {
                dest_url, title, ..
            } => Frame::Image {
                src: dest_url.to_string(),
                title: (!title.is_empty()).then(|| title.to_string()),
                alt: String::new(),
            },
            Tag::Table(alignments) => Frame::Table {
                alignments: alignments.iter().copied().map(alignment).collect(),
                header: Vec::new(),
                rows: Vec::new(),
            },
            Tag::TableHead => Frame::TableRow {
                cells: Vec::new(),
                head: true,
            },
            Tag::TableRow => Frame::TableRow {
                cells: Vec::new(),
                head: false,
            },
            Tag::TableCell => Frame::TableCell(Vec::new()),
            _ => return, // footnotes, HTML blocks: no model shape
        };
        self.stack.push(frame);
    }

    fn end(&mut self, tag: TagEnd) {
        // An unmodelled tag (a footnote, an HTML block) pushed no frame; closing on it
        // would tear down an unrelated one.
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
            Frame::List { start, items } => self.block(Block::List { start, items }),
            Frame::Item { task, blocks } => {
                if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                    items.push(ListItem { task, blocks });
                } else {
                    // An item outside a list: keep its content rather than drop it.
                    for block in blocks {
                        self.push_root(block);
                    }
                }
            },
            Frame::Emphasis(content) => self.inline(Inline::Emphasis(content)),
            Frame::Strong(content) => self.inline(Inline::Strong(content)),
            Frame::Strikethrough(content) => self.inline(Inline::Strikethrough(content)),
            Frame::Link { href, text, image } => self.inline(close_link(href, text, image)),
            Frame::Image { src, title, alt } => self.inline(Inline::Image(ImageRef {
                alt,
                src,
                title,
                ..ImageRef::default()
            })),
            Frame::Table {
                alignments,
                header,
                rows,
            } => self.block(Block::Table {
                header,
                alignments,
                rows,
            }),
            // A row or cell only ever closes inside its parent; outside one there is
            // nowhere to attach it, and its content is dropped.
            Frame::TableRow { cells, head } => {
                if let Some(Frame::Table { header, rows, .. }) = self.stack.last_mut() {
                    if head {
                        *header = cells;
                    } else {
                        rows.push(cells);
                    }
                }
            },
            Frame::TableCell(content) => {
                if let Some(Frame::TableRow { cells, .. }) = self.stack.last_mut() {
                    cells.push(content);
                }
            },
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
                | Frame::Strong(content)
                | Frame::Strikethrough(content)
                | Frame::TableCell(content),
            ) => content.push(inline),
            // A link's label is flattened to text: the model carries no nested inlines
            // inside a link — except an image opening the label, held aside so a linked
            // image survives as one.
            Some(Frame::Link { text, image, .. }) => match inline {
                Inline::Image(img) if image.is_none() && text.trim().is_empty() => {
                    *image = Some(img);
                },
                inline => flatten_into(&inline, text),
            },
            Some(Frame::Image { alt, .. }) => flatten_into(&inline, alt),
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
            Some(Frame::Quote(blocks) | Frame::Item { blocks, .. }) => blocks.push(block),
            _ => self.push_root(block),
        }
    }

    /// Push a block at the document root, stamping the source line it began on so the
    /// two vectors stay parallel.
    fn push_root(&mut self, block: Block) {
        self.blocks.push(block);
        self.block_lines.push(self.line_of(self.pending_start));
    }
}

/// Append an inline's plain text to `out`, discarding its structure.
fn flatten_into(inline: &Inline, out: &mut String) {
    match inline {
        Inline::Text(t) | Inline::Code(t) => out.push_str(t),
        Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strikethrough(children) => {
            for child in children {
                flatten_into(child, out);
            }
        },
        Inline::Link { text, .. } => out.push_str(text),
        Inline::Image(image) => out.push_str(&image.alt),
    }
}

/// The inline a closed link frame becomes: the image it wraps when its label is that
/// image alone, else an ordinary link whose text leads with any held image's alt.
fn close_link(href: String, mut text: String, image: Option<ImageRef>) -> Inline {
    match image {
        Some(mut image) if text.trim().is_empty() => {
            image.link = Some(href);
            Inline::Image(image)
        },
        Some(image) => {
            text.insert_str(0, &image.alt);
            Inline::Link { text, href }
        },
        None => Inline::Link { text, href },
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

/// Map `pulldown-cmark`'s column alignment onto the model's, so the public API stays
/// free of the parser's types.
fn alignment(alignment: pulldown_cmark::Alignment) -> Alignment {
    match alignment {
        pulldown_cmark::Alignment::None => Alignment::None,
        pulldown_cmark::Alignment::Left => Alignment::Left,
        pulldown_cmark::Alignment::Center => Alignment::Center,
        pulldown_cmark::Alignment::Right => Alignment::Right,
    }
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
