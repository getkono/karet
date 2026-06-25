//! `karet-vcs` — editor-oriented git integration for karet.
//!
//! A `gix`-backed engine for status, blame, branches and staging, emitting
//! per-line change markers and blame annotations as neutral `karet-core`
//! [`Decoration`]s. Headless by default; the ratatui source-control panels live
//! behind the `view` feature (and render `karet-diff` hunk data directly).
//!
//! This is the implementation *skeleton*: the public joints are defined; the gix
//! logic is filled in separately.

use karet_core::Decoration;
use std::path::{Path, PathBuf};

mod changes;
mod repo;
mod selection;

pub use changes::FileChange;
pub use selection::Selection;

/// Errors produced by the VCS engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VcsError {
    /// No git repository was found.
    #[error("not a git repository")]
    NotARepository,
    /// A git operation failed.
    #[error("git error: {0}")]
    Git(String),
}

/// The change state of a file in the working tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StatusKind {
    /// A newly added (tracked) file.
    Added,
    /// A modified file.
    Modified,
    /// A deleted file.
    Deleted,
    /// A renamed file.
    Renamed,
    /// An untracked file.
    Untracked,
}

/// One file's working-tree status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStatus {
    /// The file path, relative to the repository root.
    pub path: PathBuf,
    /// The change kind.
    pub kind: StatusKind,
    /// Whether the change is staged (in the index).
    pub staged: bool,
}

/// The working-tree status: the set of changed files.
#[derive(Clone, Debug, Default)]
pub struct WorkingTreeStatus {
    /// The changed files.
    pub entries: Vec<FileStatus>,
}

/// One line of blame information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// The 0-based line.
    pub line: u32,
    /// The commit id (short hash).
    pub commit: String,
    /// The commit author.
    pub author: String,
}

/// A git branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// The branch name.
    pub name: String,
    /// Whether this branch is currently checked out.
    pub is_head: bool,
}

/// An editor-oriented handle to a git repository.
pub struct Repository {
    /// The underlying `gix` repository. Note: `gix::Repository` is not `Sync`.
    inner: gix::Repository,
}

impl Repository {
    /// The current working-tree status.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn status(&self) -> Result<WorkingTreeStatus, VcsError> {
        todo!()
    }

    /// Per-line change markers for `path`, as gutter decorations.
    #[must_use]
    pub fn gutter_decorations(&self, path: &Path) -> Vec<Decoration> {
        let _ = path;
        todo!()
    }

    /// Per-line blame for `path`.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn blame(&self, path: &Path) -> Result<Vec<BlameLine>, VcsError> {
        let _ = path;
        todo!()
    }

    /// Inline, age-shaded blame decorations for `path`.
    #[must_use]
    pub fn blame_decorations(&self, path: &Path) -> Vec<Decoration> {
        let _ = path;
        todo!()
    }

    /// The repository's branches.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn branches(&self) -> Result<Vec<Branch>, VcsError> {
        todo!()
    }

    /// Stage `path` (add it to the index).
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn stage(&self, path: &Path) -> Result<(), VcsError> {
        let _ = path;
        todo!()
    }

    /// Unstage `path` (remove it from the index).
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn unstage(&self, path: &Path) -> Result<(), VcsError> {
        let _ = path;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_kinds_compare() {
        assert_eq!(StatusKind::Modified, StatusKind::Modified);
        assert_ne!(StatusKind::Added, StatusKind::Deleted);
    }

    #[test]
    fn error_displays() {
        assert_eq!(VcsError::NotARepository.to_string(), "not a git repository");
    }
}
