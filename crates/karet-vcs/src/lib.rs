//! `karet-vcs` — editor-oriented git integration for karet.
//!
//! A `gix`-backed engine for repository reads with argument-safe `git` subprocesses
//! for mature write operations, emitting
//! per-line change markers and blame annotations as neutral `karet-core`
//! [`Decoration`]s. Headless by default; the ratatui source-control panels live
//! behind the `view` feature (and render `karet-diff` hunk data directly).
//!
//! The write path requires the `git` executable on `PATH`; it never invokes a shell.

use std::path::Path;
use std::path::PathBuf;

use karet_core::Decoration;

mod branch;
mod changes;
mod detail;
mod log;
mod remote;
mod repo;
mod selection;
mod stash;
mod summary;
#[cfg(test)]
mod test_support;
mod write;

pub use branch::BranchTarget;
pub use branch::CreateBranchOptions;
pub use branch::UndoCommitOutcome;
pub use changes::FileChange;
pub use detail::CommitDetail;
pub use detail::CommitSignature;
pub use detail::Identity;
pub use detail::SignatureKind;
pub use log::Commit;
pub use remote::Remote;
pub use remote::RemoteBranch;
pub use remote::RepositoryOperation;
pub use remote::RepositoryState;
pub use remote::SyncOutcome;
pub use selection::Selection;
pub use stash::StashEntry;
pub use stash::StashOptions;
pub use summary::RepositorySummary;

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
    /// The `git` executable required for a write or network operation was unavailable.
    #[error("git executable is unavailable: {0}")]
    GitUnavailable(String),
    /// An otherwise valid destructive action requires explicit confirmation.
    #[error("confirmation required: {0}")]
    ConfirmationRequired(String),
    /// Legacy error retained for source compatibility with the former optional writer.
    #[error("the requested VCS feature is disabled")]
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

    /// The repository's local branches, sorted by name. Each carries whether it is the
    /// currently checked-out branch. A branch name is itself a valid revision, so it
    /// can be passed straight to [`file_at_rev`](Self::file_at_rev) or the diff readers.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure to read the references or the head.
    pub fn branches(&self) -> Result<Vec<Branch>, VcsError> {
        use gix::bstr::ByteSlice;

        use crate::repo::to_git;

        let head = self.current_branch()?;
        let platform = self.inner.references().map_err(to_git)?;
        let iter = platform.local_branches().map_err(to_git)?;
        let mut out = Vec::new();
        for reference in iter {
            let reference = reference.map_err(to_git)?;
            let name = reference.name().shorten().to_str_lossy().into_owned();
            let is_head = head.as_deref() == Some(name.as_str());
            out.push(Branch { name, is_head });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    /// The short name of the branch `HEAD` symbolically points to (e.g. `main`), or
    /// `None` when `HEAD` is detached. An **unborn** branch (a fresh repository with
    /// no commits yet) still has a symbolic name, so it is returned — callers that
    /// need a resolvable revision should also check [`head_hash`](Self::head_hash).
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure to read the head.
    pub fn current_branch(&self) -> Result<Option<String>, VcsError> {
        use gix::bstr::ByteSlice;

        use crate::repo::to_git;

        Ok(self
            .inner
            .head_name()
            .map_err(to_git)?
            .map(|name| name.shorten().to_str_lossy().into_owned()))
    }

    /// Stage `paths` (add their current worktree state to the index). A path that
    /// no longer exists in the worktree is staged as a deletion.
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure.
    pub fn stage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        self.git_stage(paths)
    }

    /// Unstage `paths` (reset their index entries to `HEAD`, or remove them when
    /// there is no commit yet).
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure.
    pub fn unstage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        self.git_unstage(paths)
    }

    /// Discard the working-tree changes to `paths`: tracked files are restored to
    /// `HEAD`, staged-new files are un-added and removed, and untracked files are
    /// deleted. This is destructive and cannot be undone.
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure.
    pub fn discard(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        self.git_discard(paths)
    }

    /// Stage every change in the worktree (new, modified, and deleted files).
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure.
    pub fn stage_all(&self) -> Result<(), VcsError> {
        self.git_stage_all()
    }

    /// Unstage every staged change (reset the whole index to `HEAD`).
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure.
    pub fn unstage_all(&self) -> Result<(), VcsError> {
        self.git_unstage_all()
    }

    /// Commit the staged changes with `message`, returning the new commit's hex id.
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] on failure (including unresolved conflicts or a missing
    /// `user.name`/`user.email` identity).
    pub fn commit(&self, message: &str) -> Result<String, VcsError> {
        self.git_commit(message)
    }

    /// The staged changes as a unified diff plus a `--stat` summary and file count.
    ///
    /// This is the input an external commit-message generator needs; the diff is
    /// taken between `HEAD` (or the empty tree on an unborn branch) and the index.
    ///
    /// # Errors
    /// Returns [`VcsError::GitUnavailable`] when `git` cannot be launched, or
    /// [`VcsError::Git`] if the diff cannot be computed.
    pub fn staged_diff(&self) -> Result<StagedDiff, VcsError> {
        self.git_staged_diff()
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
