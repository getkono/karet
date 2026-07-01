//! `blameline` — semantic git blame with structured output.
//!
//! An enhanced `git blame` that surfaces *why* lines changed, not just who and when.
//! Consecutive lines introduced by the same commit are grouped into a single
//! [`BlameGroup`]; each group carries the **full** commit message (not just the
//! hash); and blame can be narrowed to the function or block enclosing a line via
//! tree-sitter. Every model derives [`serde::Serialize`], so the issue's
//! `{lines, commit_hash, message, author, date}` shape is one [`to_json`] call away
//! — ready to feed to another tool or an AI agent.
//!
//! # Backend
//! Git data comes from [`gix`](gix) (gitoxide) with its `blame` feature — pure Rust,
//! no external `git` process.
//!
//! # Versioning
//! Unlike the rest of the karet workspace (which releases in lockstep), `blameline`
//! follows its **own** SemVer line starting at `1.0.0`. See the crate `README.md`.

mod blame;
mod treesitter;

use std::path::Path;

pub use blame::blame_file;
pub use treesitter::enclosing_function_range;

/// An inclusive, 1-based line range (e.g. the issue's "lines 42-58").
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LineRange {
    /// First line of the range (1-based, inclusive).
    pub start: u32,
    /// Last line of the range (1-based, inclusive).
    pub end: u32,
}

/// A run of consecutive lines attributed to a single commit — the unit of semantic
/// blame. Serializes to `{lines, commit_hash, message, author, date}`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlameGroup {
    /// The grouped, consecutive lines (1-based, inclusive).
    pub lines: LineRange,
    /// The full hex id of the commit that introduced these lines.
    pub commit_hash: String,
    /// The commit's full message (subject and body).
    pub message: String,
    /// The commit author's name.
    pub author: String,
    /// The commit's author date, ISO-8601 (falls back to the raw git timestamp).
    pub date: String,
}

impl BlameGroup {
    /// An abbreviated, 7-character commit hash for display.
    #[must_use]
    pub fn short_hash(&self) -> &str {
        let n = self.commit_hash.len().min(7);
        &self.commit_hash[..n]
    }

    /// The first line (subject) of the commit message.
    #[must_use]
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// Errors produced by the blame engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BlameError {
    /// No git repository was found at the given root.
    #[error("not a git repository")]
    NotARepository,
    /// The file exists on disk but has no committed history in `HEAD` yet
    /// (e.g. a new or staged-but-uncommitted file).
    #[error("no committed history for {0} yet")]
    NotCommitted(String),
    /// A git operation failed.
    #[error("git error: {0}")]
    Git(String),
    /// Serializing the blame groups to JSON failed.
    #[error("json error: {0}")]
    Json(String),
}

/// Semantic blame narrowed to the function/block enclosing `line` (0-based).
///
/// Parses `source` with tree-sitter to find the innermost enclosing function, then
/// clips whole-file blame to that line range. Falls back to the whole file when the
/// language is unsupported or no enclosing function is found.
///
/// # Errors
/// Returns [`BlameError`] if the repository can't be opened or blamed — including
/// [`BlameError::NotCommitted`] when the file has no committed history in `HEAD` yet.
pub fn blame_function(
    repo_root: &Path,
    file: &Path,
    source: &str,
    line: u32,
) -> Result<Vec<BlameGroup>, BlameError> {
    let all = blame_file(repo_root, file)?;
    match enclosing_function_range(source, file, line) {
        Some(range) => Ok(clip_groups(all, range)),
        None => Ok(all),
    }
}

/// Serialize blame groups to pretty JSON (`[{lines, commit_hash, message, …}, …]`).
///
/// # Errors
/// Returns [`BlameError::Json`] if serialization fails.
pub fn to_json(groups: &[BlameGroup]) -> Result<String, BlameError> {
    serde_json::to_string_pretty(groups).map_err(|e| BlameError::Json(e.to_string()))
}

/// Restrict `groups` to the lines within `range`, clipping group boundaries and
/// dropping groups that fall entirely outside it.
fn clip_groups(groups: Vec<BlameGroup>, range: LineRange) -> Vec<BlameGroup> {
    groups
        .into_iter()
        .filter_map(|mut g| {
            let start = g.lines.start.max(range.start);
            let end = g.lines.end.min(range.end);
            if start > end {
                None
            } else {
                g.lines = LineRange { start, end };
                Some(g)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(start: u32, end: u32, hash: &str) -> BlameGroup {
        BlameGroup {
            lines: LineRange { start, end },
            commit_hash: hash.to_string(),
            message: "subject line\n\nbody".to_string(),
            author: "Jane Doe".to_string(),
            date: "2026-03-16T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn short_hash_and_summary() {
        let g = group(1, 3, "abc1234567890");
        assert_eq!(g.short_hash(), "abc1234");
        assert_eq!(g.summary(), "subject line");
    }

    #[test]
    fn short_hash_handles_tiny_hashes() {
        let g = group(1, 1, "abc");
        assert_eq!(g.short_hash(), "abc");
    }

    #[test]
    fn clip_keeps_overlap_and_drops_outside() {
        let groups = vec![group(1, 10, "a"), group(11, 20, "b"), group(21, 30, "c")];
        let clipped = clip_groups(groups, LineRange { start: 8, end: 22 });
        assert_eq!(clipped.len(), 3);
        assert_eq!(clipped[0].lines, LineRange { start: 8, end: 10 });
        assert_eq!(clipped[1].lines, LineRange { start: 11, end: 20 });
        assert_eq!(clipped[2].lines, LineRange { start: 21, end: 22 });
    }

    #[test]
    fn clip_drops_non_overlapping_groups() {
        let groups = vec![group(1, 5, "a"), group(100, 200, "b")];
        let clipped = clip_groups(
            groups,
            LineRange {
                start: 50,
                end: 150,
            },
        );
        assert_eq!(clipped.len(), 1);
        assert_eq!(clipped[0].commit_hash, "b");
        assert_eq!(
            clipped[0].lines,
            LineRange {
                start: 100,
                end: 150
            }
        );
    }

    #[test]
    fn json_shape_matches_issue() -> Result<(), BlameError> {
        let json = to_json(&[group(42, 58, "abc123")])?;
        // Field names line up with the issue's structured output.
        assert!(json.contains("\"lines\""));
        assert!(json.contains("\"commit_hash\""));
        assert!(json.contains("\"message\""));
        assert!(json.contains("\"author\""));
        assert!(json.contains("\"date\""));
        assert!(json.contains("\"start\": 42"));
        assert!(json.contains("\"end\": 58"));
        Ok(())
    }
}
