//! The semantic-comment pass: retint the comment spans a codetag block covers.
//!
//! By convention, comments opened by a codetag (`TODO: fix this`, `FIXME(alice):
//! …`) deserve to stand out from the comments a reader skims past.
//! [`mark_semantic_comments`] is a pure post-pass over a buffer's [`Highlights`]:
//! it finds the comment lines whose content opens with a configured tag, extends
//! each match through the immediately following non-empty comment lines, and
//! restamps those spans [`StandardToken::CommentMark`]. Detection is
//! language-agnostic — the comment leader is whatever punctuation precedes the
//! text — so it works with any grammar that captures comments.

use karet_core::BytePos;
use karet_core::Span;
use karet_core::StandardToken;
use karet_core::TokenId;

use crate::HighlightSpan;
use crate::Highlights;

/// Configuration for [`mark_semantic_comments`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticCommentConfig {
    /// The codetags that open a block.
    ///
    /// Matching is case-sensitive and whole-word: the tag must start the comment's
    /// content (right after its leader) and be followed by `:`, `(`, whitespace, or
    /// the end of the line — `TODOX` never matches `TODO`. Tags should start with
    /// an alphanumeric character: leading punctuation is indistinguishable from a
    /// comment leader and is stripped before matching.
    pub tags: Vec<String>,
}

impl Default for SemanticCommentConfig {
    /// The conventional codetags: `TODO`, `FIXME`, `HACK`, `XXX`, `BUG`.
    ///
    /// `NOTE` is deliberately absent — it annotates prose the author considered
    /// settled, not work that still demands attention.
    fn default() -> Self {
        Self {
            tags: ["TODO", "FIXME", "HACK", "XXX", "BUG"]
                .map(str::to_owned)
                .into(),
        }
    }
}

/// Retint the comment spans belonging to a codetag block to
/// [`StandardToken::CommentMark`].
///
/// A block opens on a comment line whose content — after stripping its leader,
/// i.e. any leading whitespace and punctuation — starts with one of `config.tags`
/// (case-sensitive, whole-word; see [`SemanticCommentConfig::tags`]). The block
/// extends through the immediately following comment lines with non-empty
/// content; a line opening another tag simply starts the next block. A
/// leader-only comment line (a bare `//`, or a `// ----` divider — anything whose
/// content strips to nothing) ends the block and is not marked, as does a line
/// carrying no comment at all. A comment trailing code can open a block.
///
/// Marking is line-granular: every plain- or doc-comment span on a marked line is
/// restamped, and a multi-line `/* … */` span is split so only its marked lines
/// change color — its interior lines continue the block while non-empty, and a
/// closing-delimiter-only line (`*/`) strips to empty, ending it unmarked.
///
/// The pass is pure: `text` must be the exact buffer `highlights` was computed
/// from, and everything that is not a comment passes through untouched.
#[must_use]
pub fn mark_semantic_comments(
    text: &str,
    highlights: &Highlights,
    config: &SemanticCommentConfig,
) -> Highlights {
    if config.tags.is_empty() || !highlights.all().iter().any(|s| is_comment(s.token)) {
        return highlights.clone();
    }

    let lines = line_spans(text);
    let marked = marked_lines(text, &lines, highlights, &config.tags);
    if !marked.contains(&true) {
        return highlights.clone();
    }

    let mut out: Vec<HighlightSpan> = Vec::with_capacity(highlights.all().len());
    for s in highlights.all() {
        if !is_comment(s.token) {
            push(&mut out, *s);
            continue;
        }
        // Split the span at line boundaries so each line's verdict applies to
        // exactly its own slice (a newline stays with the line it terminates).
        let mut line = line_of(&lines, s.span.start.0);
        let mut piece_start = s.span.start.0;
        while piece_start < s.span.end.0 {
            let line_end = lines.get(line + 1).map_or(text.len(), |l| l.start);
            let piece_end = s.span.end.0.min(line_end.max(piece_start + 1));
            let token = if marked.get(line).copied().unwrap_or(false) {
                StandardToken::CommentMark.id()
            } else {
                s.token
            };
            push(
                &mut out,
                HighlightSpan {
                    span: Span {
                        start: BytePos(piece_start),
                        end: BytePos(piece_end),
                    },
                    token,
                },
            );
            piece_start = piece_end;
            line += 1;
        }
    }
    Highlights::from_sorted_spans(out)
}

/// What a line contributes to block detection, in increasing precedence — a line
/// with several comment spans takes the strongest reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum LineKind {
    /// No comment span touches the line.
    NonComment,
    /// Comment content strips to nothing (a bare leader or a `----` divider).
    Empty,
    /// Non-empty comment content that opens no tag.
    Plain,
    /// Comment content opening a configured tag.
    Tag,
}

/// A line's byte extent: where it starts and where its content ends (before the
/// line terminator).
#[derive(Clone, Copy)]
struct LineSpan {
    start: usize,
    content_end: usize,
}

/// The byte extent of every line of `text`.
fn line_spans(text: &str) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for seg in text.split_inclusive('\n') {
        let body = seg.strip_suffix('\n').unwrap_or(seg);
        let body = body.strip_suffix('\r').unwrap_or(body);
        lines.push(LineSpan {
            start,
            content_end: start + body.len(),
        });
        start += seg.len();
    }
    lines
}

/// The index of the line containing byte `pos`.
fn line_of(lines: &[LineSpan], pos: usize) -> usize {
    lines.partition_point(|l| l.start <= pos).saturating_sub(1)
}

/// Decide, line by line, whether the line's comments belong to a codetag block.
fn marked_lines(
    text: &str,
    lines: &[LineSpan],
    highlights: &Highlights,
    tags: &[String],
) -> Vec<bool> {
    let mut kinds = vec![LineKind::NonComment; lines.len()];
    for s in highlights.all().iter().filter(|s| is_comment(s.token)) {
        let mut line = line_of(lines, s.span.start.0);
        while let Some(l) = lines.get(line).filter(|l| l.start < s.span.end.0) {
            let start = s.span.start.0.max(l.start);
            let end = s.span.end.0.min(l.content_end);
            let content = if start < end {
                text.get(start..end).unwrap_or("")
            } else {
                ""
            };
            if let Some(kind) = kinds.get_mut(line) {
                *kind = (*kind).max(classify(content, tags));
            }
            line += 1;
        }
    }

    let mut marked = vec![false; lines.len()];
    let mut in_block = false;
    for (flag, kind) in marked.iter_mut().zip(&kinds) {
        match kind {
            LineKind::Tag => {
                *flag = true;
                in_block = true;
            },
            LineKind::Plain => *flag = in_block,
            LineKind::Empty | LineKind::NonComment => in_block = false,
        }
    }
    marked
}

/// Classify one comment span's slice of a line.
fn classify(content: &str, tags: &[String]) -> LineKind {
    // The leader is whatever punctuation/whitespace precedes the text — `//`,
    // `///`, `#`, `--`, a `/*` opener, a `*` gutter — so strip every leading
    // non-alphanumeric character rather than hardcode per-language leaders.
    let stripped = content.trim_start_matches(|c: char| !c.is_alphanumeric());
    if stripped.is_empty() {
        LineKind::Empty
    } else if tags.iter().any(|t| tag_opens(stripped, t)) {
        LineKind::Tag
    } else {
        LineKind::Plain
    }
}

/// Whether `content` opens with `tag` as a whole word.
fn tag_opens(content: &str, tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    match content.strip_prefix(tag) {
        Some(rest) => match rest.chars().next() {
            None => true,
            Some(c) => c == ':' || c == '(' || c.is_whitespace(),
        },
        None => false,
    }
}

/// Whether `token` is a comment class the pass may restamp.
fn is_comment(token: TokenId) -> bool {
    token == TokenId::COMMENT || token == StandardToken::CommentDoc.id()
}

/// Append `span`, coalescing with an adjacent same-token predecessor.
fn push(out: &mut Vec<HighlightSpan>, span: HighlightSpan) {
    if let Some(last) = out.last_mut()
        && last.token == span.token
        && last.span.end.0 == span.span.start.0
    {
        last.span.end = span.span.end;
        return;
    }
    out.push(span);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARK: TokenId = StandardToken::CommentMark.id();
    const DOC: TokenId = StandardToken::CommentDoc.id();

    /// Build [`Highlights`] straight from `(start, end, token)` triples.
    fn hl(spans: &[(usize, usize, TokenId)]) -> Highlights {
        Highlights::from_sorted_spans(
            spans
                .iter()
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

    /// Flatten [`Highlights`] back to comparable triples.
    fn triples(h: &Highlights) -> Vec<(usize, usize, TokenId)> {
        h.all()
            .iter()
            .map(|s| (s.span.start.0, s.span.end.0, s.token))
            .collect()
    }

    /// Tag every `//`-to-end-of-line region of `text` as a plain comment — the
    /// shape a line-comment grammar produces.
    fn line_comment_spans(text: &str) -> Highlights {
        let mut spans = Vec::new();
        let mut start = 0usize;
        for seg in text.split_inclusive('\n') {
            let body = seg.strip_suffix('\n').unwrap_or(seg);
            let body = body.strip_suffix('\r').unwrap_or(body);
            if let Some(at) = body.find("//") {
                spans.push((start + at, start + body.len(), TokenId::COMMENT));
            }
            start += seg.len();
        }
        hl(&spans)
    }

    fn run(text: &str, highlights: &Highlights) -> Highlights {
        mark_semantic_comments(text, highlights, &SemanticCommentConfig::default())
    }

    /// The token painted over the first occurrence of `needle`.
    fn token_at(h: &Highlights, text: &str, needle: &str) -> Option<TokenId> {
        let at = text.find(needle)?;
        h.all()
            .iter()
            .find(|s| s.span.start.0 <= at && at < s.span.end.0)
            .map(|s| s.token)
    }

    fn assert_sorted_non_overlapping(h: &Highlights) {
        assert!(
            h.all()
                .windows(2)
                .all(|w| w[0].span.end.0 <= w[1].span.start.0),
            "spans must stay sorted and non-overlapping: {:?}",
            h.all()
        );
    }

    #[test]
    fn default_tags_are_the_conventional_codetags() {
        let config = SemanticCommentConfig::default();
        assert_eq!(config.tags, ["TODO", "FIXME", "HACK", "XXX", "BUG"]);
        // `NOTE` is deliberately not a default: the issue's example leaves its
        // `Note:` comment unmarked.
        assert!(!config.tags.iter().any(|t| t.eq_ignore_ascii_case("note")));
    }

    #[test]
    fn issue_example_marks_only_the_tag_block() {
        // The exact example from issue #49: L1–L3 marked, L4 (bare leader) and L5
        // (`Note:` is not a tag) unmarked.
        let text = "\
// TODO: fix bug here
// here is some context...
// - lorem ipsum
//
// Note: this function is currently not being used
";
        let out = run(text, &line_comment_spans(text));
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "here is some context"), Some(MARK));
        assert_eq!(token_at(&out, text, "- lorem ipsum"), Some(MARK));
        assert_eq!(token_at(&out, text, "Note:"), Some(TokenId::COMMENT));
        // The bare `//` on L4 keeps its plain token.
        let l4 = text.lines().take(3).map(|l| l.len() + 1).sum::<usize>();
        assert_eq!(
            out.all()
                .iter()
                .find(|s| s.span.start.0 == l4)
                .map(|s| s.token),
            Some(TokenId::COMMENT)
        );
        assert_sorted_non_overlapping(&out);
    }

    #[test]
    fn tag_must_start_the_comment_content() {
        let text = "// see TODO later\n";
        let base = line_comment_spans(text);
        assert_eq!(triples(&run(text, &base)), triples(&base));
    }

    #[test]
    fn tag_word_boundaries() {
        for (text, expect) in [
            ("// TODO: colon\n", Some(MARK)),
            ("// TODO\n", Some(MARK)),
            ("// TODO(alice): owner\n", Some(MARK)),
            ("// TODO space\n", Some(MARK)),
            ("//TODO: no gap after the leader\n", Some(MARK)),
            ("// TODOX no boundary\n", Some(TokenId::COMMENT)),
            ("// TODOs plural\n", Some(TokenId::COMMENT)),
        ] {
            let out = run(text, &line_comment_spans(text));
            assert_eq!(token_at(&out, text, "TODO"), expect, "text: {text:?}");
        }
    }

    #[test]
    fn matching_is_case_sensitive() {
        let text = "// todo: lowercase is prose, not a codetag\n";
        let base = line_comment_spans(text);
        assert_eq!(triples(&run(text, &base)), triples(&base));
    }

    #[test]
    fn custom_tags_replace_the_defaults() {
        let text = "\
// SAFETY: the pointer is checked above
let x = 1;
// TODO: not a tag under this config
";
        let config = SemanticCommentConfig {
            tags: vec!["SAFETY".to_owned()],
        };
        let out = mark_semantic_comments(text, &line_comment_spans(text), &config);
        assert_eq!(token_at(&out, text, "SAFETY"), Some(MARK));
        // The default tags are replaced, not extended: TODO no longer opens a block.
        assert_eq!(token_at(&out, text, "TODO"), Some(TokenId::COMMENT));
    }

    #[test]
    fn multiple_blocks_and_interrupting_code() {
        let text = "\
// FIXME: first block
// its continuation
let x = 1;
// stray comment, no tag
// BUG: second block
";
        let out = run(text, &line_comment_spans(text));
        assert_eq!(token_at(&out, text, "FIXME"), Some(MARK));
        assert_eq!(token_at(&out, text, "its continuation"), Some(MARK));
        // The code line ends the block; the stray comment after it is unmarked.
        assert_eq!(
            token_at(&out, text, "stray comment"),
            Some(TokenId::COMMENT)
        );
        assert_eq!(token_at(&out, text, "BUG"), Some(MARK));
        assert_sorted_non_overlapping(&out);
    }

    #[test]
    fn back_to_back_tag_lines_each_open_a_block() {
        let text = "// TODO: first\n// FIXME: second\n// shared tail\n";
        let out = run(text, &line_comment_spans(text));
        assert_eq!(token_at(&out, text, "first"), Some(MARK));
        assert_eq!(token_at(&out, text, "second"), Some(MARK));
        assert_eq!(token_at(&out, text, "shared tail"), Some(MARK));
    }

    #[test]
    fn divider_line_strips_empty_and_ends_the_block() {
        // A `----` divider has no alphanumeric content: like a bare leader, it
        // terminates the block and stays unmarked.
        let text = "// HACK: fragile\n// ----\n// unrelated\n";
        let out = run(text, &line_comment_spans(text));
        assert_eq!(token_at(&out, text, "HACK"), Some(MARK));
        assert_eq!(token_at(&out, text, "----"), Some(TokenId::COMMENT));
        assert_eq!(token_at(&out, text, "unrelated"), Some(TokenId::COMMENT));
    }

    #[test]
    fn trailing_comment_after_code_opens_a_block() {
        let text = "let x = 1; // TODO: rename\n// context below\nlet y = 2;\n";
        let mut spans = vec![(0, 3, TokenId::KEYWORD)]; // `let`
        spans.extend(triples(&line_comment_spans(text)));
        let base = hl(&spans);
        let out = run(text, &base);
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "context below"), Some(MARK));
        // The code before the comment is untouched.
        assert_eq!(token_at(&out, text, "let x"), Some(TokenId::KEYWORD));
        assert_sorted_non_overlapping(&out);
    }

    #[test]
    fn multiline_block_comment_is_split_per_line() {
        // One grammar span covers the whole `/* … */`. Its tag and continuation
        // lines are marked; the closing-delimiter-only line strips to empty, so it
        // ends the block unmarked — pinned here as the documented behavior.
        let text = "/* TODO: fix\n   details\n*/\nlet x = 1;\n";
        let close = text.find("*/").unwrap_or(0);
        let base = hl(&[
            (0, close + 2, TokenId::COMMENT),
            (close + 3, close + 6, TokenId::KEYWORD),
        ]);
        let out = run(text, &base);
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "details"), Some(MARK));
        assert_eq!(token_at(&out, text, "*/"), Some(TokenId::COMMENT));
        assert_eq!(token_at(&out, text, "let x"), Some(TokenId::KEYWORD));
        // The split yields exactly one marked piece (both marked lines coalesce,
        // newline included) and one unmarked remainder.
        assert_eq!(
            triples(&out),
            vec![
                (0, close, MARK),
                (close, close + 2, TokenId::COMMENT),
                (close + 3, close + 6, TokenId::KEYWORD),
            ]
        );
        assert_sorted_non_overlapping(&out);
    }

    #[test]
    fn doc_comment_spans_participate() {
        let text = "/// TODO: document this properly\n/// once the API settles\n";
        let first = text.find('\n').unwrap_or(0);
        let base = hl(&[(0, first, DOC), (first + 1, text.len() - 1, DOC)]);
        let out = run(text, &base);
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "once the API"), Some(MARK));
    }

    #[test]
    fn empty_tag_list_is_a_no_op() {
        let text = "// TODO: fix\n";
        let base = line_comment_spans(text);
        let config = SemanticCommentConfig { tags: Vec::new() };
        assert_eq!(
            triples(&mark_semantic_comments(text, &base, &config)),
            triples(&base)
        );
    }

    #[test]
    fn non_comment_spans_are_untouched() {
        // `TODO` inside a string is a string, not a codetag.
        let text = "let s = \"TODO: not a comment\";\n";
        let base = hl(&[(8, 29, TokenId::STRING)]);
        assert_eq!(triples(&run(text, &base)), triples(&base));
    }

    #[test]
    fn crlf_lines_classify_correctly() {
        let text = "// TODO: fix\r\n// context\r\n//\r\n// after\r\n";
        let out = run(text, &line_comment_spans(text));
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "context"), Some(MARK));
        assert_eq!(token_at(&out, text, "after"), Some(TokenId::COMMENT));
    }

    #[test]
    fn marks_todo_comments_in_real_rust() -> Result<(), Box<dyn std::error::Error>> {
        use karet_treesitter::LayeredParser;
        use karet_treesitter::language_id_from_injection_name;

        let Some(rust) = language_id_from_injection_name("rust") else {
            return Ok(()); // rust grammar not compiled in; nothing to test
        };
        let text = "\
// TODO: fix bug here
// here is some context...
//
// Note: currently unused
fn main() {}
";
        let mut parser = LayeredParser::new();
        let tree = parser.parse(rust, text)?;
        let base = crate::LayeredHighlighter::new().highlight(&tree, text);
        let out = run(text, &base);
        assert_eq!(token_at(&out, text, "TODO"), Some(MARK));
        assert_eq!(token_at(&out, text, "here is some context"), Some(MARK));
        assert_eq!(token_at(&out, text, "Note:"), Some(TokenId::COMMENT));
        assert_eq!(token_at(&out, text, "fn main"), Some(TokenId::KEYWORD));
        assert_sorted_non_overlapping(&out);
        Ok(())
    }
}
