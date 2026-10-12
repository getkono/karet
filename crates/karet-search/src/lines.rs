//! Searching a buffer handed over in pieces (a rope's chunks, say) without first
//! joining it into one contiguous string.
//!
//! A query that cannot match across a line break finds exactly the same matches
//! whether it scans the whole text or each line on its own, so such a query
//! runs line by line: only a line that straddles two chunks is copied, and only
//! that line. A query that could cross a line break (a regex naming `\n`, `\s`
//! or a negated class) or that anchors to the whole text (`^`/`$` outside
//! multi-line mode) joins the chunks once and searches the result, so every
//! query keeps the answer [`search_in_file`](crate::search_in_file) gives.

use regex_syntax::ParserBuilder;
use regex_syntax::hir::Class;
use regex_syntax::hir::Hir;
use regex_syntax::hir::HirKind;
use regex_syntax::hir::Look;

use crate::Match;
use crate::Matcher;
use crate::MatcherKind;
use crate::SearchError;
use crate::SearchQuery;

/// Search the text formed by concatenating `chunks` for `query`.
///
/// Returns the same matches [`search_in_file`](crate::search_in_file) returns for
/// the joined text, with byte offsets and line/column relative to that text, but
/// copies only what a line-at-a-time search needs (see [`Matcher::find_chunks`]).
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] for a malformed regex.
pub fn search_chunks<'a>(
    chunks: impl IntoIterator<Item = &'a str>,
    query: &SearchQuery,
) -> Result<Vec<Match>, SearchError> {
    if query.pattern.is_empty() {
        return Ok(Vec::new());
    }
    Ok(Matcher::new(query)?.find_chunks(chunks))
}

impl Matcher {
    /// Find every match in the text formed by concatenating `chunks`.
    ///
    /// Agrees with [`find`](Self::find) over the joined text. When no match of
    /// this query can cross a line break, the chunks are searched one line at a
    /// time and only lines split across chunks are copied; otherwise they are
    /// joined into one string first.
    #[must_use]
    pub fn find_chunks<'a>(&self, chunks: impl IntoIterator<Item = &'a str>) -> Vec<Match> {
        if matches!(&self.kind, MatcherKind::Literal(needle) if needle.is_empty()) {
            return Vec::new();
        }
        if !self.line_bounded {
            let text: String = chunks.into_iter().collect();
            return self.find(&text);
        }
        let mut lines = LineSearch {
            matcher: self,
            matches: Vec::new(),
            start: 0,
            line: 0,
        };
        // The part of the current line seen so far, when it began in an earlier
        // chunk; empty while the current line starts in the chunk being read.
        let mut partial = String::new();
        for chunk in chunks {
            let mut rest = chunk;
            while let Some(at) = memchr::memchr(b'\n', rest.as_bytes()) {
                let (head, tail) = rest.split_at(at);
                if partial.is_empty() {
                    lines.search(head);
                } else {
                    partial.push_str(head);
                    lines.search(&partial);
                    partial.clear();
                }
                rest = tail.get(1..).unwrap_or_default();
            }
            partial.push_str(rest);
        }
        lines.search(&partial);
        lines.matches
    }
}

/// The running state of a line-at-a-time search.
struct LineSearch<'m> {
    matcher: &'m Matcher,
    matches: Vec<Match>,
    /// Byte offset of the current line's start in the whole text.
    start: usize,
    /// 0-based index of the current line.
    line: u32,
}

impl LineSearch<'_> {
    /// Search `content` (one line, without its `\n`) and move past it and the
    /// line break that ends it.
    fn search(&mut self, content: &str) {
        for found in self.matcher.find(content) {
            self.matches.push(Match {
                start: self.start + found.start,
                end: self.start + found.end,
                line: self.line,
                col: found.col,
            });
        }
        self.start += content.len() + 1;
        self.line += 1;
    }
}

/// Whether no match of the regex `query` compiles to can cross a line break or
/// tell a line from the whole text, so a per-line search finds exactly what a
/// whole-text search does.
///
/// Conservative: `false` whenever the pattern could match `\n`, uses a
/// whole-text anchor (`\A`, `\z`, or `^`/`$` outside multi-line mode), uses
/// CRLF-mode anchors, or fails to parse.
pub(crate) fn is_line_bounded(query: &SearchQuery) -> bool {
    ParserBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .build()
        .parse(&crate::regex_pattern(query))
        .is_ok_and(|hir| stays_on_one_line(&hir))
}

fn stays_on_one_line(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Empty => true,
        HirKind::Literal(literal) => !literal.0.contains(&b'\n'),
        HirKind::Class(Class::Unicode(class)) => class
            .ranges()
            .iter()
            .all(|range| !(range.start()..=range.end()).contains(&'\n')),
        HirKind::Class(Class::Bytes(class)) => class
            .ranges()
            .iter()
            .all(|range| !(range.start()..=range.end()).contains(&b'\n')),
        // A line's edges look to these exactly as the `\n` beside them does in
        // the whole text: a line start/end in multi-line mode, and a non-word
        // character for every word-boundary assertion.
        HirKind::Look(look) => !matches!(
            look,
            Look::Start | Look::End | Look::StartCRLF | Look::EndCRLF
        ),
        HirKind::Repetition(repetition) => stays_on_one_line(&repetition.sub),
        HirKind::Capture(capture) => stays_on_one_line(&capture.sub),
        HirKind::Concat(hirs) | HirKind::Alternation(hirs) => hirs.iter().all(stays_on_one_line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_in_file;

    fn query(pattern: &str, regex: bool, case_sensitive: bool, whole_word: bool) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_owned(),
            regex,
            case_sensitive,
            whole_word,
            ..SearchQuery::default()
        }
    }

    /// `text` cut into pieces of `size` characters (the last may be shorter).
    fn pieces(text: &str, size: usize) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let cut = rest
                .char_indices()
                .nth(size)
                .map_or(rest.len(), |(at, _)| at);
            let (head, tail) = rest.split_at(cut);
            out.push(head);
            rest = tail;
        }
        out
    }

    /// The chunked search must agree with the whole-text search for every query
    /// shape and every way of cutting the text, including cuts inside a line,
    /// inside a match, and right before or after a line break.
    #[test]
    fn chunked_search_agrees_with_the_whole_text_search() {
        let queries = [
            query("foo", false, true, false),
            query("FOO", false, false, false),
            query("foo", false, true, true),
            query("f.o", true, true, false),
            query("o*", true, true, false),
            query(r"(?m)^foo", true, true, false),
            query(r"(?m)foo$", true, true, false),
            query(r"\bfo\B", true, false, false),
            query("^foo", true, true, false),
            query("foo$", true, true, false),
            query(r"o\nf", true, true, false),
            query(r"o\sf", true, true, false),
            query("[^x]+", true, true, false),
            query("é", false, false, false),
        ];
        let texts = [
            "",
            "\n",
            "foo",
            "foo\n",
            "foo bar foo\nfoo\n\nxfoo foox\r\nfoo\r\n",
            "a foo\nfoo b\nfoo",
            "café foo\nÉcole foo\n\n",
        ];
        for q in &queries {
            for text in texts {
                let whole = search_in_file(text, q).unwrap_or_default();
                for size in 1..=text.chars().count().max(1) {
                    assert_eq!(
                        search_chunks(pieces(text, size), q).unwrap_or_default(),
                        whole,
                        "pattern {:?} over {text:?} in {size}-char pieces",
                        q.pattern,
                    );
                }
            }
        }
    }

    #[test]
    fn a_line_bounded_query_reports_absolute_offsets_and_lines() {
        let found = search_chunks(["ab\ncd e", "f\ncd"], &query("cd", false, true, false))
            .unwrap_or_default();
        let spans: Vec<_> = found
            .iter()
            .map(|m| (m.start, m.end, m.line, m.col))
            .collect();
        assert_eq!(spans, vec![(3, 5, 1, 0), (9, 11, 2, 0)]);
    }

    #[test]
    fn line_bounded_is_decided_from_the_pattern() {
        let bounded =
            |pattern: &str, regex: bool| is_line_bounded(&query(pattern, regex, true, false));
        assert!(bounded("foo", false));
        assert!(bounded(r"\w+\b", true));
        assert!(!bounded(r"(?m)^\s*$", true), "\\s matches a line break");
        assert!(bounded(r"(?m)^[a-z]+$", true));
        assert!(!bounded("^foo", true), "\\A anchors to the whole text");
        assert!(!bounded("foo$", true), "\\z anchors to the whole text");
        assert!(
            !bounded(r"(?Rm)^foo", true),
            "CRLF anchors are not line-local"
        );
        assert!(!bounded("(?s).", true), "dot-all matches a line break");
        assert!(
            !bounded("(", true),
            "an unparsable pattern is never bounded"
        );
    }

    #[test]
    fn an_empty_pattern_never_matches() {
        assert!(
            search_chunks(["abc"], &query("", false, false, false))
                .unwrap_or_default()
                .is_empty()
        );
        let matcher = Matcher::new(&query("", false, true, false));
        assert_eq!(matcher.map(|m| m.find_chunks(["abc"]).len()).ok(), Some(0));
    }

    #[test]
    fn an_invalid_regex_errors() {
        assert_eq!(
            search_chunks(["x"], &query("(", true, true, false)),
            Err(SearchError::InvalidPattern)
        );
    }
}
