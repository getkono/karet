//! Laying a language server's semantic tokens over the tree-sitter
//! highlights, into the one span list a snapshot carries.

use karet_core::BytePos;
use karet_core::HighlightSpan;
use karet_core::Span;
use karet_core::TokenId;
use karet_syntax::Highlights;

/// Lay `semantic` -- a language server's semantic tokens as byte spans, in
/// order and not overlapping -- over `base`, the tree-sitter highlights, into
/// one span list a renderer paints as it paints any other.
///
/// Tree-sitter stays the authority on what is lexically a comment, a string
/// or markup: a semantic token never recolours such a span, and semantic
/// `comment`, `string` and `regexp` tokens are not laid at all. That keeps
/// what the editor reads off the highlights -- bracket matching and
/// auto-closing skip strings and comments -- exactly as tree-sitter decided,
/// and keeps a comment's codetag colouring. Everywhere else, a semantic token
/// wins, including text tree-sitter left plain; text it does not cover keeps
/// the tree-sitter span.
pub(crate) fn overlay_semantic(base: &Highlights, semantic: &Highlights) -> Highlights {
    let base = base.all();
    let mut out: Vec<HighlightSpan> = Vec::with_capacity(base.len() + semantic.all().len());
    let mut emit = |start: usize, end: usize, token: TokenId| {
        if start >= end {
            return;
        }
        match out.last_mut() {
            Some(last) if last.span.end.0 == start && last.token == token => {
                last.span.end = BytePos(end);
            },
            _ => out.push(HighlightSpan {
                span: Span {
                    start: BytePos(start),
                    end: BytePos(end),
                },
                token,
            }),
        }
    };
    // Everything before `done` has been emitted; `next` is the first base
    // span that may still reach past it.
    let (mut done, mut next) = (0usize, 0usize);
    for laid in semantic.all().iter().filter(|span| lays(span.token)) {
        let (start, end) = (laid.span.start.0.max(done), laid.span.end.0);
        if start >= end {
            continue;
        }
        while let Some(b) = base.get(next).filter(|b| b.span.start.0 < start) {
            emit(b.span.start.0.max(done), b.span.end.0.min(start), b.token);
            if b.span.end.0 > start {
                break;
            }
            next += 1;
        }
        let mut at = start;
        while at < end {
            while base.get(next).is_some_and(|b| b.span.end.0 <= at) {
                next += 1;
            }
            match base.get(next) {
                Some(b) if b.span.start.0 <= at => {
                    let until = b.span.end.0.min(end);
                    let token = if is_lexical(b.token) {
                        b.token
                    } else {
                        laid.token
                    };
                    emit(at, until, token);
                    at = until;
                },
                Some(b) => {
                    let until = b.span.start.0.min(end);
                    emit(at, until, laid.token);
                    at = until;
                },
                None => {
                    emit(at, end, laid.token);
                    at = end;
                },
            }
        }
        done = end;
    }
    for b in base.get(next..).unwrap_or_default() {
        emit(b.span.start.0.max(done), b.span.end.0, b.token);
    }
    Highlights::from_sorted_spans(out)
}

/// Whether a semantic token is laid over the syntax highlights at all.
fn lays(token: TokenId) -> bool {
    use karet_core::SemanticTokenType as T;
    token
        .semantic_parts()
        .is_some_and(|(ty, _)| !matches!(ty, T::Comment | T::String | T::Regexp))
}

/// Whether a tree-sitter class is lexical -- comment, string or markup text --
/// and so never recoloured by a semantic token.
fn is_lexical(token: TokenId) -> bool {
    use karet_core::StandardToken as S;
    [
        S::String,
        S::StringEscape,
        S::Comment,
        S::CommentDoc,
        S::CommentMark,
        S::MarkupHeading,
        S::MarkupBold,
        S::MarkupItalic,
        S::MarkupLink,
        S::MarkupRaw,
        S::MarkupQuote,
        S::MarkupListMarker,
        S::MarkupStrikethrough,
    ]
    .iter()
    .any(|class| class.id() == token)
}

#[cfg(test)]
mod tests {
    use karet_core::SemanticTokenModifiers as M;
    use karet_core::SemanticTokenType as T;
    use karet_core::StandardToken;

    use super::*;

    fn spans(list: &[(usize, usize, TokenId)]) -> Highlights {
        Highlights::from_sorted_spans(
            list.iter()
                .map(|&(start, end, token)| HighlightSpan {
                    span: Span {
                        start: BytePos(start),
                        end: BytePos(end),
                    },
                    token,
                })
                .collect(),
        )
    }

    fn runs(highlights: &Highlights) -> Vec<(usize, usize, TokenId)> {
        highlights
            .all()
            .iter()
            .map(|s| (s.span.start.0, s.span.end.0, s.token))
            .collect()
    }

    const KEYWORD: TokenId = TokenId::KEYWORD;
    const STRING: TokenId = TokenId::STRING;
    const COMMENT: TokenId = TokenId::COMMENT;
    const VARIABLE: TokenId = TokenId::VARIABLE;

    /// A semantic token replaces the code span it covers and fills a gap
    /// tree-sitter left plain; text outside it keeps its syntax span, cut
    /// exactly at the token's edges.
    #[test]
    fn a_semantic_token_wins_over_code_and_fills_gaps() {
        let mac = TokenId::semantic(T::Macro, M::NONE);
        let mutable = TokenId::semantic(T::Variable, M::MUTABLE);
        let base = spans(&[(0, 3, KEYWORD), (4, 10, VARIABLE), (20, 24, KEYWORD)]);
        let semantic = spans(&[(6, 8, mutable), (12, 15, mac)]);
        assert_eq!(
            runs(&overlay_semantic(&base, &semantic)),
            [
                (0, 3, KEYWORD),
                (4, 6, VARIABLE),
                (6, 8, mutable),
                (8, 10, VARIABLE),
                (12, 15, mac),
                (20, 24, KEYWORD),
            ]
        );
    }

    /// A token straddling a string keeps the string's own colour inside it,
    /// so what the editor reads as "inside a string" never changes.
    #[test]
    fn strings_and_comments_keep_their_syntax_colour() {
        let variable = TokenId::semantic(T::Variable, M::NONE);
        let base = spans(&[(0, 4, VARIABLE), (4, 9, STRING), (10, 20, COMMENT)]);
        let semantic = spans(&[(2, 12, variable)]);
        assert_eq!(
            runs(&overlay_semantic(&base, &semantic)),
            [
                (0, 2, VARIABLE),
                (2, 4, variable),
                (4, 9, STRING),
                (9, 10, variable),
                (10, 20, COMMENT),
            ]
        );
    }

    /// Lexical semantic types are never laid, and standard-class ids in the
    /// semantic layer are ignored rather than trusted.
    #[test]
    fn lexical_semantic_types_are_not_laid() {
        let base = spans(&[(0, 5, VARIABLE)]);
        let semantic = spans(&[
            (0, 2, TokenId::semantic(T::String, M::NONE)),
            (2, 3, TokenId::semantic(T::Comment, M::DOCUMENTATION)),
            (3, 4, StandardToken::Keyword.id()),
        ]);
        assert_eq!(
            runs(&overlay_semantic(&base, &semantic)),
            [(0, 5, VARIABLE)]
        );
    }

    /// Overlapping semantic input is clamped rather than double-painted, and
    /// the output always satisfies the `Highlights` ordering invariant.
    #[test]
    fn overlapping_input_still_yields_ordered_disjoint_spans() {
        let a = TokenId::semantic(T::Function, M::NONE);
        let b = TokenId::semantic(T::Parameter, M::NONE);
        let base = spans(&[(0, 10, VARIABLE), (12, 14, KEYWORD)]);
        let semantic = spans(&[(1, 6, a), (4, 8, b), (13, 30, a)]);
        let out = runs(&overlay_semantic(&base, &semantic));
        assert_eq!(
            out,
            [
                (0, 1, VARIABLE),
                (1, 6, a),
                (6, 8, b),
                (8, 10, VARIABLE),
                (12, 13, KEYWORD),
                (13, 30, a),
            ]
        );
        assert!(out.windows(2).all(|w| w[0].1 <= w[1].0));
    }

    #[test]
    fn an_empty_layer_leaves_the_syntax_highlights_alone() {
        let base = spans(&[(0, 3, KEYWORD), (5, 9, STRING)]);
        assert_eq!(
            runs(&overlay_semantic(&base, &Highlights::default())),
            runs(&base)
        );
    }
}
