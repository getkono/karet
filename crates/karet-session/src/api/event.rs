use super::*;

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
    /// A live configuration or EditorConfig change altered one document's behavior.
    DocumentSettingsChanged {
        /// The affected document.
        doc: DocumentId,
        /// The newly resolved behavior.
        settings: DocumentSettings,
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
    /// An external LaTeX build finished, successfully or otherwise.
    LatexBuildFinished {
        /// The editable document that initiated the build.
        doc: DocumentId,
        /// The resolved root TeX file (after `% !TeX root = …` discovery).
        root: PathBuf,
        /// Generated PDF path when the compiler succeeded and produced it.
        pdf: Option<PathBuf>,
        /// Compiler diagnostics anchored to source lines.
        diagnostics: Vec<Diagnostic>,
        /// A concise failure explanation, absent on success.
        error: Option<String>,
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
    /// An open document needs a managed server that is not installed.
    ///
    /// This event is local-only: emitting it performs no metadata request or
    /// other network traffic.
    LanguageServerInstallRequired {
        /// Missing provider.
        server: LanguageServerId,
    },
    /// Local managed-language-server status, answering
    /// [`Command::LanguageServerStatus`].
    LanguageServerStatus {
        /// One row per built-in provider.
        servers: Vec<LanguageServerStatus>,
    },
    /// Exact changes discovered by an explicitly requested update check.
    LanguageServerUpdatePlan {
        /// Opaque plan required to approve these exact versions.
        plan: LanguageServerPlanId,
        /// Proposed provider changes.
        changes: Vec<LanguageServerChange>,
    },
    /// Progress for an explicitly approved managed-server operation.
    LanguageServerProgress {
        /// Provider being installed or updated.
        server: LanguageServerId,
        /// Bytes received so far.
        downloaded: u64,
        /// Expected bytes, when upstream supplied a size.
        total: Option<u64>,
    },
    /// A managed provider was atomically installed or updated.
    LanguageServerChanged {
        /// Changed provider.
        server: LanguageServerId,
        /// Newly active version.
        version: String,
        /// Whether processes using an older version still need a user-approved restart.
        restart_required: bool,
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
    /// Compact synchronization and line-change status for a nested repository.
    NestedRepositoryStatus {
        /// Exact nested repository worktree directory.
        path: PathBuf,
        /// Current divergence and uncommitted line counts.
        summary: RepositorySummary,
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
