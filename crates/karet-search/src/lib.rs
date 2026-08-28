//! `karet-search` — code search & replace for the karet toolkit.
//!
//! A ripgrep-style engine usable standalone (depends on no other karet crate):
//! in-file search plus a gitignore-aware workspace walk with streamed results.
//! Positions are reported as plain byte offsets plus 0-based line/column so the
//! crate stays dependency-light; an integrator maps them to its own coordinate
//! types.
//!
//! Search and replace (both in-file and workspace) are implemented via
//! [`search_in_file`]/[`WorkspaceSearch::run`] and
//! [`plan_replacements`]/[`apply_replacements`]/[`WorkspaceSearch::replace`]. The
//! walk itself is also public as [`walk_text_files`], for consumers that want the
//! same filtered corpus without a pattern. The workspace walk is currently
//! single-threaded (a parallel walk is a deferred optimization).

use std::ops::ControlFlow;
use std::path::Path;
use std::path::PathBuf;

use regex::Regex;
use regex::RegexBuilder;

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
pub struct Matcher(MatcherKind);

/// A compiled query: a literal needle (fast-path) or a regex.
enum MatcherKind {
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
        if !query.regex && query.case_sensitive && !query.whole_word {
            Ok(Self(MatcherKind::Literal(query.pattern.clone())))
        } else {
            Ok(Self(MatcherKind::Regex(compile(query)?)))
        }
    }

    /// Find every match in `text`.
    ///
    /// An empty pattern never matches, matching [`search_in_file`]'s behavior.
    #[must_use]
    pub fn find(&self, text: &str) -> Vec<Match> {
        match &self.0 {
            MatcherKind::Literal(needle) if needle.is_empty() => Vec::new(),
            MatcherKind::Literal(needle) => literal_matches(text, needle),
            MatcherKind::Regex(re) => regex_matches(text, re),
        }
    }

    /// Plan a [`Replacement`] for every match in `text`. When `expand` is set (a
    /// regex query), the regex `$1` / `${name}` / `$0` substitutions are expanded
    /// against each match's captures; otherwise `replacement` is inserted literally
    /// (so a literal or whole-word query never mis-reads a `$` in the replacement).
    fn plan(&self, text: &str, replacement: &str, expand: bool) -> Vec<Replacement> {
        match &self.0 {
            MatcherKind::Literal(needle) if needle.is_empty() => Vec::new(),
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
                if std::fs::write(path, updated).is_ok() {
                    summary.files_changed += 1;
                    summary.replacements += plan.len();
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReplaceSummary {
    /// The number of files written.
    pub files_changed: usize,
    /// The total number of replacements applied.
    pub replacements: usize,
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

    /// The whole point of the public matcher: compile once, run many. A reused
    /// matcher must agree with the one-shot `search_in_file` on every buffer.
    #[test]
    fn a_reused_matcher_agrees_with_search_in_file() {
        for query in [
            literal("foo"),
            SearchQuery {
                pattern: "fo+".into(),
                regex: true,
                ..Default::default()
            },
            SearchQuery {
                pattern: "foo".into(),
                whole_word: true,
                ..Default::default()
            },
            SearchQuery {
                pattern: "FOO".into(),
                ..Default::default()
            },
        ] {
            let matcher = match Matcher::new(&query) {
                Ok(matcher) => matcher,
                Err(_) => continue,
            };
            for text in ["foo bar foo", "nothing", "foofoo", "a foo\nfoo b", ""] {
                assert_eq!(
                    matcher.find(text),
                    search_in_file(text, &query).unwrap_or_default(),
                    "pattern {:?} over {text:?}",
                    query.pattern,
                );
            }
        }
    }

    #[test]
    fn a_matcher_rejects_an_invalid_regex() {
        let q = SearchQuery {
            pattern: "(".into(),
            regex: true,
            ..Default::default()
        };
        assert!(Matcher::new(&q).is_err());
    }

    /// An empty pattern never matches, so a caller that reuses one matcher over a
    /// whole tree cannot accidentally report every file as a hit.
    #[test]
    fn a_matcher_built_from_an_empty_pattern_never_matches() {
        let matcher = Matcher::new(&literal(""));
        assert_eq!(
            matcher.map(|m| m.find("anything at all").len()).ok(),
            Some(0)
        );
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

    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A temp directory removed on drop (mirrors the karet-vcs test pattern; no
    /// `tempfile` dev-dependency).
    struct TempDir {
        path: PathBuf,
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Create a fresh temp directory.
    fn temp_dir() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("karet-search-{}-{}", std::process::id(), n));
        let _ = std::fs::create_dir_all(&path);
        TempDir { path }
    }

    /// Write `contents` to `dir/rel`, creating parent directories.
    fn write(dir: &Path, rel: &str, contents: &[u8]) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
    }

    /// Collect every hit from a workspace search into a path-sorted vector.
    fn collect(root: &Path, query: &SearchQuery) -> Vec<FileHit> {
        let mut hits = Vec::new();
        let _ = WorkspaceSearch::new().run(root, query, |hit| hits.push(hit));
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        hits
    }

    #[test]
    fn workspace_search_finds_matching_files() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"needle here\nand again needle\n");
        write(&dir.path, "sub/b.txt", b"needle in subdir\n");
        write(&dir.path, "c.txt", b"nothing of interest\n");

        let hits = collect(&dir.path, &literal("needle"));
        assert_eq!(hits.len(), 2);
        // a.txt has two matches; the subdir file one.
        let total: usize = hits.iter().map(|h| h.matches.len()).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn workspace_search_respects_gitignore() {
        let dir = temp_dir();
        write(&dir.path, ".gitignore", b"ignored.txt\n");
        write(&dir.path, "kept.txt", b"needle\n");
        write(&dir.path, "ignored.txt", b"needle\n");

        let hits = collect(&dir.path, &literal("needle"));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("kept.txt"));
    }

    #[test]
    fn workspace_search_prunes_ignored_dirs_without_a_gitignore() {
        let dir = temp_dir();
        write(&dir.path, "kept.txt", b"needle\n");
        // No .gitignore anywhere: pruning must come from IGNORED_DIRS itself.
        write(&dir.path, "target/debug/build.log", b"needle\n");
        write(&dir.path, "node_modules/pkg/index.js", b"needle\n");

        let hits = collect(&dir.path, &literal("needle"));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("kept.txt"));
    }

    #[test]
    fn workspace_search_skips_binary_files() {
        let dir = temp_dir();
        write(&dir.path, "text.txt", b"needle\n");
        write(&dir.path, "blob.bin", b"needle\x00\x01needle");

        let hits = collect(&dir.path, &literal("needle"));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("text.txt"));
    }

    #[test]
    fn workspace_search_applies_include_globs() {
        let dir = temp_dir();
        write(&dir.path, "keep.rs", b"needle\n");
        write(&dir.path, "skip.md", b"needle\n");

        let query = SearchQuery {
            pattern: "needle".into(),
            case_sensitive: true,
            includes: vec!["*.rs".into()],
            ..Default::default()
        };
        let hits = collect(&dir.path, &query);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("keep.rs"));
    }

    #[test]
    fn literal_replace_plans_and_applies() {
        let text = "foo bar foo";
        let plan = plan_replacements(text, &literal("foo"), "baz").unwrap_or_default();
        assert_eq!(plan.len(), 2);
        assert_eq!(apply_replacements(text, &plan), "baz bar baz");
    }

    #[test]
    fn regex_replace_expands_capture_groups() {
        let q = SearchQuery {
            pattern: r"(\w+)=(\d+)".into(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let plan = plan_replacements("a=1 b=2", &q, "$2=$1").unwrap_or_default();
        assert_eq!(apply_replacements("a=1 b=2", &plan), "1=a 2=b");
    }

    #[test]
    fn non_regex_replacement_is_literal_even_with_dollar() {
        // A whole-word (non-regex) query compiles to a regex internally, but a `$1`
        // in the replacement must be inserted verbatim, not treated as a capture.
        let q = SearchQuery {
            pattern: "x".into(),
            whole_word: true,
            case_sensitive: true,
            ..Default::default()
        };
        let plan = plan_replacements("x y x", &q, "$1").unwrap_or_default();
        assert_eq!(apply_replacements("x y x", &plan), "$1 y $1");
    }

    #[test]
    fn apply_is_offset_safe_for_length_changing_edits() {
        // Replacements of differing lengths must not corrupt neighbours (right-to-left).
        let text = "aa bb aa";
        let plan = plan_replacements(text, &literal("aa"), "wide").unwrap_or_default();
        assert_eq!(apply_replacements(text, &plan), "wide bb wide");
    }

    #[test]
    fn empty_pattern_plans_nothing() {
        assert!(
            plan_replacements("abc", &literal(""), "z")
                .unwrap_or_default()
                .is_empty()
        );
    }

    #[test]
    fn workspace_replace_writes_matching_files_only() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"needle and needle\n");
        write(&dir.path, "b.txt", b"nothing\n");
        write(&dir.path, ".gitignore", b"ignored.txt\n");
        write(&dir.path, "ignored.txt", b"needle\n");

        let summary = WorkspaceSearch::new()
            .replace(&dir.path, &literal("needle"), "pin")
            .unwrap_or_default();
        assert_eq!(summary.files_changed, 1);
        assert_eq!(summary.replacements, 2);
        assert_eq!(
            std::fs::read_to_string(dir.path.join("a.txt")).unwrap_or_default(),
            "pin and pin\n"
        );
        // The gitignored file is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path.join("ignored.txt")).unwrap_or_default(),
            "needle\n"
        );
    }

    /// Collect every file the walk visits, as root-relative path + contents.
    fn walked(root: &Path, includes: &[String], excludes: &[String]) -> Vec<(String, String)> {
        let mut files = Vec::new();
        let _ = walk_text_files(root, includes, excludes, |path, text| {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            files.push((rel, text));
            ControlFlow::Continue(())
        });
        files.sort();
        files
    }

    #[test]
    fn walk_visits_text_files_and_honors_gitignore() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"alpha\n");
        write(&dir.path, "sub/b.txt", b"beta\n");
        write(&dir.path, ".gitignore", b"ignored.txt\n");
        write(&dir.path, "ignored.txt", b"gamma\n");

        let files = walked(&dir.path, &[], &[]);
        let names: Vec<&str> = files.iter().map(|(rel, _)| rel.as_str()).collect();
        assert!(names.contains(&"a.txt"), "{names:?}");
        assert!(names.contains(&"sub/b.txt"), "{names:?}");
        assert!(!names.contains(&"ignored.txt"), "{names:?}");
        assert_eq!(
            files.iter().find(|(rel, _)| rel == "a.txt").map(|(_, t)| t),
            Some(&"alpha\n".to_owned()),
            "the walk hands the sink the full file contents"
        );
    }

    #[test]
    fn walk_prunes_heavyweight_dirs_and_binary_files() {
        let dir = temp_dir();
        write(&dir.path, "keep.txt", b"text\n");
        // Pruned by IGNORED_DIRS even with no .gitignore mentioning them.
        write(&dir.path, "target/built.txt", b"artifact\n");
        write(&dir.path, "node_modules/dep.txt", b"vendored\n");
        // A NUL byte in the head marks the file binary.
        write(&dir.path, "blob.bin", b"pre\0post\n");

        let names: Vec<String> = walked(&dir.path, &[], &[])
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        assert_eq!(names, vec!["keep.txt".to_owned()]);
    }

    #[test]
    fn walk_applies_include_and_exclude_globs() {
        let dir = temp_dir();
        write(&dir.path, "a.rs", b"rust\n");
        write(&dir.path, "b.md", b"markdown\n");
        write(&dir.path, "skip.rs", b"rust\n");

        let names: Vec<String> = walked(&dir.path, &["*.rs".to_owned()], &["skip.rs".to_owned()])
            .into_iter()
            .map(|(rel, _)| rel)
            .collect();
        assert_eq!(names, vec!["a.rs".to_owned()]);
    }

    #[test]
    fn walk_stops_early_on_break() {
        let dir = temp_dir();
        for i in 0..5 {
            write(&dir.path, &format!("f{i}.txt"), b"x\n");
        }
        let mut seen = 0_usize;
        let _ = walk_text_files(&dir.path, &[], &[], |_, _| {
            seen += 1;
            // A result cap / cancelled request stops the walk mid-tree.
            if seen == 2 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(seen, 2);
    }

    #[test]
    fn walk_surfaces_an_invalid_glob() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"x\n");
        let result = walk_text_files(&dir.path, &["[".to_owned()], &[], |_, _| {
            ControlFlow::Continue(())
        });
        assert_eq!(result, Err(SearchError::InvalidPattern));
    }

    #[test]
    fn workspace_search_surfaces_invalid_pattern() {
        let dir = temp_dir();
        write(&dir.path, "a.txt", b"x\n");
        let query = SearchQuery {
            pattern: "(".into(),
            regex: true,
            ..Default::default()
        };
        let result = WorkspaceSearch::new().run(&dir.path, &query, |_| {});
        assert_eq!(result, Err(SearchError::InvalidPattern));
    }
}
