//! `karet-vcs` — editor-oriented git integration for karet.
//!
//! A `gix`-backed engine for status, blame, branches and staging, emitting
//! per-line change markers and blame annotations as neutral `karet-core`
//! [`Decoration`]s. Headless by default; the ratatui source-control panels live
//! behind the `view` feature (and render `karet-diff` hunk data directly).
//!
//! This is the implementation *skeleton*: the public joints are defined; the gix
//! logic is filled in separately.

use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;

mod changes;
mod detail;
mod log;
mod repo;
mod selection;
#[cfg(feature = "git2")]
mod write;

pub use changes::FileChange;
pub use detail::CommitDetail;
pub use detail::CommitSignature;
pub use detail::Identity;
pub use detail::SignatureKind;
pub use log::Commit;
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
    /// A write action was requested but the `git2` feature is not enabled.
    #[error("staging requires the `git2` feature")]
    FeatureDisabled,
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
    /// A file with unresolved merge conflicts.
    Conflicted,
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

/// The staged changes rendered as a unified diff, for feeding an external
/// commit-message generator (`git diff --cached`, without a `git` subprocess).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StagedDiff {
    /// The unified-diff text of every staged change.
    pub patch: String,
    /// A `--stat`-style per-file summary (files changed, insertions, deletions).
    pub stat: String,
    /// The number of files the staged diff touches.
    pub file_count: usize,
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
    /// The underlying `gix` repository (status and diff reads). Note:
    /// `gix::Repository` is not `Sync`.
    inner: gix::Repository,
    /// The `libgit2` handle backing the write actions (stage/discard/commit).
    /// Opened from the same path as `inner`, so it resolves the same (possibly
    /// linked-worktree) repository.
    #[cfg(feature = "git2")]
    git2: git2::Repository,
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

    /// Stage `paths` (add their current worktree state to the index). A path that
    /// no longer exists in the worktree is staged as a deletion.
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure.
    pub fn stage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_stage(paths)
        }
        #[cfg(not(feature = "git2"))]
        {
            let _ = paths;
            Err(VcsError::FeatureDisabled)
        }
    }

    /// Unstage `paths` (reset their index entries to `HEAD`, or remove them when
    /// there is no commit yet).
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure.
    pub fn unstage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_unstage(paths)
        }
        #[cfg(not(feature = "git2"))]
        {
            let _ = paths;
            Err(VcsError::FeatureDisabled)
        }
    }

    /// Discard the working-tree changes to `paths`: tracked files are restored to
    /// `HEAD`, staged-new files are un-added and removed, and untracked files are
    /// deleted. This is destructive and cannot be undone.
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure.
    pub fn discard(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_discard(paths)
        }
        #[cfg(not(feature = "git2"))]
        {
            let _ = paths;
            Err(VcsError::FeatureDisabled)
        }
    }

    /// Stage every change in the worktree (new, modified, and deleted files).
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure.
    pub fn stage_all(&self) -> Result<(), VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_stage_all()
        }
        #[cfg(not(feature = "git2"))]
        {
            Err(VcsError::FeatureDisabled)
        }
    }

    /// Unstage every staged change (reset the whole index to `HEAD`).
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure.
    pub fn unstage_all(&self) -> Result<(), VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_unstage_all()
        }
        #[cfg(not(feature = "git2"))]
        {
            Err(VcsError::FeatureDisabled)
        }
    }

    /// Commit the staged changes with `message`, returning the new commit's hex id.
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] on failure (including unresolved conflicts or a missing
    /// `user.name`/`user.email` identity).
    pub fn commit(&self, message: &str) -> Result<String, VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_commit(message)
        }
        #[cfg(not(feature = "git2"))]
        {
            let _ = message;
            Err(VcsError::FeatureDisabled)
        }
    }

    /// The staged changes as a unified diff plus a `--stat` summary and file count.
    ///
    /// This is the input an external commit-message generator needs; the diff is
    /// taken between `HEAD` (or the empty tree on an unborn branch) and the index.
    ///
    /// # Errors
    /// Returns [`VcsError::FeatureDisabled`] if the `git2` feature is off, or
    /// [`VcsError::Git`] if the diff cannot be computed.
    pub fn staged_diff(&self) -> Result<StagedDiff, VcsError> {
        #[cfg(feature = "git2")]
        {
            self.git2_staged_diff()
        }
        #[cfg(not(feature = "git2"))]
        {
            Err(VcsError::FeatureDisabled)
        }
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
