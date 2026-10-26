//! `karet-search` — code search & replace for the karet toolkit.
//!
//! A ripgrep-style engine usable standalone (depends on no other karet crate):
//! in-file search plus a gitignore-aware workspace walk with streamed results.
//! Positions are reported as plain byte offsets plus 0-based line/column so the
//! crate stays dependency-light; an integrator maps them to its own coordinate
//! types.
//!
//! Search and replace (both in-file and workspace) are implemented via
//! [`search_in_file`]/[`WorkspaceSearch::run`] (or [`search_chunks`] for a
//! buffer held in pieces, such as a rope, with no whole-text copy) and
//! [`plan_replacements`]/[`apply_replacements`]/[`WorkspaceSearch::replace`]. The
//! walk itself is also public as [`walk_text_files`], for consumers that want the
//! same filtered corpus without a pattern, and as [`walk_files`] for consumers
//! that want only the file paths (a file picker). The workspace walk is currently
//! single-threaded (a parallel walk is a deferred optimization).

use regex::Regex;
use regex::RegexBuilder;

mod case;
mod lines;
mod walk;

pub use case::preserve_case;
pub use lines::search_chunks;
pub use walk::FileHit;
pub use walk::IGNORED_DIRS;
pub use walk::ReplaceSummary;
pub use walk::WorkspaceSearch;
pub use walk::WriteFailure;
pub use walk::walk_files;
pub use walk::walk_text_files;

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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    RegexBuilder::new(&regex_pattern(query))
        .case_insensitive(!query.case_sensitive)
        .build()
        .map_err(|_| SearchError::InvalidPattern)
}

/// The regex source [`compile`] builds for `query`: the pattern, escaped unless
/// it is a regex, and wrapped in `\b…\b` for a whole-word query.
fn regex_pattern(query: &SearchQuery) -> String {
    let base = if query.regex {
        query.pattern.clone()
    } else {
        regex::escape(&query.pattern)
    };
    if query.whole_word {
        format!(r"\b(?:{base})\b")
    } else {
        base
    }
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

/// Run the regex `find_iter` loop, tracking line/column linearly.
fn regex_matches(text: &str, re: &Regex) -> Vec<Match> {
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
    matches
}

/// A compiled query, reusable across many buffers.
///
/// Compiling is not free — a regex query pays a full regex build — so a caller
/// searching a whole tree compiles once and reuses the matcher for every file.
/// [`search_in_file`] is the one-shot convenience that compiles per call; reach
/// for this type whenever the same query is run against more than one buffer.
///
/// ```
/// use karet_search::{Matcher, SearchQuery};
///
/// let query = SearchQuery {
///     pattern: "todo".to_string(),
///     ..Default::default()
/// };
/// let matcher = Matcher::new(&query)?;
/// let first = matcher.find("todo: one");
/// let second = matcher.find("nothing here");
/// assert_eq!((first.len(), second.len()), (1, 0));
/// # Ok::<(), karet_search::SearchError>(())
/// ```
pub struct Matcher {
    /// The compiled query.
    kind: MatcherKind,
    /// Whether no match can cross a line break, so a buffer can be searched one
    /// line at a time (see [`Matcher::find_chunks`]).
    line_bounded: bool,
}

/// A compiled query: nothing at all, a literal needle (fast-path), or a regex.
enum MatcherKind {
    /// An empty pattern, which matches nothing whatever the query's options:
    /// compiled as a regex it would match at every position (or at every word
    /// boundary, for a whole-word query).
    Never,
    /// An exact, case-sensitive substring search.
    Literal(String),
    /// A compiled regular expression.
    Regex(Regex),
}

impl Matcher {
    /// Compile `query` into a matcher that can be reused across buffers.
    ///
    /// # Errors
    /// Returns [`SearchError::InvalidPattern`] for a malformed regex.
    pub fn new(query: &SearchQuery) -> Result<Self, SearchError> {
        if query.pattern.is_empty() {
            Ok(Self {
                kind: MatcherKind::Never,
                line_bounded: true,
            })
        } else if !query.regex && query.case_sensitive && !query.whole_word {
            Ok(Self {
                kind: MatcherKind::Literal(query.pattern.clone()),
                line_bounded: !query.pattern.contains('\n'),
            })
        } else {
            Ok(Self {
                kind: MatcherKind::Regex(compile(query)?),
                line_bounded: lines::is_line_bounded(query),
            })
        }
    }

    /// Find every match in `text`.
    ///
    /// An empty pattern never matches, whatever the query's options, matching
    /// [`search_in_file`]'s behavior.
    #[must_use]
    pub fn find(&self, text: &str) -> Vec<Match> {
        match &self.kind {
            MatcherKind::Never => Vec::new(),
            MatcherKind::Literal(needle) => literal_matches(text, needle),
            MatcherKind::Regex(re) => regex_matches(text, re),
        }
    }

    /// Plan a [`Replacement`] for every match in `text`. When `expand` is set (a
    /// regex query), the regex `$1` / `${name}` / `$0` substitutions are expanded
    /// against each match's captures; otherwise `replacement` is inserted literally
    /// (so a literal or whole-word query never mis-reads a `$` in the replacement).
    fn plan(&self, text: &str, replacement: &str, expand: bool) -> Vec<Replacement> {
        match &self.kind {
            MatcherKind::Never => Vec::new(),
            MatcherKind::Literal(needle) => literal_matches(text, needle)
                .into_iter()
                .map(|m| Replacement {
                    start: m.start,
                    end: m.end,
                    text: replacement.to_string(),
                })
                .collect(),
            MatcherKind::Regex(re) => {
                let mut out = Vec::new();
                for caps in re.captures_iter(text) {
                    let Some(whole) = caps.get(0) else {
                        continue;
                    };
                    let text = if expand {
                        let mut dst = String::new();
                        caps.expand(replacement, &mut dst);
                        dst
                    } else {
                        replacement.to_string()
                    };
                    out.push(Replacement {
                        start: whole.start(),
                        end: whole.end(),
                        text,
                    });
                }
                out
            },
        }
    }
}

/// Search `text` for `query`, returning every match.
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] for a malformed regex.
pub fn search_in_file(text: &str, query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
    if query.pattern.is_empty() {
        return Ok(Vec::new());
    }
    Ok(Matcher::new(query)?.find(text))
}

/// Plan a [`Replacement`] for every match of `query` in `text`, replacing each with
/// `replacement`. When [`SearchQuery::regex`] is set, `$1` / `${name}` / `$0`
/// capture substitutions are expanded; otherwise `replacement` is inserted verbatim.
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] for a malformed regex.
pub fn plan_replacements(
    text: &str,
    query: &SearchQuery,
    replacement: &str,
) -> Result<Vec<Replacement>, SearchError> {
    if query.pattern.is_empty() {
        return Ok(Vec::new());
    }
    Ok(Matcher::new(query)?.plan(text, replacement, query.regex))
}

/// Apply `replacements` to `text`, returning the rewritten string. Spans are applied
/// right-to-left so earlier byte offsets stay valid; out-of-range or non-char-boundary
/// spans are skipped defensively.
#[must_use]
pub fn apply_replacements(text: &str, replacements: &[Replacement]) -> String {
    let mut ordered: Vec<&Replacement> = replacements.iter().collect();
    ordered.sort_by_key(|r| std::cmp::Reverse(r.start));
    let mut out = text.to_string();
    for r in ordered {
        if r.start <= r.end
            && r.end <= out.len()
            && out.is_char_boundary(r.start)
            && out.is_char_boundary(r.end)
        {
            out.replace_range(r.start..r.end, &r.text);
        }
    }
    out
}

#[cfg(test)]
mod tests;
