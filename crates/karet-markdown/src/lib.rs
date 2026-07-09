//! `karet-markdown` — a markdown rendering model for karet (and LSP hover docs).
//!
//! Parses markdown into a block/inline render model decoupled from any renderer.
//! Enable `view` for a ratatui renderer, and `highlight` to syntax-highlight code
//! fences via `karet-syntax`.
//!
//! Two stages. [`parse`] turns source into a tree of [`Block`]s and [`Inline`]s;
//! [`MarkdownDocument::wrap`] soft-wraps that tree to a column width, producing
//! [`WrappedLine`]s of [`TextSpan`]s tagged with a semantic
//! [`TokenId`](karet_core::TokenId). Nothing here knows about a terminal: a consumer
//! resolves those tokens to colors (and bold/italic) through `karet-theme`.

mod parse;
mod wrap;

#[cfg(feature = "highlight")]
mod highlight;

#[cfg(feature = "view")]
pub mod view;

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
    /// A list, as a sequence of items (each a sequence of blocks).
    List(Vec<Vec<Block>>),
    /// A block quote.
    Quote(Vec<Block>),
    /// A thematic break (horizontal rule).
    Rule,
}

/// A parsed markdown document: an ordered sequence of blocks.
#[derive(Clone, Debug, Default)]
pub struct MarkdownDocument {
    /// The top-level blocks.
    pub blocks: Vec<Block>,
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
}

/// Parse markdown `source` into a [`MarkdownDocument`].
#[must_use]
pub fn parse(source: &str) -> MarkdownDocument {
    parse::parse(source)
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
        };
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(Block::Rule, Block::Rule);
    }
}
