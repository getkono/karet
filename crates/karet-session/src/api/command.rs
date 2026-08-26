//! The [`Command`] half of the backend seam: every request the presentation
//! layer can submit.
//!
//! Split out of [`api`](super) purely for the code-line ceiling; the vocabulary
//! is one contract with [`Event`](super::Event) and the two are versioned together.

use super::*;

/// A request submitted by the presentation layer to the backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
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
    /// Query the managed language-server registry without performing network I/O.
    LanguageServerStatus,
    /// Explicitly approve discovery and installation of one missing server.
    InstallLanguageServer {
        /// Provider to install at its latest stable version.
        server: LanguageServerId,
    },
    /// Explicitly perform network metadata checks for installed servers.
    CheckLanguageServerUpdates {
        /// One provider to check, or `None` to force-check every installed provider.
        server: Option<LanguageServerId>,
    },
    /// Apply part or all of the exact update plan previously returned by the backend.
    ApplyLanguageServerPlan {
        /// Opaque plan identifier.
        plan: LanguageServerPlanId,
        /// Providers from the plan to apply. An empty set is rejected.
        servers: Vec<LanguageServerId>,
    },
    /// Deactivate a Karet-managed provider and safely retire its payload.
    UninstallLanguageServer {
        /// Managed provider to uninstall.
        server: LanguageServerId,
    },
    /// Restart this session's processes for an already-approved active version.
    RestartLanguageServer {
        /// Provider whose running slots should restart.
        server: LanguageServerId,
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
    /// Compile the LaTeX root containing an editable TeX document and produce a PDF.
    BuildLatex {
        /// The open TeX document that initiated the build.
        doc: DocumentId,
    },
    /// Run a workspace search on the backend's search worker; answered with
    /// [`Event::SearchResults`]. A newer search supersedes an unstarted one.
    Search {
        /// The search query and options.
        query: karet_search::SearchQuery,
        /// Keep at most this many file hits.
        limit: usize,
    },
    /// Spell-check the whole workspace on the backend's scan worker; answered with
    /// a stream of [`Event::SpellingScanProgress`] batches and one final
    /// [`Event::SpellingScanFinished`].
    ///
    /// Open documents are answered from their live buffers rather than from disk,
    /// so an unsaved edit is never reported stale. A no-op when spell-checking is
    /// disabled (the finish event still arrives, with nothing scanned), and
    /// cancellable through [`Command::Cancel`].
    ScanWorkspaceSpelling {
        /// Keep at most this many misspellings; the scan stops once it is reached
        /// and reports `truncated`.
        limit: usize,
    },
    /// Re-run the dependency-freshness check for one open manifest.
    RefreshManifestHints {
        /// The manifest document.
        doc: DocumentId,
    },
    /// Scan the workspace for codetag comments (`TODO`, `FIXME`, …), streaming
    /// results; cancellable through [`Command::Cancel`]. The tag vocabulary is
    /// `editor.semanticComments.tags` — the same set the editor tints.
    ScanWorkspaceTodos {
        /// Stop after this many hits, reporting truncation.
        limit: usize,
    },
    /// Start a debug session from a `debug.configurations` entry (the first
    /// one when unnamed). Progress and outcomes arrive as unsolicited
    /// `Debug*` events.
    DebugStart {
        /// The configuration name; `None` = the first configuration.
        configuration: Option<String>,
    },
    /// End the debug session, terminating the debuggee when the adapter
    /// supports it.
    DebugStop,
    /// Resume the stopped thread.
    DebugContinue,
    /// Step over the current line.
    DebugStepOver,
    /// Step into the call at the stop location.
    DebugStepIn,
    /// Step out of the current frame.
    DebugStepOut,
    /// Pause the running debuggee.
    DebugPause,
    /// The stopped thread's call stack; answered by [`Event::DebugStack`].
    DebugStackTrace,
    /// The variable scopes of one frame; answered by [`Event::DebugScopes`].
    DebugScopes {
        /// The frame id (from [`Event::DebugStack`]).
        frame: i64,
    },
    /// The children of a variables reference (a scope handle or a structured
    /// variable's); answered by [`Event::DebugVariables`]. Fetch lazily, on
    /// expand — references can be arbitrarily deep.
    DebugVariables {
        /// The `variablesReference` handle.
        reference: i64,
    },
    /// Evaluate an expression in the debuggee (the REPL); answered by
    /// [`Event::DebugEvaluated`].
    DebugEvaluate {
        /// The expression.
        expression: String,
        /// The frame to evaluate in, when one is selected.
        frame: Option<i64>,
    },
    /// Replace the breakpoints of one file (the full set, not a delta —
    /// `setBreakpoints` is full-replace per file by design). Stored so a
    /// session started later replays them; forwarded live to a running one,
    /// answered by [`Event::DebugBreakpoints`].
    DebugSetBreakpoints {
        /// The source file.
        path: std::path::PathBuf,
        /// The 0-based breakpoint lines.
        lines: Vec<u32>,
    },
    /// Run every code cell of a notebook, top to bottom, on its kernel
    /// (started on first use; `notebook.kernel.autoStart` starts it at open).
    /// Progress arrives as unsolicited `Notebook*` events plus refreshed
    /// [`Event::DocumentConverted`] previews; an errored cell stops the run.
    NotebookRunAll {
        /// The `.ipynb` path.
        path: std::path::PathBuf,
    },
    /// Run one code cell (by its index among the notebook's cells).
    NotebookRunCell {
        /// The `.ipynb` path.
        path: std::path::PathBuf,
        /// The cell index (all cells, not only code cells).
        cell: usize,
    },
    /// Interrupt the running cell (out-of-band, on the control channel).
    NotebookInterrupt,
    /// Restart the kernel; every cell's outputs are marked stale (cleared).
    NotebookRestart,
    /// Replace across every workspace match on the search worker; answered with
    /// [`Event::SearchReplaced`]. Open buffers pick the edits up through the
    /// file watcher.
    SearchReplaceAll {
        /// The query selecting the text to replace.
        query: karet_search::SearchQuery,
        /// The replacement text.
        replacement: String,
    },
    /// Add `word` to a spell-check dictionary settings layer. The write runs on
    /// the backend (never a UI thread); answered with
    /// [`Event::DictionaryWordAdded`], or
    /// [`Event::ProjectSettingsCreationRequired`] when the project layer does not
    /// exist and `create_project` was not set.
    AddDictionaryWord {
        /// The word to accept.
        word: String,
        /// Which settings layer receives it.
        scope: DictionaryScope,
        /// Explicit confirmation to create a missing project settings tree.
        create_project: bool,
    },
    /// Persist the inline-blame toggle to the user settings layer.
    SetBlameEnabled {
        /// Whether inline blame is enabled.
        enabled: bool,
    },
    /// Resolve the repository/remote facts for one file on the VCS worker
    /// (discovery starts from the file's own directory, so nested repositories
    /// resolve correctly); answered with [`Event::RemoteFacts`].
    RemoteFacts {
        /// The file whose repository context is wanted.
        path: PathBuf,
    },
    /// Prepare one [`Event::VcsStatus`] entry's displayable diff (line diff,
    /// syntax tokens, intra-line pairs) on the VCS worker; answered with
    /// [`Event::ChangePrepared`].
    PrepareChange {
        /// The changed file's path as listed by [`Event::VcsStatus`].
        path: PathBuf,
        /// `true` for the staged entry, `false` for the working-tree entry.
        staged: bool,
    },
    /// Index a package's seams, answering with [`Event::SeamIndexed`].
    IndexSeams {
        /// The package root to index. Defaults to the first workspace root when absent.
        root: Option<PathBuf>,
    },
    /// Re-index one file whose text changed, keeping the rest of the tree.
    ReindexSeams {
        /// The file that changed.
        path: PathBuf,
        /// Its current text, which may be unsaved buffer content.
        text: String,
    },
    /// Evaluate a seam query, answering with [`Event::SeamQueryResult`].
    SeamQuery {
        /// The query text, exactly as typed.
        text: String,
    },
    /// Fetch one seam node's edges, answering with [`Event::SeamNodeDetail`].
    SeamNode {
        /// The node's identity, as its semantic path.
        path: String,
    },
    /// Switch the active configuration, answering with a re-evaluated
    /// [`Event::SeamIndexed`].
    SetSeamConfiguration {
        /// The configuration to activate.
        name: String,
    },
    /// Convert a binary document (DOCX) to markdown for a read-only preview;
    /// answered with [`Event::DocumentConverted`].
    ConvertDocument {
        /// The document to convert.
        path: PathBuf,
    },
    /// Prepare an ad-hoc diff of two provided texts for display (e.g. the
    /// client's two-file diff mode); answered with [`Event::DiffPrepared`].
    PrepareDiff {
        /// Path used for language detection and labeling (the new side's).
        path: PathBuf,
        /// The old (left) text.
        old: String,
        /// The new (right) text.
        new: String,
    },
    /// Prepare the diff of one file at a revision against its current content,
    /// on the VCS worker; answered with [`Event::DiffPrepared`]. The revision
    /// side is read from the repository; the current side is `live` when given
    /// (an unsaved buffer), the worktree file otherwise.
    DiffWithRev {
        /// The file to diff (absolute or workspace-relative).
        path: PathBuf,
        /// The revision to read the old side at (e.g. `HEAD`, a branch, a hash).
        rev: String,
        /// The current (new-side) text, when the client holds unsaved edits.
        live: Option<String>,
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
    /// Apply a unified-diff patch to the index only (per-hunk staging):
    /// `reverse: false` stages the patch's changes, `reverse: true` un-stages
    /// them. The worktree is untouched. Answered by a fresh
    /// [`Event::VcsStatus`], or an [`Event::Notification`] when the patch does
    /// not apply.
    ApplyIndexPatch {
        /// A unified-diff patch (typically one hunk, from
        /// `karet_diff::format_hunk_patch`).
        patch: String,
        /// Un-stage instead of stage.
        reverse: bool,
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
    /// Load the current and incoming index stages for an unresolved merge conflict.
    MergeConflict {
        /// Repository-relative or absolute path to the conflicted file.
        path: PathBuf,
    },
    /// Load branch, remote, operation, and stash state for Source Control.
    RepositorySnapshot,
    /// Compute compact status for a nested repository shown in the explorer.
    NestedRepositoryStatus {
        /// Exact nested repository worktree directory.
        path: PathBuf,
    },
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

    // --- The remote seam -------------------------------------------------
    //
    // Local mode could answer these from the presentation layer's own process,
    // and used to. They exist because a client that does not share a machine
    // with the workspace has no other way to ask, and because routing both modes
    // through one path is what keeps the remote one from rotting.
    /// Declare the lines a view is displaying, so the backend can scope a
    /// document's highlight spans to them.
    ///
    /// Highlights are resolved per rendered line, so spans outside the viewport
    /// are never read. Bounding them is what keeps a keystroke's answer to about
    /// a screenful instead of a whole file's worth of spans.
    ///
    /// Advisory and idempotent: a backend may answer with a wider range (it
    /// pads by a margin so small scrolls need no round trip), and a client that
    /// never sends this gets whole-document highlights.
    SetViewport {
        /// The document being displayed.
        doc: DocumentId,
        /// The view displaying it — two views can show one document at
        /// different scroll positions.
        view: ViewId,
        /// First visible 0-based line.
        first_line: u32,
        /// Last visible 0-based line, inclusive.
        last_line: u32,
    },
    /// Classify a workspace path, answered by [`Event::PathClassified`].
    ///
    /// Deciding which renderer a path warrants needs its leading bytes and its
    /// length, so it has to happen where the file is.
    ClassifyPath {
        /// The path to classify.
        path: PathBuf,
        /// Bypass the size guard, so an over-large file opens with the renderer
        /// its content warrants rather than a placeholder.
        ignore_size: bool,
    },
    /// Read a byte range of a workspace file, answered by [`Event::FileBytes`].
    ///
    /// Backs the renderers that consume bytes rather than text — images, PDF
    /// pages, hex dumps. The bytes are rendered by the client, which is the side
    /// that knows its cell grid and graphics protocol.
    ReadFileBytes {
        /// The file to read.
        path: PathBuf,
        /// The byte offset to start at.
        offset: u64,
        /// How many bytes to read; the backend may return fewer.
        len: u64,
    },
    /// List the workspace's text files for quick-open, answered by
    /// [`Event::FilesListed`].
    ///
    /// Uses the same gitignore-aware walk the workspace search does, so what
    /// quick-open offers and what a search covers cannot drift.
    ListFiles {
        /// Stop after this many paths and report the listing as truncated.
        limit: usize,
    },
    /// List one directory's immediate children, answered by
    /// [`Event::DirectoryListed`].
    ReadDirectory {
        /// The directory to list.
        path: PathBuf,
        /// Include dotfiles.
        show_hidden: bool,
        /// Flag gitignored entries as ignored rather than listing them plainly.
        /// They are never filtered out — a tree dims them instead.
        respect_gitignore: bool,
    },
    /// Create, rename, copy or delete a workspace path, answered by
    /// [`Event::PathMutated`].
    MutatePath {
        /// What to do.
        mutation: PathMutation,
    },
    /// Store the client's opaque view state so a later attach can restore it.
    ///
    /// Tabs, panes and carets belong to the client — the backend never
    /// interprets this blob. But a client process does not outlive its
    /// connection, so the backend holds the bytes on its behalf; see
    /// [`Event::ViewStateRestored`].
    CheckpointViewState {
        /// The client's serialized view state.
        blob: Vec<u8>,
    },
}
