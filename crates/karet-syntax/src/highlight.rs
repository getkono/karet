//! The tree-sitter highlighter: run a grammar's highlights query and flatten the
//! (overlapping) captures into ordered, non-overlapping [`HighlightSpan`]s.

use std::collections::HashMap;

use karet_core::BytePos;
use karet_core::Span;
use karet_core::TokenId;
use karet_treesitter::LanguageId;
use karet_treesitter::LayeredTree;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;

use crate::HighlightSpan;
use crate::Highlights;
use crate::SyntaxError;
use crate::map::map_capture;

/// Computes [`Highlights`] from a [`SyntaxTree`] using a grammar's highlights query.
pub struct Highlighter {
    query: Query,
    /// Capture-index → (token, specificity), precomputed from the query's capture names.
    lut: Vec<Option<(TokenId, u8)>>,
}

/// How specific a capture name is: the number of dot-separated segments.
///
/// Grammars refine a general capture with a narrower one over the *same* node —
/// tree-sitter-rust captures `(line_comment) @comment` and then
/// `(line_comment (doc_comment)) @comment.documentation`. Ranking by specificity lets
/// the narrower name win regardless of the order the grammar happens to declare them.
fn specificity(capture_name: &str) -> u8 {
    let segments = capture_name.split('.').count();
    u8::try_from(segments).unwrap_or(u8::MAX)
}

impl Highlighter {
    /// Build a highlighter for `lang`.
    ///
    /// # Errors
    /// Returns [`SyntaxError::UnsupportedLanguage`] if no grammar/highlights query is
    /// compiled in for `lang`, or [`SyntaxError::Query`] if the query fails to compile.
    pub fn new(lang: LanguageId) -> Result<Self, SyntaxError> {
        let source =
            karet_treesitter::highlights_query(lang).ok_or(SyntaxError::UnsupportedLanguage)?;
        let query = Query::compile(lang, source).map_err(|e| SyntaxError::Query(e.to_string()))?;
        let lut: Vec<Option<(TokenId, u8)>> = query
            .capture_names()
            .iter()
            .map(|n| map_capture(n).map(|token| (token, specificity(n))))
            .collect();
        Ok(Self { query, lut })
    }

    /// Highlight `text` using its parsed `tree`.
    ///
    /// `tree` must have been parsed (with `karet-treesitter`) for the same language
    /// this highlighter was built for, from the same `text`.
    ///
    /// # Errors
    /// This currently never errors; the result is `Ok` for API symmetry with future
    /// query-time failures.
    pub fn highlight(&self, tree: &SyntaxTree, text: &str) -> Result<Highlights, SyntaxError> {
        let mut caps: Vec<Cap> = Vec::new();
        self.collect(tree, text, 0, &mut caps);
        Ok(Highlights::from_sorted_spans(flatten(caps)))
    }

    /// Append this grammar's captures over `tree` to `out`, stamped with `layer`.
    fn collect(&self, tree: &SyntaxTree, text: &str, layer: u32, out: &mut Vec<Cap>) {
        for rc in tree.captures(&self.query, text) {
            if let Some(Some((token, specificity))) = self.lut.get(rc.capture as usize) {
                out.push(Cap {
                    start: rc.span.start.0,
                    end: rc.span.end.0,
                    token: *token,
                    layer,
                    specificity: *specificity,
                    key: rc.capture,
                });
            }
        }
    }
}

/// Highlights a [`LayeredTree`], merging the captures of every injected language into
/// one span list.
///
/// Layers share the document's byte coordinates, so merging is just concatenation plus
/// the usual innermost-wins flatten: a fenced block's rust `keyword` span is smaller
/// than markdown's `text.literal` span over the whole fence, so the keyword paints over
/// it. Where the embedded grammar says nothing, the parent's color shows through.
#[derive(Default)]
pub struct LayeredHighlighter {
    /// One highlighter per language; `None` records "no grammar/query compiled in", so
    /// a missing grammar is not retried on every keystroke.
    by_lang: HashMap<LanguageId, Option<Highlighter>>,
}

impl LayeredHighlighter {
    /// Create an empty layered highlighter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Highlight every layer of `tree` against `text` and merge the result.
    ///
    /// A layer whose language has no highlights query simply contributes nothing, so an
    /// exotic embedded language degrades to plain text rather than failing the document.
    pub fn highlight(&mut self, tree: &LayeredTree, text: &str) -> Highlights {
        let mut caps: Vec<Cap> = Vec::new();
        for (index, layer) in tree.layers().enumerate() {
            let lang = layer.language();
            let entry = self
                .by_lang
                .entry(lang)
                .or_insert_with(|| Highlighter::new(lang).ok());
            if let Some(highlighter) = entry {
                // `layers()` is ordered shallowest-first, so the index doubles as depth
                // for tie-breaking.
                let layer_ord = u32::try_from(index).unwrap_or(u32::MAX);
                highlighter.collect(layer, text, layer_ord, &mut caps);
            }
        }
        Highlights::from_sorted_spans(flatten(caps))
    }
}

/// An intermediate capture: a byte range, its token, and the three keys that break an
/// exact same-range tie — producing layer, capture-name specificity, capture index.
struct Cap {
    start: usize,
    end: usize,
    token: TokenId,
    /// Index of the producing layer; deeper (larger) layers are more specific.
    layer: u32,
    /// Dot-segment count of the capture name; see [`specificity`].
    specificity: u8,
    key: u32,
}

/// Flatten overlapping captures into ordered, non-overlapping spans where the
/// innermost (smallest) capture wins for its sub-range — so e.g. a `string.escape`
/// paints over the enclosing `string`. Adjacent same-token spans are coalesced.
fn flatten(mut caps: Vec<Cap>) -> Vec<HighlightSpan> {
    caps.retain(|c| c.end > c.start);
    // At equal start, the longer (outer) capture opens first. For an identical range the
    // most specific reading wins, in order: the deeper layer (an injected language's
    // view of a node beats its host's), then the narrower capture name
    // (`comment.documentation` beats `comment`), then the lower capture index as a
    // deterministic last resort.
    caps.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| b.end.cmp(&a.end))
            .then_with(|| b.layer.cmp(&a.layer))
            .then_with(|| b.specificity.cmp(&a.specificity))
            .then_with(|| a.key.cmp(&b.key))
    });

    let mut out: Vec<HighlightSpan> = Vec::new();
    let mut stack: Vec<Cap> = Vec::new();
    let mut i = 0usize;
    let mut pos = match caps.first() {
        Some(c) => c.start,
        None => return out,
    };

    while i < caps.len() || !stack.is_empty() {
        let next_start = caps.get(i).map(|c| c.start);
        let top_end = stack.last().map(|c| c.end);
        let boundary = match (next_start, top_end) {
            (Some(s), Some(e)) => s.min(e),
            (Some(s), None) => s,
            (None, Some(e)) => e,
            (None, None) => break,
        };

        if boundary > pos {
            if let Some(top) = stack.last() {
                emit(&mut out, pos, boundary, top.token);
            }
            pos = boundary;
        }

        // Close every capture that ends at or before the cursor.
        while stack.last().is_some_and(|c| c.end <= pos) {
            stack.pop();
        }
        // Open every capture starting exactly here. Skip one with the same range as
        // the current innermost (its lower-key sibling already claimed the range).
        while let Some(c) = caps.get(i) {
            if c.start != pos {
                break;
            }
            let dup = stack
                .last()
                .is_some_and(|t| t.start == c.start && t.end == c.end);
            if !dup {
                stack.push(Cap {
                    start: c.start,
                    end: c.end,
                    token: c.token,
                    layer: c.layer,
                    specificity: c.specificity,
                    key: c.key,
                });
            }
            i += 1;
        }
    }

    out
}

/// Append `[start, end)` with `token`, coalescing with an adjacent same-token span.
fn emit(out: &mut Vec<HighlightSpan>, start: usize, end: usize, token: TokenId) {
    if let Some(last) = out.last_mut()
        && last.token == token
        && last.span.end.0 == start
    {
        last.span.end = BytePos(end);
        return;
    }
    out.push(HighlightSpan {
        span: Span {
            start: BytePos(start),
            end: BytePos(end),
        },
        token,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(start: usize, end: usize, token: TokenId, key: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer: 0,
            specificity: 1,
            key,
        }
    }

    /// A capture from injected layer `layer`.
    fn cap_at(start: usize, end: usize, token: TokenId, layer: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer,
            specificity: 1,
            key: 0,
        }
    }

    /// A capture whose name has `specificity` dot-separated segments.
    fn cap_spec(start: usize, end: usize, token: TokenId, specificity: u8, key: u32) -> Cap {
        Cap {
            start,
            end,
            token,
            layer: 0,
            specificity,
            key,
        }
    }

    fn spans(caps: Vec<Cap>) -> Vec<(usize, usize, u16)> {
        flatten(caps)
            .into_iter()
            .map(|s| (s.span.start.0, s.span.end.0, s.token.0))
            .collect()
    }

    #[test]
    fn disjoint_captures_preserved() {
        let out = spans(vec![
            cap(0, 3, TokenId::KEYWORD, 0),
            cap(4, 9, TokenId::FUNCTION, 1),
        ]);
        assert_eq!(
            out,
            vec![(0, 3, TokenId::KEYWORD.0), (4, 9, TokenId::FUNCTION.0)]
        );
    }

    #[test]
    fn inner_capture_overrides_outer() {
        // A string [0,10) with an escape [3,5) inside it.
        let out = spans(vec![
            cap(0, 10, TokenId::STRING, 0),
            cap(3, 5, TokenId::new(12), 1), // StringEscape id
        ]);
        assert_eq!(
            out,
            vec![
                (0, 3, TokenId::STRING.0),
                (3, 5, 12),
                (5, 10, TokenId::STRING.0),
            ]
        );
    }

    #[test]
    fn same_range_lower_key_wins() {
        let out = spans(vec![
            cap(0, 4, TokenId::FUNCTION, 1),
            cap(0, 4, TokenId::VARIABLE, 0),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::VARIABLE.0)]);
    }

    #[test]
    fn adjacent_same_token_coalesced() {
        let out = spans(vec![
            cap(0, 3, TokenId::KEYWORD, 0),
            cap(3, 6, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(out, vec![(0, 6, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn zero_length_captures_dropped() {
        assert!(spans(vec![cap(5, 5, TokenId::KEYWORD, 0)]).is_empty());
    }

    #[test]
    fn specificity_counts_dot_segments() {
        assert_eq!(specificity("comment"), 1);
        assert_eq!(specificity("comment.documentation"), 2);
        assert_eq!(specificity("keyword.control.import"), 3);
    }

    #[test]
    fn narrower_capture_name_wins_an_exact_range_tie() {
        // tree-sitter-rust captures `(line_comment) @comment` *before*
        // `(line_comment (doc_comment)) @comment.documentation`, both on the same node.
        // The narrower name must win despite its higher capture index.
        let doc = karet_core::StandardToken::CommentDoc.id();
        let out = spans(vec![
            cap_spec(0, 8, TokenId::COMMENT, 1, 0),
            cap_spec(0, 8, doc, 2, 1),
        ]);
        assert_eq!(out, vec![(0, 8, doc.0)]);
    }

    #[test]
    fn layer_outranks_specificity() {
        // An injected language's plain `keyword` beats the host's narrower name: the
        // deeper layer is the more authoritative reading of that text.
        let out = spans(vec![
            cap_spec(0, 4, TokenId::COMMENT, 3, 0),
            Cap {
                start: 0,
                end: 4,
                token: TokenId::KEYWORD,
                layer: 1,
                specificity: 1,
                key: 9,
            },
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn deeper_layer_wins_an_exact_range_tie() {
        // The host grammar and an injected one both claim [0,4): the injected layer's
        // reading of that text is the more specific one.
        let out = spans(vec![
            cap_at(0, 4, TokenId::STRING, 0),
            cap_at(0, 4, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
        // Order of submission must not matter.
        let out = spans(vec![
            cap_at(0, 4, TokenId::KEYWORD, 1),
            cap_at(0, 4, TokenId::STRING, 0),
        ]);
        assert_eq!(out, vec![(0, 4, TokenId::KEYWORD.0)]);
    }

    #[test]
    fn embedded_spans_paint_over_the_hosts_coarse_span() {
        // markdown tags the whole fence `text.literal`; the embedded rust layer tags a
        // keyword inside it. The keyword wins its sub-range, the rest stays raw.
        let raw = karet_core::StandardToken::MarkupRaw.id();
        let out = spans(vec![
            cap_at(0, 20, raw, 0),
            cap_at(5, 7, TokenId::KEYWORD, 1),
        ]);
        assert_eq!(
            out,
            vec![(0, 5, raw.0), (5, 7, TokenId::KEYWORD.0), (7, 20, raw.0)]
        );
    }
}
