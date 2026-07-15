//! The in-process contract between the presentation layer and the backend: the
//! [`Command`]s a client submits and the [`Event`]s the backend emits.
//!
//! This module carries only neutral `karet-core` (plus a few engine) types, so it
//! is the designated extraction point for a future dependency-light
//! `karet-protocol` crate when the client-server split is undertaken.

use std::path::PathBuf;

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
use karet_vcs::Commit;
use karet_vcs::CommitDetail;
use karet_vcs::FileChange;

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
    /// Recompute and re-emit the source-control status.
    RefreshVcs,
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
    /// A commit was created.
    Committed {
        /// The new commit's hex object id.
        oid: String,
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
}
