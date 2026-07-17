//! `karet-syntax` — tree-sitter-powered syntactic analysis for karet editors.
//!
//! Produces *data*, not rendering: highlight spans tagged with semantic
//! [`TokenId`]s. Consumers apply a theme (`karet-theme`) and render. This is the
//! crate behind the standalone "highlight a code snippet" use case. Highlighting
//! runs a grammar's query through `karet-treesitter`'s single parse host.
//!
//! [`Highlighter`] highlights one language. [`LayeredHighlighter`] highlights a
//! `LayeredTree` — a document plus its injected languages — so a markdown code fence
//! is coloured as the language it names and a Rust doc comment as the markdown it is.
//! [`mark_semantic_comments`] is an optional post-pass that retints codetag comment
//! blocks (`TODO: …` and friends) so they stand out from ordinary comments.
//!
//! Fold regions, bracket pairs and structural selection are reserved (the public
//! joints are defined; their tree-walking is filled in with the editor).

use std::collections::BTreeMap;

use karet_core::BytePos;
use karet_core::Span;
use karet_core::TokenId;
use karet_treesitter::SyntaxTree;

mod blocks;
mod highlight;
mod map;
mod semantic;

pub use blocks::SemanticBlock;
pub use blocks::SemanticBlocker;
pub use blocks::SemanticBlocks;
pub use highlight::Highlighter;
pub use highlight::LayeredHighlighter;
pub use semantic::SemanticCommentConfig;
pub use semantic::mark_semantic_comments;

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

    /// Shift these spans to stay aligned with a buffer edited in `[start, old_end)` →
    /// `[start, new_end)`.
    ///
    /// When re-highlighting is asynchronous the buffer changes before fresh spans
    /// arrive. Rendering the old spans verbatim would smear color across the shifted
    /// text; translating them keeps everything after the edit correctly aligned for the
    /// frame or two before the highlighter answers.
    ///
    /// Spans wholly before the edit are untouched, spans wholly after are shifted, and
    /// a span the edit actually cut through is dropped — its extent is no longer known,
    /// so the affected text renders unhighlighted rather than wrong.
    #[must_use]
    pub fn translate(&self, start: BytePos, old_end: BytePos, new_end: BytePos) -> Self {
        let spans = self
            .spans
            .iter()
            .filter_map(|s| {
                if s.span.end.0 <= start.0 {
                    return Some(*s);
                }
                if s.span.start.0 >= old_end.0 {
                    return Some(HighlightSpan {
                        span: Span {
                            start: BytePos(shift_pos(s.span.start.0, old_end.0, new_end.0)),
                            end: BytePos(shift_pos(s.span.end.0, old_end.0, new_end.0)),
                        },
                        token: s.token,
                    });
                }
                // The edit cut through this span.
                None
            })
            .collect();
        Self { spans }
    }
}

/// Move `pos` (which lies at or after `old_end`) by the edit's signed length delta.
fn shift_pos(pos: usize, old_end: usize, new_end: usize) -> usize {
    if new_end >= old_end {
        pos + (new_end - old_end)
    } else {
        pos.saturating_sub(old_end - new_end)
    }
}

/// A foldable region as an inclusive line range `[start, end]` (0-based lines). The
/// `start` line is the header that stays visible when collapsed; lines
/// `start + 1 ..= end` are the ones that hide. Line ranges (not byte spans) because
/// folding is inherently a line operation for every consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    /// The 0-based header line (stays visible when collapsed).
    pub start: u32,
    /// The 0-based last line of the region, inclusive.
    pub end: u32,
}

/// The foldable regions of a buffer, in document order (outermost first), with at
/// most one region per start line.
#[derive(Clone, Debug, Default)]
pub struct FoldRegions {
    regions: Vec<FoldRegion>,
}

impl FoldRegions {
    /// The fold regions, outermost first.
    #[must_use]
    pub fn regions(&self) -> &[FoldRegion] {
        &self.regions
    }
}

/// Compute fold regions from a parsed tree.
///
/// The algorithm is deliberately **language-agnostic**: it folds every named node
/// that spans more than one line, keeping the outermost node per start line so a
/// construct and its wrapper (or two constructs sharing a line, like `} else {`)
/// don't stack duplicate folds. This works on any tree-sitter grammar with no
/// per-language configuration.
///
/// The trade-off is granularity: because it folds *any* multi-line node, it also
/// creates folds for coarse nodes a human might not (e.g. a multi-line call
/// argument list). Polished, per-construct folding — fold a function's body but not
/// its signature line, skip trivial expressions — is inherently **not**
/// language-agnostic: it requires per-grammar `folds.scm` queries (a `@fold`
/// capture per language). That refinement is deferred; this baseline gives usable
/// folds everywhere today.
#[must_use]
pub fn fold(tree: &SyntaxTree) -> FoldRegions {
    // One fold per start line — the outermost node beginning there, i.e. the one that
    // ends on the latest line.
    let mut by_start: BTreeMap<u32, u32> = BTreeMap::new();
    for node in tree.multiline_named_spans() {
        by_start
            .entry(node.start_row)
            .and_modify(|end| *end = (*end).max(node.end_row))
            .or_insert(node.end_row);
    }
    // BTreeMap iterates by ascending start line, i.e. document order (outermost
    // first), which is exactly the contract of `FoldRegions::regions`.
    FoldRegions {
        regions: by_start
            .into_iter()
            .map(|(start, end)| FoldRegion { start, end })
            .collect(),
    }
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
    use karet_core::StandardToken;

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
    fn translate_shifts_spans_after_an_insertion() {
        let hl = Highlights::from_sorted_spans(vec![
            HighlightSpan {
                span: span(0, 3),
                token: TokenId::KEYWORD,
            },
            HighlightSpan {
                span: span(10, 14),
                token: TokenId::FUNCTION,
            },
        ]);
        // Insert 2 bytes at 5: before is untouched, after slides right.
        let out = hl.translate(BytePos(5), BytePos(5), BytePos(7));
        assert_eq!(out.all()[0].span, span(0, 3));
        assert_eq!(out.all()[1].span, span(12, 16));
    }

    #[test]
    fn translate_shifts_spans_after_a_deletion() {
        let hl = Highlights::from_sorted_spans(vec![HighlightSpan {
            span: span(10, 14),
            token: TokenId::FUNCTION,
        }]);
        // Delete bytes [5,8): the span slides left by 3.
        let out = hl.translate(BytePos(5), BytePos(8), BytePos(5));
        assert_eq!(out.all()[0].span, span(7, 11));
    }

    #[test]
    fn translate_drops_a_span_the_edit_cut_through() {
        let hl = Highlights::from_sorted_spans(vec![
            HighlightSpan {
                span: span(0, 3),
                token: TokenId::KEYWORD,
            },
            HighlightSpan {
                span: span(4, 9),
                token: TokenId::STRING,
            },
        ]);
        // Typing inside the string: its extent is unknown until the reparse lands, so it
        // renders unhighlighted rather than wrong.
        let out = hl.translate(BytePos(6), BytePos(6), BytePos(7));
        assert_eq!(out.all().len(), 1);
        assert_eq!(out.all()[0].token, TokenId::KEYWORD);
    }

    #[test]
    fn translate_preserves_sorted_non_overlapping_order() {
        let hl = Highlights::from_sorted_spans(vec![
            HighlightSpan {
                span: span(0, 3),
                token: TokenId::KEYWORD,
            },
            HighlightSpan {
                span: span(5, 8),
                token: TokenId::STRING,
            },
            HighlightSpan {
                span: span(9, 12),
                token: TokenId::NUMBER,
            },
        ]);
        let out = hl.translate(BytePos(4), BytePos(4), BytePos(6));
        assert!(
            out.all()
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0)
        );
        // `spans_in` relies on that invariant, so it must still find the shifted span.
        assert_eq!(out.spans_in(span(7, 11)).len(), 1);
    }

    #[test]
    fn fold_finds_multiline_regions() -> Result<(), Box<dyn std::error::Error>> {
        use std::path::Path;

        use karet_treesitter::ParserPool;
        use karet_treesitter::SyntaxTree;
        use karet_treesitter::language_id_from_path;

        let Some(lang) = language_id_from_path(Path::new("x.rs")) else {
            return Ok(()); // rust grammar not compiled in; nothing to test
        };
        // A function whose body spans several lines: the fn item (and its block) are
        // multi-line and must yield a fold; the one-line `let` inside must not.
        let src = "fn main() {\n    let x = 42;\n    let y = x + 1;\n}\n";
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, src)?;
        let folds = fold(&tree);
        let regions = folds.regions();
        assert!(!regions.is_empty(), "expected at least one fold region");
        // Every region spans more than one line, and the function's body (starting on
        // line 0) folds down to (at least) the closing brace on line 3.
        for r in regions {
            assert!(r.end > r.start, "region {r:?} does not span multiple lines");
        }
        assert!(
            regions.iter().any(|r| r.start == 0 && r.end >= 3),
            "expected a fold covering the whole function body"
        );
        // Regions are unique per start line (outermost kept) and in document order.
        let starts: Vec<u32> = regions.iter().map(|r| r.start).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted, "regions must be in ascending start order");
        assert_eq!(
            starts.len(),
            sorted
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            "at most one region per start line"
        );
        Ok(())
    }

    #[test]
    fn fold_is_empty_for_single_line_source() -> Result<(), Box<dyn std::error::Error>> {
        use std::path::Path;

        use karet_treesitter::ParserPool;
        use karet_treesitter::SyntaxTree;
        use karet_treesitter::language_id_from_path;

        let Some(lang) = language_id_from_path(Path::new("x.rs")) else {
            return Ok(());
        };
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, "let x = 1;")?;
        assert!(fold(&tree).regions().is_empty());
        Ok(())
    }

    /// The token covering `needle`'s first byte in `text`, per `highlights`.
    fn token_of(highlights: &Highlights, text: &str, needle: &str) -> Option<TokenId> {
        let at = text.find(needle)?;
        highlights
            .all()
            .iter()
            .find(|s| s.span.start.0 <= at && at < s.span.end.0)
            .map(|s| s.token)
    }

    #[test]
    fn markdown_code_fence_is_highlighted_as_its_language() -> Result<(), Box<dyn std::error::Error>>
    {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        let src = "# Title\n\n```rust\nfn main() { let x = 42; }\n```\n";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        // The embedded rust wins over markdown's coarse `text.literal` fence span.
        assert_eq!(
            token_of(&highlights, src, "fn main"),
            Some(TokenId::KEYWORD)
        );
        assert_eq!(token_of(&highlights, src, "42"), Some(TokenId::CONSTANT));
        // Markdown's own structure keeps its markup colors.
        assert_eq!(
            token_of(&highlights, src, "Title"),
            Some(StandardToken::MarkupHeading.id())
        );
        // Spans remain sorted and non-overlapping after the cross-layer merge.
        assert!(
            highlights
                .all()
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0)
        );
        Ok(())
    }

    #[test]
    fn markdown_inline_emphasis_is_highlighted() -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        // Emphasis and links live in the *inline* grammar, reachable only by injection.
        let src = "Some *slanted* and **heavy** text, plus <http://example.com>.\n";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(md, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        assert_eq!(
            token_of(&highlights, src, "slanted"),
            Some(StandardToken::MarkupItalic.id())
        );
        assert_eq!(
            token_of(&highlights, src, "heavy"),
            Some(StandardToken::MarkupBold.id())
        );
        assert_eq!(
            token_of(&highlights, src, "http://example.com"),
            Some(StandardToken::MarkupLink.id())
        );
        Ok(())
    }

    #[test]
    fn rust_doctest_in_a_doc_comment_is_highlighted_as_rust()
    -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(rust) = language_id_from_injection_name("rust") else {
            return Ok(());
        };
        let src = "\
/// Adds one to `x`.
///
/// ```rust
/// let y = 1 + 1;
/// ```
pub fn add_one() {}
";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, src)?;
        let highlights = LayeredHighlighter::new().highlight(&tree, src);

        // The doctest body is real rust: `let` is a keyword, not comment text.
        assert_eq!(token_of(&highlights, src, "let y"), Some(TokenId::KEYWORD));
        // The doc comment's prose is a doc comment, distinct from a plain comment.
        assert_eq!(
            token_of(&highlights, src, "Adds one"),
            Some(StandardToken::CommentDoc.id())
        );
        // And the outer function is still highlighted normally.
        assert_eq!(token_of(&highlights, src, "pub"), Some(TokenId::KEYWORD));
        Ok(())
    }

    #[test]
    fn layered_highlight_tracks_a_live_edit() -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::Edit;
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(md) = language_id_from_injection_name("markdown") else {
            return Ok(());
        };
        let old = "text\n";
        let new = "text\n\n```rust\nfn f() {}\n```\n";

        let mut parser = LayeredParser::new();
        let mut highlighter = LayeredHighlighter::new();
        let mut tree = parser.parse(md, old)?;
        assert!(
            token_of(&highlighter.highlight(&tree, old), old, "text") != Some(TokenId::KEYWORD)
        );

        parser.reparse(
            &mut tree,
            &[Edit {
                start_byte: old.len(),
                old_end_byte: old.len(),
                new_end_byte: new.len(),
                start_point: (1, 0),
                old_end_point: (1, 0),
                new_end_point: (5, 0),
            }],
            new,
        )?;
        let after = highlighter.highlight(&tree, new);
        // Typing the fence lights up the embedded language immediately.
        assert_eq!(token_of(&after, new, "fn f"), Some(TokenId::KEYWORD));
        // ...and matches a cold parse of the same text.
        let cold = LayeredHighlighter::new().highlight(&parser.parse(md, new)?, new);
        assert_eq!(after.all(), cold.all());
        Ok(())
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
