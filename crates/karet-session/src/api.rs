//! The in-process contract between the presentation layer and the backend: the
//! [`Command`]s a client submits and the [`Event`]s the backend emits.
//!
//! This module carries only neutral `karet-core` (plus a few engine) types, so it
//! is the designated extraction point for a future dependency-light
//! `karet-protocol` crate when the client-server split is undertaken.

use std::path::PathBuf;

use karet_core::BlameAttribution;
use karet_core::Change;
use karet_core::CompletionItem;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::Diagnostic;
use karet_core::Hover;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::NotificationKind;
use karet_core::Severity;
use karet_core::Symbol;
use karet_search::FileHit;
use karet_search::SearchQuery;
use karet_syntax::HighlightSpan;
use karet_text::EditCause;
use karet_vcs::Branch;
use karet_vcs::BranchTarget;
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::CreateBranchOptions;
use karet_vcs::FileChange;
use karet_vcs::Remote;
use karet_vcs::RemoteBranch;
use karet_vcs::RepositoryState;
use karet_vcs::StashEntry;
use karet_vcs::StashOptions;

/// A complete repository snapshot for Source Control controls and pickers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

use crate::config::LoadedConfig;

/// Identifies an open document within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(pub u64);

/// Identifies a view (editor pane) within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ViewId(pub u64);

/// Correlates a [`Command`] with the [`Event`] that answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

/// Which producer a [`Event::DecorationsChanged`] batch belongs to, so the client
/// can replace one producer's decoration layer atomically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecorationLayer {
    /// Version-control markers (git gutter, blame).
    Vcs,
    /// Debugger markers (breakpoints, current line).
    Dap,
    /// Search-match highlights.
    Search,
    /// Language-server decorations.
    Lsp,
}

/// Which diff-between-two-points a [`Command::RangeChanges`] asks for. The backend
/// resolves the endpoints against the repository (upstream, base branch, merge base) so
/// ref resolution stays with the repo, and answers with [`Event::RangeReady`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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

/// A request submitted by the presentation layer to the backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Command {
    /// Cancel a safely-droppable background request.
    ///
    /// Cancellation is cooperative: a worker suppresses results and stops before
    /// the next expensive phase. Repository mutations are never cancellable.
    Cancel {
        /// The original request to cancel.
        request: RequestId,
    },
    /// Open a document.
    OpenDocument {
        /// The file path to open.
        path: PathBuf,
        /// An explicit language id, or `None` to detect from the path.
        language: Option<String>,
    },
    /// Close a document.
    CloseDocument {
        /// The document to close.
        doc: DocumentId,
    },
    /// Apply an atomic change to a document.
    ApplyChange {
        /// The target document.
        doc: DocumentId,
        /// The change to apply.
        change: Change,
        /// Why the edit happened, used for undo grouping.
        cause: EditCause,
    },
    /// Save a document to disk.
    Save {
        /// The document to save.
        doc: DocumentId,
    },
    /// Retarget an open document to a new path after a filesystem rename/move.
    RetargetDocument {
        /// The document to retarget.
        doc: DocumentId,
        /// The document's new file path.
        path: PathBuf,
    },
    /// Undo the most recent edit group on a document.
    Undo {
        /// The target document.
        doc: DocumentId,
    },
    /// Redo the most recently undone edit group on a document.
    Redo {
        /// The target document.
        doc: DocumentId,
    },
    /// Request completions at a position.
    Completion {
        /// The target document.
        doc: DocumentId,
        /// The position to complete at.
        position: LineCol,
    },
    /// Request hover information at a position.
    Hover {
        /// The target document.
        doc: DocumentId,
        /// The position to hover.
        position: LineCol,
    },
    /// Resolve the definition of the symbol at a position.
    Definition {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
    },
    /// Request the document's symbols.
    DocumentSymbols {
        /// The target document.
        doc: DocumentId,
    },
    /// Search workspace symbols.
    WorkspaceSymbols {
        /// The query string.
        query: String,
    },
    /// Rename the symbol at a position.
    Rename {
        /// The target document.
        doc: DocumentId,
        /// The position of the symbol.
        position: LineCol,
        /// The new name.
        new_name: String,
    },
    /// Format a document as part of saving it.
    FormatOnSave {
        /// The document to format.
        doc: DocumentId,
    },
    /// Run a workspace search.
    Search {
        /// The search query and options.
        query: SearchQuery,
    },
    /// Report the client's cursor/selection state for a view.
    SetCursor {
        /// The target document.
        doc: DocumentId,
        /// The view whose cursors changed.
        view: ViewId,
        /// The new cursor state.
        cursors: CursorState,
    },
    /// Stage the given paths (add their worktree state to the index).
    Stage {
        /// Repository-relative paths to stage.
        paths: Vec<PathBuf>,
    },
    /// Unstage the given paths (reset their index entries to `HEAD`).
    Unstage {
        /// Repository-relative paths to unstage.
        paths: Vec<PathBuf>,
    },
    /// Discard the working-tree changes to the given paths (destructive).
    Discard {
        /// Repository-relative paths to discard.
        paths: Vec<PathBuf>,
    },
    /// Stage every change in the worktree.
    StageAll,
    /// Unstage every staged change.
    UnstageAll,
    /// Commit the staged changes with the given message.
    Commit {
        /// The commit message.
        message: String,
    },
    /// Generate a commit message from the staged diff (answered asynchronously by
    /// [`Event::CommitMessageGenerated`], or an [`Event::Notification`] when nothing
    /// is staged, generation fails, or the `aicommit` feature / `git.aiCommit`
    /// setting is disabled). Honours the `git.aiCommit.*` settings.
    GenerateCommitMessage,
    /// Recompute and re-emit the source-control status.
    RefreshVcs,
    /// Load branch, remote, operation, and stash state for Source Control.
    RepositorySnapshot,
    /// Run one repository mutation on the serialized background worker.
    VcsAction {
        /// Action to run.
        action: VcsAction,
    },
    /// Fetch a page of open pull requests for one GitHub remote.
    PullRequests {
        /// Configured remote whose URL identifies the GitHub repository.
        remote: String,
        /// One-based page number.
        page: u32,
        /// Maximum entries per page, from 1 to 100.
        per_page: u8,
    },
    /// Attribute the current buffer's cursor line.
    Blame {
        /// Open document to attribute.
        doc: DocumentId,
        /// Buffer version the client currently renders.
        version: u64,
        /// Zero-based cursor line.
        line: u32,
    },
    /// Fetch a page of the commit-history log (newest first), for lazy loading.
    VcsLog {
        /// How many commits to skip from `HEAD`.
        skip: usize,
        /// The maximum number of commits to return.
        limit: usize,
    },
    /// Load the full detail of a single commit (first answered by
    /// [`Event::CommitDetailReady`], then by [`Event::CommitReady`] once changed files
    /// are computed).
    CommitDetail {
        /// The revision to resolve: a hash, a ref name, `HEAD`, `HEAD~3`, ….
        rev: String,
    },
    /// Compute the diff between two points (answered by [`Event::RangeReady`], or an
    /// [`Event::Notification`] when the range cannot be resolved — e.g. no upstream, no
    /// base branch, a bad revision, or unrelated histories).
    RangeChanges {
        /// Which comparison to compute.
        spec: RangeSpec,
    },
    /// Fetch a page of a single file's history (answered by [`Event::FileHistory`]).
    FileHistory {
        /// The file whose history to walk.
        path: PathBuf,
        /// How many matching commits to skip.
        skip: usize,
        /// The maximum number of commits to return.
        limit: usize,
    },
    /// Lazily fetch a commit's GitHub "Verified" status (answered by
    /// [`Event::CommitVerification`]). A no-op unless the backend was built with the
    /// `github` feature and the `origin` remote is a GitHub repository.
    FetchCommitVerification {
        /// The full commit hash to look up.
        hash: String,
    },
    /// Re-evaluate GitHub eligibility and authentication for the workspace root.
    GithubRefresh,
    /// Authenticate the GitHub manager for this session with a personal access token.
    /// The backend consumes the token immediately and never includes it in an event.
    GithubLogin {
        /// Personal access token entered through the presentation's masked control.
        token: GithubToken,
    },
    /// Search repository issues with GitHub query syntax.
    GithubSearchIssues {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Search repository pull requests with GitHub query syntax.
    GithubSearchPullRequests {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Load repository Actions workflows and recent runs.
    GithubActions {
        /// One-based result page.
        page: u32,
    },
    /// Load one issue and its complete conversation comments.
    GithubIssue {
        /// Repository-local issue number.
        number: u64,
    },
    /// Load one pull request's canonical primary resource.
    GithubPullRequest {
        /// Repository-local pull request number.
        number: u64,
    },
    /// Replace a pull request's Markdown description.
    GithubUpdatePullRequestBody {
        /// Repository-local pull-request number.
        number: u64,
        /// New Markdown body.
        body: String,
    },
    /// Add a Markdown comment to a pull request conversation.
    GithubCommentPullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Comment Markdown.
        body: String,
    },
    /// Merge a pull request at its currently displayed head SHA.
    GithubMergePullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Expected head SHA, preventing an unseen update from being merged.
        head_sha: String,
    },
    /// Convert a pull request to draft or mark it ready for review.
    GithubSetPullRequestDraft {
        /// GraphQL pull-request node identifier.
        node_id: String,
        /// Repository-local pull-request number, used to refresh after mutation.
        number: u64,
        /// Desired draft state.
        draft: bool,
    },
    /// Load repository-aware options for the new-issue form.
    GithubIssueMetadata,
    /// Create a repository issue.
    GithubCreateIssue {
        /// The complete primary create payload.
        issue: GithubNewIssue,
    },
    /// Create a repository pull request.
    GithubCreatePullRequest {
        /// The complete primary create payload.
        pull_request: GithubNewPullRequest,
    },
    /// Recover the crash-recovery swaps announced by [`Event::SwapsFound`]: restore
    /// each backed-up buffer as an unsaved (dirty) document.
    RecoverSwaps,
    /// Discard the crash-recovery swaps announced by [`Event::SwapsFound`] without
    /// recovering them.
    DiscardSwaps,
    /// Build the workspace package-dependency graph (answered by [`Event::GraphReady`]).
    DependencyGraph,
    /// Return the loaded settings and their in-memory provenance for this session.
    LoadedConfig,
}

/// Which visualization a [`Event::GraphReady`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GraphKind {
    /// The package-dependency graph of the workspace.
    Dependency,
    /// The usage/call graph of a symbol.
    Usage,
}

/// A forge's verification verdict for a commit signature (see
/// [`Event::CommitVerification`]). Mirrors GitHub's `commit.verification`; defined here
/// (rather than re-exported from `karet-github`) so the seam stays stable whether or not
/// the `github` feature is compiled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubVerification {
    /// Whether the forge considers the signature verified.
    pub verified: bool,
    /// The forge's machine reason (`valid`, `unsigned`, `unknown_key`, …).
    pub reason: String,
    /// The signer the forge attributes the commit to, when present.
    pub signer: Option<String>,
}

/// A public GitHub repository selected for the session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubRepository {
    /// Owner or organization login.
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

/// Credential source, with no secret material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GithubAuthSource {
    /// Anonymous public-read mode.
    Anonymous,
    /// `GITHUB_TOKEN`.
    GithubToken,
    /// `GH_TOKEN`.
    GhToken,
    /// GitHub CLI credential.
    GithubCli,
    /// Explicit embedding/test credential.
    Explicit,
}

/// Safe authentication/capability state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubAuth {
    /// Credential source.
    pub source: GithubAuthSource,
    /// Whether mutation controls may be enabled.
    pub can_write: bool,
    /// Stable numeric identifier for the authenticated account, when known.
    #[serde(default)]
    pub viewer_id: Option<u64>,
    /// Login name of the authenticated account, when known.
    #[serde(default)]
    pub viewer_login: Option<String>,
}

/// A transient GitHub token whose debug representation never exposes the secret.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubToken(String);

impl GithubToken {
    /// Wrap a token received from an interactive presentation.
    #[must_use]
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Consume the wrapper for immediate authentication.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for GithubToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GithubToken(***)")
    }
}

/// A GitHub label used in issue and pull-request tables.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubLabel {
    /// Label name.
    pub name: String,
    /// Six-digit RGB colour without `#`.
    pub color: String,
}

/// An issue table row and detail header.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubIssue {
    /// Repository-local issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Raw Markdown body.
    pub body: Option<String>,
    /// Open/closed state.
    pub state: String,
    /// Creator login.
    pub creator: Option<String>,
    /// Stable numeric identifier of the creator account.
    #[serde(default)]
    pub creator_id: Option<u64>,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// Applied labels.
    pub labels: Vec<GithubLabel>,
    /// Whether another issue blocks this issue.
    pub blocked: bool,
    /// GitHub web URL.
    pub html_url: String,
}

/// One issue or pull-request timeline comment.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubComment {
    /// Stable API identifier.
    pub id: u64,
    /// Author login.
    pub creator: Option<String>,
    /// Raw Markdown body.
    pub body: String,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// GitHub web URL.
    pub html_url: String,
}

/// A pull-request table row.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequest {
    /// Repository-local pull-request number.
    pub number: u64,
    /// Pull-request title.
    pub title: String,
    /// Raw Markdown body.
    pub body: Option<String>,
    /// Open/closed state.
    pub state: String,
    /// Creator login.
    pub creator: Option<String>,
    /// Stable numeric identifier of the creator account.
    #[serde(default)]
    pub creator_id: Option<u64>,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// Applied labels.
    pub labels: Vec<GithubLabel>,
    /// Whether this is a draft.
    pub draft: bool,
    /// GraphQL node identifier used by draft/readiness mutations.
    #[serde(default)]
    pub node_id: String,
    /// Head commit SHA.
    #[serde(default)]
    pub head_sha: String,
    /// Base commit SHA.
    #[serde(default)]
    pub base_sha: String,
    /// Whether GitHub currently considers the pull request mergeable.
    #[serde(default)]
    pub mergeable: Option<bool>,
    /// Whether the pull request has already been merged.
    #[serde(default)]
    pub merged: bool,
    /// GitHub web URL.
    pub html_url: String,
}

/// One commit in a GitHub pull request.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequestCommit {
    /// Full commit SHA.
    pub sha: String,
    /// First line of the commit message.
    pub summary: String,
    /// Author display name.
    pub author: String,
    /// Commit timestamp in Unix seconds.
    pub committed_unix: i64,
    /// Parent commit SHAs.
    pub parents: Vec<String>,
    /// GitHub web URL.
    pub html_url: String,
}

/// One check run attached to a pull request head.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubCheckRun {
    /// Stable check-run identifier.
    pub id: u64,
    /// Check name.
    pub name: String,
    /// Queued/in-progress/completed state.
    pub status: String,
    /// Final result when complete.
    pub conclusion: Option<String>,
    /// GitHub web URL for the check details.
    pub html_url: String,
}

/// One non-comment event in a pull-request conversation timeline.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequestActivity {
    /// Stable event identifier when GitHub supplies one.
    pub id: Option<u64>,
    /// GitHub event kind, such as `committed` or `head_ref_force_pushed`.
    pub kind: String,
    /// Actor login when present.
    pub actor: Option<String>,
    /// Commit involved in the event when present.
    pub commit_id: Option<String>,
    /// Previous head SHA for a force-push event.
    pub before: Option<String>,
    /// New head SHA for a force-push event.
    pub after: Option<String>,
    /// Event timestamp in Unix seconds when present.
    pub created_unix: Option<i64>,
}

/// A GitHub Actions workflow.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubWorkflow {
    /// Workflow identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Repository-relative workflow path.
    pub path: String,
    /// GitHub workflow state.
    pub state: String,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
}

/// A GitHub Actions workflow run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubWorkflowRun {
    /// Run identifier.
    pub id: u64,
    /// Workflow identifier.
    pub workflow_id: u64,
    /// Display title.
    pub title: String,
    /// Branch, when present.
    pub branch: Option<String>,
    /// Head SHA.
    pub head_sha: String,
    /// Trigger event.
    pub event: String,
    /// Queued/in-progress/completed state.
    pub status: Option<String>,
    /// Final conclusion.
    pub conclusion: Option<String>,
    /// Actor login.
    pub actor: Option<String>,
    /// Repository-local run number.
    pub run_number: u64,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// GitHub web URL.
    pub html_url: String,
}

/// A generic GitHub result page.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPage<T> {
    /// Rows in this page.
    pub items: Vec<T>,
    /// Current one-based page.
    pub page: u32,
    /// Next page, when supplied.
    pub next_page: Option<u32>,
    /// Total result count, when supplied.
    pub total_count: Option<u64>,
}

/// Primary fields for issue creation.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubNewIssue {
    /// Issue title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Assignee logins.
    pub assignees: Vec<String>,
    /// Label names.
    pub labels: Vec<String>,
    /// Milestone number.
    pub milestone: Option<u64>,
    /// Repository issue-type identifier.
    pub issue_type: Option<String>,
}

/// Primary fields for pull-request creation.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubNewPullRequest {
    /// Pull-request title.
    pub title: String,
    /// Source branch or owner-qualified source branch.
    pub head: String,
    /// Destination branch.
    pub base: String,
    /// Markdown body.
    pub body: String,
    /// Whether to create a draft.
    pub draft: bool,
    /// Whether maintainers may modify the source branch.
    pub maintainer_can_modify: bool,
}

/// A crash-recovery swap offered to the UI on startup (see [`Event::SwapsFound`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapInfo {
    /// The document the swap backs up.
    pub original: PathBuf,
    /// When the swap was last written (milliseconds since the Unix epoch).
    pub updated_unix_ms: u128,
    /// Whether the original file changed on disk since the swap was written —
    /// recovering would discard those on-disk changes.
    pub conflict: bool,
}

/// A message emitted by the backend to the presentation layer. When it answers a
/// [`Command`], it is delivered with that command's [`RequestId`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// A document was opened at the given version.
    Opened {
        /// The opened document.
        doc: DocumentId,
        /// Its initial version.
        version: u64,
    },
    /// A change was applied, producing a new version.
    Applied {
        /// The document.
        doc: DocumentId,
        /// The resulting version.
        version: u64,
    },
    /// A document was saved.
    Saved {
        /// The saved document.
        doc: DocumentId,
    },
    /// A document path was retargeted after a filesystem rename/move.
    Retargeted {
        /// The retargeted document.
        doc: DocumentId,
        /// The document's new file path.
        path: PathBuf,
    },
    /// A document was closed.
    Closed {
        /// The closed document.
        doc: DocumentId,
    },
    /// A clean document was reloaded from disk after an external change. The new
    /// content arrives on the snapshot stream; this event carries the new version.
    Reloaded {
        /// The reloaded document.
        doc: DocumentId,
        /// The version after reloading.
        version: u64,
    },
    /// A document changed on disk while it had unsaved edits. The client should
    /// prompt the user (keep mine / reload theirs / view diff).
    ExternalConflict {
        /// The document with the conflict.
        doc: DocumentId,
    },
    /// An `OpenDocument` failed because the file's contents are not valid UTF-8.
    /// No document is registered for `path` — full non-UTF-8 editing isn't
    /// supported, so the client should fall back to a read-only view instead of
    /// leaving the tab's document unset forever.
    NotUtf8 {
        /// The path that could not be opened as text.
        path: PathBuf,
    },
    /// A debounced filesystem change was observed (see `karet-watch`). Distinct
    /// from the specific `Reloaded`/`VcsStatus`/`VcsLog` reactions the backend
    /// already performs on the same event — this tells the client something on
    /// disk changed so it can refresh anything else it derives from the
    /// workspace (e.g. re-run a live workspace search).
    FsChanged {
        /// The affected paths, as reported by the debounced watcher.
        paths: Vec<PathBuf>,
    },
    /// The watched configuration changed and a new in-memory snapshot is active.
    ConfigChanged {
        /// The merged settings, diagnostics, and provenance now used by the session.
        report: Box<LoadedConfig>,
    },
    /// New diagnostics were published for a document.
    DiagnosticsPublished {
        /// The document.
        doc: DocumentId,
        /// The full diagnostic set for the document.
        diagnostics: Vec<Diagnostic>,
    },
    /// A producer's decoration layer changed.
    DecorationsChanged {
        /// The document.
        doc: DocumentId,
        /// Which producer's layer this replaces.
        layer: DecorationLayer,
        /// The new decorations for that layer.
        decorations: Vec<Decoration>,
    },
    /// Updated syntax highlight spans for a document.
    Highlights {
        /// The document.
        doc: DocumentId,
        /// The highlight spans.
        spans: Vec<HighlightSpan>,
    },
    /// Resolved document symbols.
    Symbols {
        /// The document.
        doc: DocumentId,
        /// The symbols.
        symbols: Vec<Symbol>,
    },
    /// Completion results answering a [`Command::Completion`]. Delivered with the
    /// originating command's [`RequestId`]; `doc` and `version` echo the request's
    /// target so the client can drop sets that are stale by the time they arrive
    /// (document switched, or edited past `version`).
    Completions {
        /// The document the completions are for.
        doc: DocumentId,
        /// The document version the request was made against.
        version: u64,
        /// The completion items, with edit ranges in buffer (UTF-32) columns.
        items: Vec<CompletionItem>,
    },
    /// Hover result answering a [`Command::Hover`].
    HoverResult {
        /// The hover, if any.
        hover: Option<Hover>,
    },
    /// Definition locations answering a [`Command::Definition`].
    Definitions {
        /// The resolved locations.
        locations: Vec<Location>,
    },
    /// Search results answering a [`Command::Search`].
    SearchResults {
        /// The per-file hits.
        hits: Vec<FileHit>,
    },
    /// Progress on a long-running operation.
    Progress {
        /// A human-readable status message.
        message: String,
        /// Percent complete (0–100), if known.
        percent: Option<u8>,
    },
    /// A condition the client should surface to the user (an error, a warning, or
    /// an out-of-band informational message). Distinct from [`Progress`](Self::Progress),
    /// which is for genuine long-running progress.
    Notification {
        /// How prominently to surface it.
        severity: Severity,
        /// The originating subsystem.
        kind: NotificationKind,
        /// A human-readable message.
        message: String,
    },
    /// The current source-control status: the staged (`HEAD`↔index) and working
    /// (index↔worktree, plus untracked and conflicted) change sets.
    VcsStatus {
        /// The staged changes.
        staged: Vec<FileChange>,
        /// The working-tree changes (unstaged, untracked, conflicted).
        working: Vec<FileChange>,
    },
    /// Branch, remote, operation, and stash state for Source Control.
    RepositorySnapshot {
        /// Complete snapshot captured after a read or successful action.
        snapshot: Box<RepositorySnapshot>,
    },
    /// A repository action was accepted by the serialized worker.
    VcsOperationStarted {
        /// Accepted action.
        action: VcsAction,
    },
    /// A repository action finished successfully or failed.
    VcsOperationFinished {
        /// Completed action.
        action: VcsAction,
        /// Structured success result; absent when `error` is present.
        outcome: Option<VcsOutcome>,
        /// Human-readable failure, if the action failed.
        error: Option<String>,
    },
    /// One page of open pull requests for a remote.
    PullRequests {
        /// Remote queried by the command.
        remote: String,
        /// Returned entries.
        items: Vec<PullRequestSummary>,
        /// Next page advertised by the forge.
        next_page: Option<u32>,
    },
    /// Current-buffer blame, safe to discard when document/version/cursor changed.
    BlameResult {
        /// Attributed document.
        doc: DocumentId,
        /// Buffer version used for mapping.
        version: u64,
        /// Cursor line used for the request.
        line: u32,
        /// Attribution for the requested line, or `None` when the file has no
        /// committed history available.
        attribution: Option<BlameAttribution>,
    },
    /// A commit was created.
    Committed {
        /// The new commit's hex object id.
        oid: String,
    },
    /// A commit message was generated from the staged diff, answering
    /// [`Command::GenerateCommitMessage`]. The client fills its commit input with it.
    CommitMessageGenerated {
        /// The generated commit message.
        message: String,
    },
    /// A page of the commit-history log, answering a [`Command::VcsLog`].
    VcsLog {
        /// How many commits were skipped from `HEAD` (the page offset).
        skip: usize,
        /// The commits in this page, newest first.
        commits: Vec<Commit>,
        /// Whether more commits exist beyond this page.
        has_more: bool,
    },
    /// New commits appeared at the tip (an external `git commit`, amend, or small
    /// rebase detected via file-watching). These should be prepended to the loaded
    /// log without disturbing already-paged history. Emitted spontaneously, never in
    /// answer to a request.
    VcsCommitsPrepended {
        /// The new commits, newest first.
        commits: Vec<Commit>,
    },
    /// A commit's metadata, answering the first stage of [`Command::CommitDetail`].
    CommitDetailReady {
        /// The commit metadata (message, author/committer, parents, signature). Boxed
        /// to keep this large payload from bloating every other [`Event`] variant.
        detail: Box<CommitDetail>,
    },
    /// A commit's full detail plus its file changes, answering the final stage of
    /// [`Command::CommitDetail`].
    CommitReady {
        /// The commit metadata (message, author/committer, parents, signature). Boxed
        /// to keep this large payload from bloating every other [`Event`] variant.
        detail: Box<CommitDetail>,
        /// The files this commit changed relative to its first parent, for the diff view.
        changes: Vec<FileChange>,
    },
    /// The diff between two points, answering [`Command::RangeChanges`].
    RangeReady {
        /// The resolved "before" endpoint, for the compare header (e.g. `origin/main`,
        /// or a short hash).
        base_label: String,
        /// The resolved "after" endpoint, for the compare header (e.g. `HEAD`).
        head_label: String,
        /// Whether the diff was taken from the merge base (three-dot) rather than the tips.
        merge_base: bool,
        /// The files that differ between the two points, for the diff view.
        changes: Vec<FileChange>,
    },
    /// A page of a file's history, answering [`Command::FileHistory`].
    FileHistory {
        /// The file the history is for.
        path: PathBuf,
        /// How many commits were skipped (the page offset).
        skip: usize,
        /// The commits touching the file in this page, newest first.
        commits: Vec<Commit>,
        /// Whether more commits exist beyond this page.
        has_more: bool,
    },
    /// A commit's GitHub verification status, answering
    /// [`Command::FetchCommitVerification`]. Emitted only on a successful fetch.
    CommitVerification {
        /// The commit this verdict is for.
        hash: String,
        /// The forge's verification verdict.
        status: GithubVerification,
    },
    /// Current GitHub eligibility and authentication state.
    GithubAvailability {
        /// Eligible repository, or `None` when the pinned view must be hidden.
        repository: Option<GithubRepository>,
        /// Authentication state; anonymous when ineligible.
        auth: GithubAuth,
    },
    /// A page of issue search results.
    GithubIssues {
        /// Search result page.
        page: GithubPage<GithubIssue>,
    },
    /// A page of pull-request search results.
    GithubPullRequests {
        /// Search result page.
        page: GithubPage<GithubPullRequest>,
    },
    /// Actions workflows and runs loaded together for a layout-stable screen.
    GithubActions {
        /// Repository workflows.
        workflows: GithubPage<GithubWorkflow>,
        /// Recent workflow runs.
        runs: GithubPage<GithubWorkflowRun>,
    },
    /// Repository-aware options for the new-issue form.
    GithubIssueMetadataReady {
        /// Logins which GitHub permits as issue assignees.
        assignees: Vec<String>,
    },
    /// A created issue, also used as the primary issue-detail payload.
    GithubIssueReady {
        /// Issue data.
        issue: GithubIssue,
        /// Complete issue timeline comments.
        comments: GithubPage<GithubComment>,
    },
    /// A pull request detail response, also used after creation.
    GithubPullRequestReady {
        /// Pull-request data.
        pull_request: GithubPullRequest,
        /// Complete issue-conversation comments attached to the pull request.
        comments: GithubPage<GithubComment>,
        /// Commits contained in the pull request, in GitHub's API order.
        commits: Vec<GithubPullRequestCommit>,
        /// Check runs attached to the current head.
        checks: Vec<GithubCheckRun>,
        /// Non-comment conversation activity returned by GitHub's timeline API.
        activity: Vec<GithubPullRequestActivity>,
        /// Timeline-only load failure. The rest of the pull request remains usable.
        activity_error: Option<String>,
    },
    /// A GitHub operation failed without disrupting the session actor.
    GithubError {
        /// Short operation name.
        operation: String,
        /// Safe error message.
        message: String,
    },
    /// Crash-recovery swaps from a previous session were found on startup. The UI
    /// prompts the user to [`Command::RecoverSwaps`] or [`Command::DiscardSwaps`].
    SwapsFound {
        /// The recoverable swaps.
        swaps: Vec<SwapInfo>,
    },
    /// A visualization graph is ready to render (answers [`Command::DependencyGraph`]).
    GraphReady {
        /// Which visualization this is.
        kind: GraphKind,
        /// A short title for the view (e.g. the workspace or symbol name).
        title: String,
        /// The neutral graph to render.
        view: karet_core::GraphView,
    },
    /// The loaded settings and provenance for this running session.
    LoadedConfig {
        /// The loaded configuration report.
        report: Box<LoadedConfig>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_payloads_construct() {
        assert_eq!(DocumentId(1), DocumentId(1));
        assert_ne!(RequestId(1), RequestId(2));
        let _cmd = Command::Save { doc: DocumentId(7) };
        let _cmd = Command::RetargetDocument {
            doc: DocumentId(7),
            path: PathBuf::from("new.txt"),
        };
        let _ev = Event::Saved { doc: DocumentId(7) };
        let _ev = Event::Retargeted {
            doc: DocumentId(7),
            path: PathBuf::from("new.txt"),
        };
        let _cfg = Command::LoadedConfig;
        assert_eq!(DecorationLayer::Vcs, DecorationLayer::Vcs);
    }

    #[test]
    fn github_token_debug_never_exposes_the_secret() {
        let token = GithubToken::new("github_pat_super_secret".to_string());
        let debug = format!("{token:?}");
        assert_eq!(debug, "GithubToken(***)");
        assert!(!debug.contains("super_secret"));
    }

    #[test]
    fn pull_request_conversation_models_remain_serde_ready() -> Result<(), serde_json::Error> {
        let commit = GithubPullRequestCommit {
            sha: "bbbbbbbb".to_string(),
            summary: "Add feature".to_string(),
            author: "Octo Cat".to_string(),
            committed_unix: 2,
            parents: vec!["aaaaaaaa".to_string()],
            html_url: "https://github.com/o/r/commit/bbbbbbbb".to_string(),
        };
        let check = GithubCheckRun {
            id: 9,
            name: "CI".to_string(),
            status: "completed".to_string(),
            conclusion: Some("success".to_string()),
            html_url: "https://github.com/o/r/runs/9".to_string(),
        };
        let activity = GithubPullRequestActivity {
            id: Some(3),
            kind: "committed".to_string(),
            actor: Some("octocat".to_string()),
            commit_id: Some(commit.sha.clone()),
            before: None,
            after: None,
            created_unix: Some(2),
        };
        let commit_json = serde_json::to_string(&commit)?;
        let check_json = serde_json::to_string(&check)?;
        let activity_json = serde_json::to_string(&activity)?;
        assert_eq!(
            serde_json::from_str::<GithubPullRequestCommit>(&commit_json)?,
            commit
        );
        assert_eq!(serde_json::from_str::<GithubCheckRun>(&check_json)?, check);
        assert_eq!(
            serde_json::from_str::<GithubPullRequestActivity>(&activity_json)?,
            activity
        );
        let commands = [
            Command::GithubUpdatePullRequestBody {
                number: 12,
                body: "body".to_string(),
            },
            Command::GithubCommentPullRequest {
                number: 12,
                body: "comment".to_string(),
            },
            Command::GithubMergePullRequest {
                number: 12,
                head_sha: "bbbbbbbb".to_string(),
            },
            Command::GithubSetPullRequestDraft {
                node_id: "PR_node".to_string(),
                number: 12,
                draft: true,
            },
        ];
        assert_eq!(commands.len(), 4);
        Ok(())
    }
}
