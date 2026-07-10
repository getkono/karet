//! `karet-markdown` — a markdown rendering model for karet (and LSP hover docs).
//!
//! Parses markdown (CommonMark plus GitHub tables and task lists) into a block/inline
//! render model decoupled from any renderer. Enable `view` for a ratatui renderer, and
//! `highlight` to syntax-highlight code fences via `karet-syntax`.
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

mod parse;
mod wrap;

#[cfg(feature = "highlight")]
mod highlight;

#[cfg(feature = "view")]
pub mod view;

pub use wrap::Anchor;
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
    /// A hyperlink.
    Link {
        /// The link text.
        text: String,
        /// The link target.
        href: String,
    },
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
