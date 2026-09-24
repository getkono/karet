//! `karet-markdown` — a markdown rendering model for karet (and LSP hover docs).
//!
//! Parses markdown (CommonMark plus GitHub tables and task lists) into a block/inline
//! render model decoupled from any renderer. Enable `view` for a ratatui renderer, and
//! `highlight` to syntax-highlight code fences via `karet-syntax`.
//!
//! Embedded HTML maps onto the same model through a curated subset — text formatting,
//! links, images, headings, lists, `<details>`, and `align="center"`/`"right"` on
//! containers ([`Block::Aligned`]); other tags keep only their text, and `<script>`-like
//! elements vanish with their content. There is no HTML layout engine: see
//! `docs/scope.md` for what the preview deliberately does not render.
//!
//! Two stages. [`parse`] turns source into a tree of [`Block`]s and [`Inline`]s;
//! [`MarkdownDocument::wrap`] soft-wraps that tree to a column width, producing
//! [`WrappedLine`]s of [`TextSpan`]s tagged with a semantic
//! [`TokenId`](karet_core::TokenId). Nothing here knows about a terminal: a consumer
//! resolves those tokens to colors (and bold/italic) through `karet-theme`.
//!
//! A [`WrappedDocument`] also carries [`Anchor`]s tying each top-level block back to the
//! source line it came from, so a rendered preview can be scrolled in step with the
//! markdown it was rendered from.

pub mod edit;
mod html;
#[cfg(feature = "lint")]
pub mod lint;
#[cfg(feature = "mermaid")]
pub mod mermaid;
mod parse;
mod table;
pub mod toc;
mod wrap;

#[cfg(feature = "highlight")]
mod highlight;

#[cfg(feature = "view")]
pub mod view;

pub use wrap::Anchor;
pub use wrap::ImageSlice;
pub use wrap::TextSpan;
pub use wrap::WrappedDocument;
pub use wrap::WrappedLine;

/// An inline span of markdown content.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Inline {
    /// Plain text.
    Text(String),
    /// Inline code.
    Code(String),
    /// Emphasized (italic) content.
    Emphasis(Vec<Inline>),
    /// Strong (bold) content.
    Strong(Vec<Inline>),
    /// Struck-through content (GFM `~~…~~`).
    Strikethrough(Vec<Inline>),
    /// A hyperlink.
    Link {
        /// The link text.
        text: String,
        /// The link target.
        href: String,
    },
    /// An image: markdown `![alt](src "title")`, or an HTML `<img>`.
    Image(ImageRef),
}

/// A referenced image, as written — nothing here has been resolved or loaded.
///
/// Whether it paints as pixels is the consumer's call (see [`ImageSizer`]); without one it
/// renders as a chip carrying its alt text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageRef {
    /// The alternative text.
    pub alt: String,
    /// The image source, verbatim (a relative path, an absolute URL, …).
    pub src: String,
    /// The title, if one was given.
    pub title: Option<String>,
    /// The HTML `width` attribute in CSS pixels, if one was given.
    pub width: Option<u32>,
    /// The HTML `height` attribute in CSS pixels, if one was given.
    pub height: Option<u32>,
    /// The target of the link wrapping the image (`[![badge](b.svg)](https://ci)`), if any.
    pub link: Option<String>,
}

/// Decides which images are painted as pixels, by knowing their size.
///
/// The model does no I/O: a consumer that can load an image reports its native pixel
/// size here, and [`MarkdownDocument::wrap_with`] reserves rows for it. Resolving the
/// source — and refusing one it will not load — is entirely the consumer's policy.
pub trait ImageSizer {
    /// The native `(width, height)` in pixels of `image`, or `None` to render it as a
    /// chip instead.
    fn dimensions(&self, image: &ImageRef) -> Option<(u32, u32)>;
}

/// One item of a [`Block::List`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListItem {
    /// Whether the item's box is ticked, for a task-list item (`- [ ]` / `- [x]`), or
    /// `None` for an ordinary item.
    ///
    /// GitHub spells the checkbox inside the item's first paragraph; the model lifts it
    /// onto the item, where it belongs — it marks the item, exactly as a bullet does.
    pub task: Option<bool>,
    /// The item's content.
    pub blocks: Vec<Block>,
}

/// How a table column's cells are aligned within their column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Alignment {
    /// No alignment was declared; cells render left-aligned.
    #[default]
    None,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

/// One table cell: a run of inline content.
pub type Cell = Vec<Inline>;

/// One table row: a cell per column.
pub type Row = Vec<Cell>;

/// A block-level markdown element.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// A paragraph.
    Paragraph(Vec<Inline>),
    /// A heading.
    Heading {
        /// Heading level (1–6).
        level: u8,
        /// Heading content.
        content: Vec<Inline>,
    },
    /// A fenced or indented code block.
    CodeBlock {
        /// The language tag, if any.
        lang: Option<String>,
        /// The raw code.
        code: String,
    },
    /// A list.
    List {
        /// The first ordinal of an ordered list (`1` for `1.`), or `None` when the list
        /// is unordered.
        start: Option<u64>,
        /// The items, top to bottom.
        items: Vec<ListItem>,
    },
    /// A block quote.
    Quote(Vec<Block>),
    /// A GitHub-flavored table.
    Table {
        /// The header row.
        header: Row,
        /// Per-column alignment. A column past the end of this vector is
        /// [`Alignment::None`].
        alignments: Vec<Alignment>,
        /// The body rows, top to bottom. A row may be short; missing cells are empty.
        rows: Vec<Row>,
    },
    /// A thematic break (horizontal rule).
    Rule,
    /// Blocks an HTML container aligns (`<p align="center">`, `<center>`): each line
    /// is padded to sit centered or right-aligned within the width. Code and tables
    /// keep their own layout.
    Aligned {
        /// The declared alignment.
        align: Alignment,
        /// The aligned content.
        blocks: Vec<Block>,
    },
}

/// A parsed markdown document: an ordered sequence of blocks.
#[derive(Clone, Debug, Default)]
pub struct MarkdownDocument {
    /// The top-level blocks.
    pub blocks: Vec<Block>,
    /// The 0-based source line each top-level block begins on, parallel to `blocks`.
    /// Private so the two vectors cannot drift out of step; read it through
    /// [`block_line`](Self::block_line).
    block_lines: Vec<usize>,
}

impl MarkdownDocument {
    /// Soft-wrap the document to `width` terminal columns.
    ///
    /// With the `highlight` feature, a fenced code block whose info string names a
    /// compiled-in grammar is syntax-highlighted; otherwise it renders as raw markup.
    #[must_use]
    pub fn wrap(&self, width: u16) -> WrappedDocument {
        wrap::wrap(self, width)
    }

    /// As [`wrap`](Self::wrap), but a paragraph holding only images gives every image
    /// `sizer` sizes rows of its own, as [`ImageSlice`]s on the lines it reserves.
    ///
    /// An image is fitted to the width at an assumed 8×16-pixel cell, honouring its
    /// HTML `width`/`height`, never upscaled and never taller than 20 lines. Images
    /// among text, in a table, or left unsized render as chips.
    #[must_use]
    pub fn wrap_with(&self, width: u16, sizer: &dyn ImageSizer) -> WrappedDocument {
        wrap::wrap_with(self, width, sizer)
    }

    /// The 0-based source line the top-level block at `index` begins on, or `None` when
    /// `index` is out of range.
    #[must_use]
    pub fn block_line(&self, index: usize) -> Option<usize> {
        self.block_lines.get(index).copied()
    }
}

/// Parse markdown `source` into a [`MarkdownDocument`].
#[must_use]
pub fn parse(source: &str) -> MarkdownDocument {
    parse::parse(source)
}

pub use table::format_tables;
pub use table::table_line_ranges;

/// The checkbox a [`ListItem::task`] renders as, trailing space included.
pub(crate) fn task_marker(checked: bool) -> &'static str {
    if checked { "☑ " } else { "☐ " }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_model_constructs() {
        let doc = MarkdownDocument {
            blocks: vec![Block::Heading {
                level: 1,
                content: vec![Inline::Text("Title".to_owned())],
            }],
            block_lines: vec![0],
        };
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.block_line(0), Some(0));
        assert_eq!(doc.block_line(1), None);
        assert_eq!(Block::Rule, Block::Rule);
    }
}
