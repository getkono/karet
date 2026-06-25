//! The tree-sitter highlighter: run a grammar's highlights query and flatten the
//! (overlapping) captures into ordered, non-overlapping [`HighlightSpan`]s.

use karet_core::{BytePos, Span, TokenId};
use karet_treesitter::{LanguageId, Query, SyntaxTree};

use crate::map::map_capture;
use crate::{HighlightSpan, Highlights, SyntaxError};

/// Computes [`Highlights`] from a [`SyntaxTree`] using a grammar's highlights query.
pub struct Highlighter {
    query: Query,
    /// Capture-index → token, precomputed from the query's capture names.
    lut: Vec<Option<TokenId>>,
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
        let lut: Vec<Option<TokenId>> = query
            .capture_names()
            .iter()
            .map(|n| map_capture(n))
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
        for rc in tree.captures(&self.query, text) {
            if let Some(Some(token)) = self.lut.get(rc.capture as usize) {
                caps.push(Cap {
                    start: rc.span.start.0,
                    end: rc.span.end.0,
                    token: *token,
                    key: rc.capture,
                });
            }
        }
        Ok(Highlights::from_sorted_spans(flatten(caps)))
    }
}

/// An intermediate capture: a byte range, its token, and the query capture index
/// (used only to break exact same-range ties deterministically).
struct Cap {
    start: usize,
    end: usize,
    token: TokenId,
    key: u32,
}

/// Flatten overlapping captures into ordered, non-overlapping spans where the
/// innermost (smallest) capture wins for its sub-range — so e.g. a `string.escape`
/// paints over the enclosing `string`. Adjacent same-token spans are coalesced.
fn flatten(mut caps: Vec<Cap>) -> Vec<HighlightSpan> {
    caps.retain(|c| c.end > c.start);
    // At equal start, the longer (outer) capture opens first; equal range → lower key.
    caps.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| b.end.cmp(&a.end))
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
}
