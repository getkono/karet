//! `karet-syntax` — tree-sitter-powered syntactic analysis for karet editors.
//!
//! Produces *data*, not rendering: highlight spans tagged with semantic
//! [`TokenId`]s. Consumers apply a theme (`karet-theme`) and render. This is the
//! crate behind the standalone "highlight a code snippet" use case. Highlighting
//! runs a grammar's query through `karet-treesitter`'s single parse host.
//!
//! Fold regions, bracket pairs and structural selection are reserved (the public
//! joints are defined; their tree-walking is filled in with the editor).

use karet_core::Span;
use karet_core::TokenId;
use karet_treesitter::SyntaxTree;

mod highlight;
mod map;

pub use highlight::Highlighter;

/// Errors produced by syntactic analysis.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyntaxError {
    /// No highlight configuration exists (is compiled in) for the language.
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
    /// All highlight spans, in document (byte) order.
    #[must_use]
    pub fn all(&self) -> &[HighlightSpan] {
        &self.spans
    }

    /// The highlight spans overlapping `range`, in order.
    #[must_use]
    pub fn spans_in(&self, range: Span) -> &[HighlightSpan] {
        // `spans` is sorted by start and non-overlapping, so both predicates are
        // monotonic and `partition_point` gives the overlapping window.
        let start = self
            .spans
            .partition_point(|s| s.span.end.0 <= range.start.0);
        let end = self.spans.partition_point(|s| s.span.start.0 < range.end.0);
        &self.spans[start..end.max(start)]
    }

    /// Wrap an already-sorted, non-overlapping span list (from the highlighter).
    pub(crate) fn from_sorted_spans(spans: Vec<HighlightSpan>) -> Self {
        Self { spans }
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

/// Compute fold regions from a parsed tree. (Reserved; not yet implemented.)
#[must_use]
pub fn fold(tree: &SyntaxTree) -> FoldRegions {
    let _ = tree;
    FoldRegions::default()
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

/// Compute matched bracket pairs from a parsed tree. (Reserved; not yet implemented.)
#[must_use]
pub fn brackets(tree: &SyntaxTree) -> BracketPairs {
    let _ = tree;
    BracketPairs::default()
}

/// The direction of a structural-selection change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandDir {
    /// Grow the selection to the enclosing node.
    Expand,
    /// Shrink to the previously-expanded child.
    Shrink,
}

/// Expand or shrink the selection `at` to the appropriate syntax node. (Reserved;
/// not yet implemented — returns `at` unchanged.)
#[must_use]
pub fn structural_selection(tree: &SyntaxTree, at: Span, dir: ExpandDir) -> Span {
    let _ = (tree, dir);
    at
}

#[cfg(test)]
mod tests {
    use karet_core::BytePos;

    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            SyntaxError::UnsupportedLanguage.to_string(),
            "unsupported language"
        );
    }

    #[test]
    fn spans_in_returns_overlapping_window() {
        let hl = Highlights::from_sorted_spans(vec![
            HighlightSpan {
                span: span(0, 3),
                token: TokenId::KEYWORD,
            },
            HighlightSpan {
                span: span(5, 9),
                token: TokenId::FUNCTION,
            },
            HighlightSpan {
                span: span(12, 14),
                token: TokenId::NUMBER,
            },
        ]);
        // Overlaps the middle span only.
        let got = hl.spans_in(span(6, 8));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].token, TokenId::FUNCTION);
        // Range covering the first two spans.
        assert_eq!(hl.spans_in(span(0, 10)).len(), 2);
        // Gap between spans yields nothing.
        assert!(hl.spans_in(span(9, 12)).is_empty());
    }

    #[test]
    fn highlights_rust_code_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
        use std::path::Path;

        use karet_treesitter::ParserPool;
        use karet_treesitter::SyntaxTree;
        use karet_treesitter::language_id_from_path;

        let Some(lang) = language_id_from_path(Path::new("x.rs")) else {
            return Ok(()); // rust grammar not compiled into this build; nothing to test
        };
        let src = "fn main() { let x = 42; }";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src)?;
        let highlights = Highlighter::new(lang)?.highlight(&tree, src)?;
        let spans = highlights.all();

        assert!(!spans.is_empty());
        // `fn` is a keyword starting at byte 0.
        assert!(
            spans
                .iter()
                .any(|s| s.token == TokenId::KEYWORD && s.span.start.0 == 0)
        );
        // `42` is a numeric literal — Rust's grammar tags it `constant.builtin`,
        // which maps to CONSTANT via the dotted fallback.
        assert!(spans.iter().any(|s| s.token == TokenId::CONSTANT));
        // Output is sorted and non-overlapping.
        assert!(
            spans
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0)
        );
        Ok(())
    }
}
