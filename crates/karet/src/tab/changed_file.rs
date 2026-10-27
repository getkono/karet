//! The changed files the diff, commit and compare tabs show, and the
//! Source-Control group a single-file diff belongs to.
//!
//! Diffing, syntax highlighting, and intra-line pairing all happen in the session
//! backend (off the UI thread); a [`ChangedFile`] holds the prepared result. Its
//! painters live in `ui::content::diff`, which fills the per-theme row cache
//! kept here.

use std::cell::RefCell;

use karet_session::PreparedChange;
use karet_theme::Theme;
use ratatui::text::Line;

/// Which Source-Control group a changed file belongs to, mirroring VS Code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// `HEAD` vs the index: the staged changes.
    Staged,
    /// The index vs the worktree (and untracked files): the working-tree changes.
    Working,
}

/// A changed file ready for display: the backend-prepared diff plus a per-theme
/// cache of the assembled unified rows. (The Source-Control group lives on the
/// tab — [`crate::tab::TabKind::Diff`] — not the file.)
pub struct ChangedFile {
    /// The prepared change delivered by the backend (identity, language, diff).
    pub change: PreparedChange,
    /// Unified rows cached per theme snapshot, so scrolling does not re-assemble
    /// spans every frame. Live theme reloads rebuild correctly. Filled only by
    /// the diff painters.
    pub(crate) unified_cache: RefCell<Option<(Theme, Vec<Line<'static>>)>>,
    /// Whether the code-review store marks this file reviewed (see
    /// `crate::app::review`).
    pub reviewed: bool,
}

impl ChangedFile {
    /// Wrap a backend-prepared change for display.
    pub fn new(change: PreparedChange) -> Self {
        Self {
            change,
            unified_cache: RefCell::new(None),
            reviewed: false,
        }
    }

    /// The display language name (e.g. `"Rust"`).
    #[must_use]
    pub fn language(&self) -> &str {
        &self.change.language
    }

    /// The 1-based first changed line in the file's new text (see
    /// [`karet_diff::PreparedDiff::first_changed_line`]).
    #[must_use]
    pub fn first_changed_line(&self) -> Option<u32> {
        self.change.diff.first_changed_line()
    }

    /// The `(added, removed)` line counts for the per-file `+N −M` summary.
    #[must_use]
    pub fn line_stats(&self) -> (usize, usize) {
        self.change.diff.line_stats()
    }
}

/// Build a test [`ChangedFile`] by diffing two texts locally (plaintext, no tokens).
#[cfg(test)]
pub(crate) fn test_changed_file(path: &str, old: &str, new: &str) -> ChangedFile {
    let diff = karet_diff::diff_text(
        old,
        new,
        &karet_diff::DiffOptions {
            path_hint: Some(path.to_string()),
            ..Default::default()
        },
    );
    ChangedFile::new(PreparedChange {
        path: std::path::PathBuf::from(path),
        old_path: None,
        status: karet_vcs::StatusKind::Modified,
        language: "plaintext".to_string(),
        diff: karet_diff::PreparedDiff::new(diff, Vec::new(), Vec::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_changed_line_points_at_the_first_addition() {
        let fv = test_changed_file("notes.txt", "a\nb\nc\nd\n", "a\nb\nx\nd\n");
        assert_eq!(fv.first_changed_line(), Some(3));
    }
}
