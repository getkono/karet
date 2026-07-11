//! The neutral, presentation-free document model a DOCX parses into.
//!
//! The model is deliberately small — it captures only what karet needs to *display*
//! a Word document: block structure (paragraphs, headings, lists, tables) and the
//! handful of character runs markdown can express (bold / italic / underline /
//! strikethrough, plus hyperlink targets). It carries no fonts, colours, sizes,
//! sections, or images (an image is flattened to a `[image]` placeholder span). A
//! future higher-fidelity renderer can consume this model directly instead of the
//! markdown projection in [`crate::to_markdown`].

/// A parsed DOCX document: an ordered sequence of block-level elements.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// The document body, in reading order.
    pub blocks: Vec<Block>,
}

/// A block-level element of a [`Document`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Block {
    /// A paragraph: a style plus the inline [`Span`]s that make up its text.
    Paragraph {
        /// The paragraph's role (normal, heading, or list item).
        style: ParaStyle,
        /// The inline runs, in order.
        spans: Vec<Span>,
    },
    /// A table: rows of cells, each cell a run of inline [`Span`]s.
    ///
    /// Indexed `rows[row][cell]` yielding that cell's spans. Rows are not required
    /// to be the same length (a malformed table is kept as-is rather than padded).
    Table {
        /// The table rows, top to bottom; each row is a list of cells.
        rows: Vec<Vec<Vec<Span>>>,
    },
}

/// The role of a [`Block::Paragraph`], deciding how it renders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParaStyle {
    /// Ordinary body text.
    Normal,
    /// A heading at the given level (`1..=6`); `Title` maps to level 1.
    Heading(u8),
    /// A list item at a nesting `level` (0-based), ordered (numbered) or bulleted.
    ListItem {
        /// Whether the list is ordered (numbered) rather than bulleted.
        ordered: bool,
        /// The 0-based nesting depth (`w:ilvl`).
        level: u8,
    },
}

/// An inline run of text with its character formatting.
///
/// A DOCX `w:r` run maps to one (or, across `w:br`/`w:tab`, a few) of these. The
/// four boolean toggles are exactly the emphases markdown can express; `link`
/// carries a resolved hyperlink target when the run sits inside a `w:hyperlink`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    /// The run's text (a `w:br` contributes `"\n"`, a `w:tab` contributes `"\t"`,
    /// and an image `w:drawing`/`w:pict` contributes the literal `"[image]"`).
    pub text: String,
    /// Whether the run is bold (`w:b`).
    pub bold: bool,
    /// Whether the run is italic (`w:i`).
    pub italic: bool,
    /// Whether the run is underlined (`w:u`). Markdown has no underline, so this is
    /// preserved in the model but rendered as plain text by [`crate::to_markdown`].
    pub underline: bool,
    /// Whether the run is struck through (`w:strike`).
    pub strike: bool,
    /// A resolved hyperlink target, when the run is inside a `w:hyperlink`.
    pub link: Option<String>,
}

impl Span {
    /// A plain, unformatted span carrying `text`.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}
