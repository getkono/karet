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

/// Errors produced by search/replace.
#[derive(Debug, thiserror::Error)]
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

/// Search `text` for `query`, returning every match.
///
/// # Errors
/// Returns [`SearchError::InvalidPattern`] for a malformed regex.
pub fn search_in_file(text: &str, query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
    let _ = (text, query);
    todo!()
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
}
