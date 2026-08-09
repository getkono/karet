//! The source-control slice of the backend vocabulary: repository snapshots,
//! serialized actions, and prepared-diff payloads.

use super::*;

/// A complete repository snapshot for Source Control controls and pickers.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepositorySnapshot {
    /// Current branch, upstream divergence, and recovery state.
    pub state: RepositoryState,
    /// Local branches.
    pub branches: Vec<Branch>,
    /// Configured remotes.
    pub remotes: Vec<Remote>,
    /// Locally known remote-tracking branches.
    pub remote_branches: Vec<RemoteBranch>,
    /// Stash entries, newest first.
    pub stashes: Vec<StashEntry>,
}

/// A forge-neutral open pull request suitable for the branch picker.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PullRequestSummary {
    /// Repository-local pull request number.
    pub number: u64,
    /// Pull request title.
    pub title: String,
    /// Author login, when available.
    pub author: Option<String>,
    /// Whether the pull request is a draft.
    pub draft: bool,
    /// Source branch name.
    pub head_ref: String,
    /// Source repository, including fork owner.
    pub head_repo: String,
    /// Current source commit.
    pub head_sha: String,
    /// Target branch name.
    pub base_ref: String,
    /// Target repository.
    pub base_repo: String,
    /// Browser URL.
    pub url: String,
}

/// One serialized repository mutation. The backend runs these off the actor thread.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum VcsAction {
    /// Create, optionally switch to, and optionally publish a branch.
    CreateBranch(CreateBranchOptions),
    /// Switch to a local or remote-tracking branch.
    SwitchBranch(BranchTarget),
    /// Rename a local branch.
    RenameBranch {
        /// Existing local name.
        old: String,
        /// Replacement local name.
        new: String,
    },
    /// Safely delete a merged local branch.
    DeleteBranch {
        /// Local branch to delete.
        name: String,
    },
    /// Publish a local branch.
    PublishBranch {
        /// Destination remote.
        remote: String,
        /// Local branch to publish.
        branch: String,
        /// Whether to configure the published branch as upstream.
        set_upstream: bool,
    },
    /// Delete a remote branch.
    DeleteRemoteBranch {
        /// Destination remote.
        remote: String,
        /// Remote branch to delete.
        branch: String,
    },
    /// Undo the latest commit with a soft reset.
    UndoCommit {
        /// Explicit confirmation when the commit is already upstream.
        allow_upstream: bool,
    },
    /// Create a stash.
    StashPush(StashOptions),
    /// Load a stash patch without changing the repository.
    StashPreview {
        /// Stable stash selector.
        reference: String,
    },
    /// Apply a stash while keeping it.
    StashApply {
        /// Stable stash selector.
        reference: String,
    },
    /// Apply and remove a stash.
    StashPop {
        /// Stable stash selector.
        reference: String,
    },
    /// Permanently remove a stash.
    StashDrop {
        /// Stable stash selector.
        reference: String,
    },
    /// Create and switch to a branch from a stash.
    StashBranch {
        /// New local branch name.
        name: String,
        /// Stable stash selector.
        reference: String,
    },
    /// Fetch and prune a remote.
    Fetch {
        /// Remote to fetch and prune.
        remote: String,
    },
    /// Pull using Git configuration and push the current branch.
    Sync,
    /// Continue the in-progress merge, rebase, or cherry-pick.
    Continue,
    /// Abort the in-progress merge, rebase, or cherry-pick.
    Abort,
    /// Skip the current rebase or cherry-pick commit.
    Skip,
    /// Fetch and switch to a reusable local GitHub pull-request branch.
    CheckoutPullRequest {
        /// GitHub remote that owns the pull-request ref.
        remote: String,
        /// Repository-local pull-request number.
        number: u64,
    },
}

/// Structured result from a repository action.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum VcsOutcome {
    /// The action completed without a more specific result.
    Completed,
    /// A stash was created; false means there were no changes to save.
    StashCreated(bool),
    /// Patch text for a stash preview.
    StashPreview {
        /// Previewed stash selector.
        reference: String,
        /// Unified diff and stat text.
        patch: String,
    },
    /// Sync cannot proceed until the current branch is published.
    NeedsPublish,
    /// A managed pull-request branch was fast-forwarded.
    PullRequestUpdated,
    /// The new local branch used for a checked-out pull request.
    PullRequestCheckedOut {
        /// Reusable local branch name.
        branch: String,
    },
    /// Commit removed from `HEAD` by undo.
    CommitUndone {
        /// Commit removed from `HEAD`.
        commit: String,
        /// Whether the removed commit was already reachable upstream.
        was_upstream: bool,
    },
}

/// Repository/remote facts for one file, answering [`Command::RemoteFacts`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteFacts {
    /// The `origin` remote's fetch URL.
    pub origin_url: String,
    /// `HEAD`'s commit hash, absent on an unborn branch.
    pub head: Option<String>,
    /// The current branch name, absent when detached.
    pub branch: Option<String>,
    /// The file's path relative to the repository worktree root.
    pub rel_path: PathBuf,
    /// Whether the file is tracked at `HEAD`.
    pub tracked: bool,
}

/// One changed file in an [`Event::VcsStatus`] listing: identity, status, and
/// added/removed line counts — no file contents. Ask [`Command::PrepareChange`]
/// for the displayable diff.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeSummary {
    /// The file path, relative to the repository root (the new path for renames).
    pub path: PathBuf,
    /// The previous path, set only for renames.
    pub old_path: Option<PathBuf>,
    /// The change status.
    pub status: karet_vcs::StatusKind,
    /// Whether the change is binary (line counts are then `0`).
    pub is_binary: bool,
    /// Added line count.
    pub added: usize,
    /// Removed line count.
    pub removed: usize,
}

/// A changed file prepared for display off the client thread: identity plus the
/// diff with per-line syntax token runs and intra-line emphasis precomputed
/// (see [`karet_diff::PreparedDiff`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreparedChange {
    /// The file path (the new path for renames).
    pub path: PathBuf,
    /// The previous path, set only for renames.
    pub old_path: Option<PathBuf>,
    /// The change status.
    pub status: karet_vcs::StatusKind,
    /// The display language name (e.g. `Rust`).
    pub language: String,
    /// The prepared diff: line diff, token runs, and intra-line pairs. Binary
    /// changes carry an empty diff flagged binary.
    pub diff: karet_diff::PreparedDiff,
}

/// Which diff-between-two-points a [`Command::RangeChanges`] asks for. The backend
/// resolves the endpoints against the repository (upstream, base branch, merge base) so
/// ref resolution stays with the repo, and answers with [`Event::RangeReady`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum RangeSpec {
    /// The current branch's unpushed work: `@{upstream}...HEAD` (three-dot) — what the
    /// local commits change since they diverged from the tracking branch.
    Unpushed,
    /// The current branch's changes since it forked from a base branch:
    /// `base...HEAD` (three-dot). `base` is auto-detected when `None`.
    SinceBase {
        /// The base branch/ref to compare against, or `None` to auto-detect.
        base: Option<String>,
    },
    /// An explicit comparison between two revisions. `merge_base` selects three-dot
    /// (`base...head`, from their merge base) over two-dot (`base..head`, the raw tips).
    Between {
        /// The "before" revision.
        base: String,
        /// The "after" revision.
        head: String,
        /// Whether to diff from the merge base (three-dot) rather than the tips.
        merge_base: bool,
    },
}
