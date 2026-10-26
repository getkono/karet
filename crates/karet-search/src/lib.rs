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
//! same filtered corpus without a pattern. The workspace walk is currently
//! single-threaded (a parallel walk is a deferred optimization).

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;

use regex::Regex;
use regex::RegexBuilder;

mod case;
mod lines;

pub use case::preserve_case;
pub use lines::search_chunks;

/// Directory names every workspace walk prunes outright, on top of the
/// gitignore/hidden filters (kept in parity with `karet-watch`'s watcher
/// enumeration): VCS metadata and the classic heavyweight build/dependency
/// trees, which drown results even when a workspace has no `.gitignore`.
pub const IGNORED_DIRS: &[&str] = &[".git", "target", "node_modules"];

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

/// A file together with its matches, streamed from a workspace search.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileHit {
    /// The file path.
    pub path: PathBuf,
    /// The matches within the file.
    pub matches: Vec<Match>,
}

/// The maximum file size (in bytes) the workspace search will read; larger files
/// are skipped. Tune later alongside the deferred parallel walk.
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// How many leading bytes to inspect when sniffing for binary content.
const BINARY_SNIFF_BYTES: usize = 8192;

/// A gitignore-aware workspace search.
#[derive(Default)]
pub struct WorkspaceSearch {}

impl WorkspaceSearch {
    /// Create a workspace search.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Walk `root` and run `query`, invoking `sink` once per file with matches.
    ///
    /// The walk honors `.gitignore`/hidden-file conventions and the query's
    /// `includes`/`excludes` globs (ripgrep `-g` semantics), and skips binary and
    /// oversize files. Results stream through `sink` as each matching file is
    /// found.
    ///
    /// The current walk is single-threaded; a parallel
    /// [`ignore::WalkBuilder::build_parallel`]-based path is a deferred
    /// optimization.
    ///
    /// # Errors
    /// Returns [`SearchError::InvalidPattern`] if the pattern or an include/exclude
    /// glob is invalid.
    pub fn run(
        &self,
        root: &Path,
        query: &SearchQuery,
        mut sink: impl FnMut(FileHit),
    ) -> Result<(), SearchError> {
        if query.pattern.is_empty() {
            return Ok(());
        }
        let matcher = Matcher::new(query)?;
        walk_text_files(root, &query.includes, &query.excludes, |path, text| {
            let matches = matcher.find(&text);
            if !matches.is_empty() {
                sink(FileHit {
                    path: path.to_path_buf(),
                    matches,
                });
            }
            ControlFlow::Continue(())
        })
    }

    /// Walk `root` and replace every match of `query` with `replacement`, writing
    /// each changed file back to disk. Honors the same gitignore / glob / binary /
    /// size filters as [`run`](Self::run); returns a [`ReplaceSummary`] of what
    /// changed. Regex capture substitutions (`$1`, `${name}`) apply when
    /// [`SearchQuery::regex`] is set.
    ///
    /// A file that cannot be written back does not stop the walk: the other files
    /// are still rewritten, and the one that failed is listed in
    /// [`ReplaceSummary::failures`] (and counted nowhere else), so the caller can
    /// tell a partial replace from a complete one.
    ///
    /// # Errors
    /// Returns [`SearchError::InvalidPattern`] if the pattern or a glob is invalid.
    pub fn replace(
        &self,
        root: &Path,
        query: &SearchQuery,
        replacement: &str,
    ) -> Result<ReplaceSummary, SearchError> {
        if query.pattern.is_empty() {
            return Ok(ReplaceSummary::default());
        }
        let matcher = Matcher::new(query)?;
        let mut summary = ReplaceSummary::default();
        walk_text_files(root, &query.includes, &query.excludes, |path, text| {
            let plan = matcher.plan(&text, replacement, query.regex);
            if !plan.is_empty() {
                let updated = apply_replacements(&text, &plan);
                match std::fs::write(path, updated) {
                    Ok(()) => {
                        summary.files_changed += 1;
                        summary.replacements += plan.len();
                    },
                    Err(error) => summary.failures.push(WriteFailure {
                        path: path.to_path_buf(),
                        error: error.to_string(),
                    }),
                }
            }
            ControlFlow::Continue(())
        })?;
        Ok(summary)
    }
}

/// Walk `root` gitignore-aware and hand every readable text file to `sink`.
///
/// The walk applies exactly the filters [`WorkspaceSearch::run`] does — `.gitignore`
/// and hidden-file conventions, the pruned [`IGNORED_DIRS`], ripgrep `-g` semantics
/// for `includes`/`excludes`, and the binary/oversize/non-UTF-8 skips — so a consumer
/// that needs the *files* rather than pattern matches (a workspace linter, a
/// spell-check pass, an indexer) sees the same corpus a search would.
///
/// `sink` receives each file's path and its full contents, and returns
/// [`ControlFlow::Break`] to stop the walk early (a result cap, a cancelled request).
/// The walk is single-threaded, matching [`WorkspaceSearch::run`].
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] if an include/exclude glob is invalid.
pub fn walk_text_files(
    root: &Path,
    includes: &[String],
    excludes: &[String],
    mut sink: impl FnMut(&Path, String) -> ControlFlow<()>,
) -> Result<(), SearchError> {
    for entry in build_walk(root, includes, excludes)?.flatten() {
        let Some(text) = read_searchable(&entry) else {
            continue;
        };
        if sink(entry.path(), text).is_break() {
            break;
        }
    }
    Ok(())
}

/// The result of a workspace [`replace`](WorkspaceSearch::replace).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplaceSummary {
    /// The number of files written.
    pub files_changed: usize,
    /// The total number of replacements applied (in the files written).
    pub replacements: usize,
    /// The files that had matches but could not be written back, left as they
    /// were; empty when every write succeeded.
    pub failures: Vec<WriteFailure>,
}

/// A file a workspace [`replace`](WorkspaceSearch::replace) could not write back.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WriteFailure {
    /// The file that was not written.
    pub path: PathBuf,
    /// Why the write failed, as the operating system reported it.
    pub error: String,
}

/// Build the gitignore-aware workspace walk (shared by every walking entry point).
fn build_walk(
    root: &Path,
    includes: &[String],
    excludes: &[String],
) -> Result<ignore::Walk, SearchError> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.standard_filters(true);
    // Honor `.gitignore` even outside a git repository (matches editor expectations
    // and keeps non-repo workspaces filtered).
    builder.require_git(false);
    // Never follow symlinks (a cycle must not stall the walk) and always prune
    // the heavyweight dirs, even when no ignore file mentions them.
    builder.follow_links(false);
    builder.filter_entry(|entry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !IGNORED_DIRS.contains(&name))
    });
    if !includes.is_empty() || !excludes.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        for inc in includes {
            overrides
                .add(inc)
                .map_err(|_| SearchError::InvalidPattern)?;
        }
        for exc in excludes {
            // `!glob` excludes in override syntax.
            overrides
                .add(&format!("!{exc}"))
                .map_err(|_| SearchError::InvalidPattern)?;
        }
        let overrides = overrides.build().map_err(|_| SearchError::InvalidPattern)?;
        builder.overrides(overrides);
    }
    Ok(builder.build())
}

/// Read a walked entry as UTF-8 text, or `None` if it is not a searchable file
/// (a directory, oversize, binary, or non-UTF-8).
fn read_searchable(entry: &ignore::DirEntry) -> Option<String> {
    if !entry.file_type().is_some_and(|t| t.is_file()) {
        return None;
    }
    if entry.metadata().is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        return None;
    }
    let bytes = std::fs::read(entry.path()).ok()?;
    // Skip binary files: a NUL byte in the head, or invalid UTF-8.
    let head = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if head.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
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
