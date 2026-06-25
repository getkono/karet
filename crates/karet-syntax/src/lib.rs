//! `karet-syntax` — tree-sitter-powered syntactic analysis for karet editors.
//!
//! Produces *data*, not rendering: highlight spans tagged with semantic
//! [`TokenId`]s, fold regions, bracket pairs and structural selection ranges.
//! Consumers apply a theme (`karet-theme`) and render. This is the crate behind
//! the standalone "highlight a code snippet" use case.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! query-running logic is filled in separately.

use karet_core::{Span, TokenId};
use karet_treesitter::{LanguageId, SyntaxTree};

/// Errors produced by syntactic analysis.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyntaxError {
    /// No highlight configuration exists for the language.
    #[error("unsupported language")]
    UnsupportedLanguage,
    /// A highlight/fold query failed.
    #[error("query error: {0}")]
    Query(String),
}

/// A highlighted region: a byte [`Span`] tagged with a semantic [`TokenId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    /// The byte range covered.
    pub span: Span,
    /// The semantic token class.
    pub token: TokenId,
}

/// An ordered, non-overlapping set of [`HighlightSpan`]s for a buffer.
#[derive(Clone, Debug, Default)]
pub struct Highlights {
    spans: Vec<HighlightSpan>,
}

impl Highlights {
    /// All highlight spans, in document order.
    #[must_use]
    pub fn all(&self) -> &[HighlightSpan] {
        &self.spans
    }

    /// The highlight spans overlapping `range`, in order.
    #[must_use]
    pub fn spans_in(&self, range: Span) -> &[HighlightSpan] {
        let _ = range;
        todo!()
    }
}

/// Computes [`Highlights`] from a [`SyntaxTree`] using tree-sitter highlight queries.
pub struct Highlighter {}

impl Highlighter {
    /// Build a highlighter for `lang`.
    ///
    /// # Errors
    /// Returns [`SyntaxError::UnsupportedLanguage`] if no configuration exists.
    pub fn new(lang: LanguageId) -> Result<Self, SyntaxError> {
        let _ = lang;
        todo!()
    }

    /// Highlight `text` using its parsed `tree`.
    ///
    /// # Errors
    /// Returns [`SyntaxError::Query`] if the highlight query fails.
    pub fn highlight(&self, tree: &SyntaxTree, text: &str) -> Result<Highlights, SyntaxError> {
        let _ = (tree, text);
        todo!()
    }
}

/// The foldable regions of a buffer (by byte span).
#[derive(Clone, Debug, Default)]
pub struct FoldRegions {
    regions: Vec<Span>,
}

impl FoldRegions {
    /// The fold regions, outermost first.
    #[must_use]
    pub fn regions(&self) -> &[Span] {
        &self.regions
    }
}

/// Compute fold regions from a parsed tree.
#[must_use]
pub fn fold(tree: &SyntaxTree) -> FoldRegions {
    let _ = tree;
    todo!()
}

/// Matched bracket pairs (open span, close span) within a buffer.
#[derive(Clone, Debug, Default)]
pub struct BracketPairs {
    pairs: Vec<(Span, Span)>,
}

impl BracketPairs {
    /// The bracket pairs.
    #[must_use]
    pub fn pairs(&self) -> &[(Span, Span)] {
        &self.pairs
    }
}

/// Compute matched bracket pairs from a parsed tree.
#[must_use]
pub fn brackets(tree: &SyntaxTree) -> BracketPairs {
    let _ = tree;
    todo!()
}

/// The direction of a structural-selection change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandDir {
    /// Grow the selection to the enclosing node.
    Expand,
    /// Shrink to the previously-expanded child.
    Shrink,
}

/// Expand or shrink the selection `at` to the appropriate syntax node.
#[must_use]
pub fn structural_selection(tree: &SyntaxTree, at: Span, dir: ExpandDir) -> Span {
    let _ = (tree, at, dir);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use karet_core::BytePos;

    #[test]
    fn highlight_span_constructs() {
        let h = HighlightSpan {
            span: Span {
                start: BytePos(0),
                end: BytePos(3),
            },
            token: TokenId::KEYWORD,
        };
        assert_eq!(h.token, TokenId::KEYWORD);
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            SyntaxError::UnsupportedLanguage.to_string(),
            "unsupported language"
        );
    }
}
