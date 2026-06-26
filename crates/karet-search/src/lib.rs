//! `karet-search` — code search & replace for the karet toolkit.
//!
//! A ripgrep-style engine usable standalone (depends on no other karet crate):
//! incremental in-file search plus a gitignore-aware parallel workspace walk with
//! streamed results and replace planning. Positions are reported as plain byte
//! offsets plus 0-based line/column so the crate stays dependency-light; an
//! integrator maps them to its own coordinate types.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! search/replace logic is filled in separately.

use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

/// Errors produced by search/replace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    /// The search pattern was invalid (e.g. a bad regex).
    #[error("invalid search pattern")]
    InvalidPattern,
    /// An I/O error while walking or reading files.
    #[error("search i/o error")]
    Io,
}

/// A search query with its options and glob filters.
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    /// The pattern (literal text or a regex when `regex` is set).
    pub pattern: String,
    /// Interpret `pattern` as a regular expression.
    pub regex: bool,
    /// Match case-sensitively.
    pub case_sensitive: bool,
    /// Match whole words only.
    pub whole_word: bool,
    /// Glob patterns of paths to include.
    pub includes: Vec<String>,
    /// Glob patterns of paths to exclude.
    pub excludes: Vec<String>,
}

/// A single match within a buffer or file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    /// Byte offset of the match start.
    pub start: usize,
    /// Byte offset of the match end (exclusive).
    pub end: usize,
    /// 0-based line of the match start.
    pub line: u32,
    /// 0-based column (in bytes) of the match start.
    pub col: u32,
}

/// A single replacement within a file: replace `[start, end)` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// Byte offset of the span start.
    pub start: usize,
    /// Byte offset of the span end (exclusive).
    pub end: usize,
    /// The replacement text.
    pub text: String,
}

/// Compile a [`SearchQuery`] into a [`Regex`].
///
/// Literal patterns are escaped; `whole_word` wraps the pattern in `\b…\b`; and
/// matching is case-insensitive unless [`SearchQuery::case_sensitive`] is set.
/// Shared by [`search_in_file`] and the workspace walk so both honor the same
/// semantics.
fn compile(query: &SearchQuery) -> Result<Regex, SearchError> {
    let base = if query.regex {
        query.pattern.clone()
    } else {
        regex::escape(&query.pattern)
    };
    let pattern = if query.whole_word {
        format!(r"\b(?:{base})\b")
    } else {
        base
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .build()
        .map_err(|_| SearchError::InvalidPattern)
}

/// Advance `line`/`line_start` by counting the newlines in `text[from..to]`.
/// `from` must be the byte already accounted for in `line`/`line_start`, and
/// matches arrive in ascending order, so the whole scan is linear.
fn advance_lines(text: &str, from: usize, to: usize, line: &mut u32, line_start: &mut usize) {
    for (i, &b) in text.as_bytes()[from..to].iter().enumerate() {
        if b == b'\n' {
            *line += 1;
            *line_start = from + i + 1;
        }
    }
}

/// Fast-path literal search via [`memchr::memmem`], skipping the regex engine for
/// the common exact, case-sensitive, non-word-bounded query.
fn literal_matches(text: &str, needle: &str) -> Vec<Match> {
    let finder = memchr::memmem::Finder::new(needle.as_bytes());
    let mut matches = Vec::new();
    let (mut line, mut line_start, mut scanned) = (0u32, 0usize, 0usize);
    for start in finder.find_iter(text.as_bytes()) {
        advance_lines(text, scanned, start, &mut line, &mut line_start);
        scanned = start;
        matches.push(Match {
            start,
            end: start + needle.len(),
            line,
            col: (start - line_start) as u32,
        });
    }
    matches
}

/// Search `text` for `query`, returning every match.
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] for a malformed regex.
pub fn search_in_file(text: &str, query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
    if query.pattern.is_empty() {
        return Ok(Vec::new());
    }
    if !query.regex && query.case_sensitive && !query.whole_word {
        return Ok(literal_matches(text, &query.pattern));
    }
    let re = compile(query)?;
    let mut matches = Vec::new();
    let (mut line, mut line_start, mut scanned) = (0u32, 0usize, 0usize);
    for m in re.find_iter(text) {
        advance_lines(text, scanned, m.start(), &mut line, &mut line_start);
        scanned = m.start();
        matches.push(Match {
            start: m.start(),
            end: m.end(),
            line,
            col: (m.start() - line_start) as u32,
        });
    }
    Ok(matches)
}

/// A file together with its matches, streamed from a workspace search.
#[derive(Clone, Debug)]
pub struct FileHit {
    /// The file path.
    pub path: PathBuf,
    /// The matches within the file.
    pub matches: Vec<Match>,
}

/// A parallel, gitignore-aware workspace search.
#[derive(Default)]
pub struct WorkspaceSearch {}

impl WorkspaceSearch {
    /// Create a workspace search.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk `root` and run `query`, invoking `sink` once per file with matches.
    pub fn run(&self, root: &Path, query: &SearchQuery, sink: impl FnMut(FileHit)) {
        let _ = (root, query, sink);
        todo!()
    }
}

/// A planned set of replacements across files.
#[derive(Clone, Debug, Default)]
pub struct ReplacePlan {}

impl ReplacePlan {
    /// The replacements this plan would apply, grouped by file.
    #[must_use]
    pub fn changes(&self) -> Vec<(PathBuf, Vec<Replacement>)> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_defaults() {
        let q = SearchQuery::default();
        assert!(!q.regex);
        assert!(q.includes.is_empty());
    }

    #[test]
    fn error_displays() {
        assert_eq!(
            SearchError::InvalidPattern.to_string(),
            "invalid search pattern"
        );
    }

    fn literal(pattern: &str) -> SearchQuery {
        SearchQuery {
            pattern: pattern.to_string(),
            case_sensitive: true,
            ..Default::default()
        }
    }

    #[test]
    fn literal_finds_all_occurrences() {
        let m = search_in_file("foo bar foo", &literal("foo")).unwrap_or_default();
        assert_eq!(m.len(), 2);
        assert_eq!((m[0].start, m[0].end, m[0].col), (0, 3, 0));
        assert_eq!((m[1].start, m[1].end, m[1].col), (8, 11, 8));
    }

    #[test]
    fn case_insensitive_matches() {
        let q = SearchQuery {
            pattern: "FOO".into(),
            ..Default::default()
        };
        let m = search_in_file("a foo b", &q).unwrap_or_default();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].start, 2);
    }

    #[test]
    fn regex_matches() {
        let q = SearchQuery {
            pattern: "f.o".into(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let m = search_in_file("foo fao fxo", &q).unwrap_or_default();
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn whole_word_excludes_substrings() {
        let q = SearchQuery {
            pattern: "cat".into(),
            whole_word: true,
            case_sensitive: true,
            ..Default::default()
        };
        let m = search_in_file("cat category cat", &q).unwrap_or_default();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].start, 0);
        assert_eq!(m[1].start, 13);
    }

    #[test]
    fn line_and_col_are_tracked() {
        // Both the literal fast-path and the regex path must agree on line/col.
        let lit = search_in_file("ab\ncd ef\ncd", &literal("cd")).unwrap_or_default();
        let re = SearchQuery {
            pattern: "cd".into(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let rex = search_in_file("ab\ncd ef\ncd", &re).unwrap_or_default();
        for m in [&lit, &rex] {
            assert_eq!(m.len(), 2);
            assert_eq!((m[0].line, m[0].col), (1, 0));
            assert_eq!((m[1].line, m[1].col), (2, 0));
        }
    }

    #[test]
    fn invalid_regex_errors() {
        let q = SearchQuery {
            pattern: "(".into(),
            regex: true,
            ..Default::default()
        };
        assert_eq!(search_in_file("x", &q), Err(SearchError::InvalidPattern));
    }

    #[test]
    fn empty_pattern_returns_nothing() {
        assert!(
            search_in_file("abc", &literal(""))
                .unwrap_or_default()
                .is_empty()
        );
    }

    #[test]
    fn zero_width_pattern_terminates() {
        let q = SearchQuery {
            pattern: "x*".into(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        // The key property is that iterating zero-width matches terminates.
        let m = search_in_file("abc", &q).unwrap_or_default();
        assert!(!m.is_empty());
    }
}
