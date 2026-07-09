//! Syntax-highlighting a fenced code block (the `highlight` feature).
//!
//! The fence's info string names a language the same way an injection query's
//! `@injection.language` capture does, so the same resolver serves both — a fence tagged
//! `sh`, `rs` or `c++` finds its grammar.

use karet_core::BytePos;
use karet_core::Span;
use karet_syntax::LayeredHighlighter;
use karet_treesitter::LayeredParser;
use karet_treesitter::language_id_from_injection_name;

use crate::wrap::TextSpan;

/// Highlight `code` as `lang`, returning one styled run list per line.
///
/// `None` when `lang` names no compiled-in grammar or the parse fails; the caller then
/// paints the block as raw markup.
pub(crate) fn code_lines(lang: Option<&str>, code: &str) -> Option<Vec<Vec<TextSpan>>> {
    let lang = language_id_from_injection_name(lang?)?;
    let mut parser = LayeredParser::new();
    let tree = parser.parse(lang, code).ok()?;
    // Layered: a fence may itself embed another language (an HTML block with a script).
    let highlights = LayeredHighlighter::new().highlight(&tree, code);

    let mut lines = Vec::new();
    let mut start = 0usize;
    for line in code.split_inclusive('\n') {
        let end = start + line.len();
        // Trim the newline: it is the line break, not content.
        let content_end = end - line.len() + line.trim_end_matches('\n').len();
        lines.push(paint(
            code,
            start,
            content_end,
            highlights.spans_in(Span {
                start: BytePos(start),
                end: BytePos(content_end),
            }),
        ));
        start = end;
    }
    Some(lines)
}

/// Slice `[start, end)` of `code` into styled runs, filling the gaps between
/// `highlights` with unstyled text.
fn paint(
    code: &str,
    start: usize,
    end: usize,
    highlights: &[karet_syntax::HighlightSpan],
) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    let mut cursor = start;
    let mut push = |from: usize, to: usize, token| {
        if to > from
            && let Some(text) = code.get(from..to)
        {
            spans.push(TextSpan {
                text: text.to_owned(),
                token,
            });
        }
    };

    for hl in highlights {
        let from = hl.span.start.0.max(start);
        let to = hl.span.end.0.min(end);
        if to <= from {
            continue;
        }
        push(cursor, from, None);
        push(from, to, Some(hl.token));
        cursor = to;
    }
    push(cursor, end, None);
    spans
}

#[cfg(test)]
mod tests {
    use karet_core::TokenId;

    use super::*;

    #[test]
    fn unknown_or_absent_language_declines() {
        assert!(code_lines(None, "x").is_none());
        assert!(code_lines(Some("brainfuck"), "+++").is_none());
    }

    #[test]
    fn rust_fence_is_highlighted_line_by_line() {
        let Some(lines) = code_lines(Some("rust"), "fn main() {\n    let x = 1;\n}\n") else {
            return; // the rust grammar is not compiled into this build
        };
        assert_eq!(lines.len(), 3);
        // `fn` opens the first line as a keyword.
        assert_eq!(
            lines.first().and_then(|l| l.first()).map(|s| s.token),
            Some(Some(TokenId::KEYWORD))
        );
        // Every line's text round-trips (no bytes lost or duplicated in the painting).
        let joined: String = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(joined, "fn main() {\n    let x = 1;\n}");
    }

    #[test]
    fn a_fence_alias_resolves_to_its_grammar() {
        // `rs` is rust; the resolver is shared with injection queries.
        assert_eq!(
            code_lines(Some("rs"), "fn f() {}\n").is_some(),
            code_lines(Some("rust"), "fn f() {}\n").is_some()
        );
    }

    #[test]
    fn newlines_are_not_painted_into_a_span() {
        let Some(lines) = code_lines(Some("rust"), "fn f() {}\n") else {
            return;
        };
        assert!(
            lines.iter().flatten().all(|s| !s.text.contains('\n')),
            "a line's runs must not carry the line break"
        );
    }
}
