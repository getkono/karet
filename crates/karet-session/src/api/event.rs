use super::*;

/// A message emitted by the backend to the presentation layer. When it answers a
/// [`Command`], it is delivered with that command's [`RequestId`].
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    /// A managed provider was deactivated for future resolution.
    LanguageServerRemoved {
        /// Provider that was deactivated.
        server: LanguageServerId,
        /// Whether its immutable payload remains until shared brokers release it.
        cleanup_pending: bool,
    },
    /// A repository-scoped provider changed lifecycle state in this session.
    LanguageServerRuntimeChanged {
        /// Provider whose connection changed.
        server: LanguageServerId,
        /// Repository root owned by the connection.
        root: PathBuf,
        /// New lifecycle state.
        state: LanguageServerRuntimeState,
        /// Most recent concise failure, when applicable.
        error: Option<String>,
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
    /// Workspace symbols answering a [`Command::WorkspaceSymbols`] query.
    WorkspaceSymbols {
        /// Matching symbols from the active repository servers.
        symbols: Vec<Symbol>,
    },
    /// A refactoring edit for preview and explicit application by the client.
    WorkspaceEdit {
        /// Version-independent, path-grouped edits in buffer coordinates.
        edit: WorkspaceEdit,
    },
    /// Formatting edits answering [`Command::FormatOnSave`].
    FormattingEdits {
        /// Document the edits target.
        doc: DocumentId,
        /// Buffer version the edits were computed against.
        version: u64,
        /// Non-overlapping edits in buffer coordinates.
        edits: Vec<TextEdit>,
    },
    /// Workspace search results answering a [`Command::Search`].
    SearchResults {
        /// The per-file hits, capped at the request's limit.
        hits: Vec<karet_search::FileHit>,
    },
    /// One streamed batch of a workspace spelling scan, answering
    /// [`Command::ScanWorkspaceSpelling`]. Batches arrive as the walk progresses so
    /// a client can fill a list incrementally rather than waiting for the whole
    /// workspace; `files_scanned` is cumulative across the scan.
    SpellingScanProgress {
        /// The misspellings found since the previous batch.
        hits: Vec<SpellingHit>,
        /// How many files the scan has visited so far.
        files_scanned: usize,
    },
    /// The dependency-freshness hints for one open manifest (unsolicited;
    /// re-emitted as the buffer changes, tagged with the checked version so
    /// stale answers are droppable).
    ManifestHints {
        /// The manifest document.
        doc: DocumentId,
        /// The buffer version the check ran against.
        version: u64,
        /// Per-dependency hints, in line order.
        hints: Vec<ManifestHint>,
    },
    /// Today's WakaTime coding-time text for the status bar (unsolicited;
    /// only emitted while `wakatime.enabled` is set and a key is configured).
    WakatimeStatus {
        /// e.g. `"2 hrs 15 mins"`.
        text: String,
    },
    /// A batch of workspace codetag-scan results (see
    /// [`Command::ScanWorkspaceTodos`]); `files_scanned` is cumulative.
    TodoScanProgress {
        /// The codetags found since the previous batch.
        hits: Vec<TodoHit>,
        /// How many files the scan has visited so far.
        files_scanned: usize,
    },
    /// The workspace codetag scan ended (complete, truncated, or cancelled).
    TodoScanFinished {
        /// Total files visited.
        files_scanned: usize,
        /// Whether the hit limit cut the scan short.
        truncated: bool,
        /// Whether [`Command::Cancel`] stopped it.
        cancelled: bool,
    },
    /// One open document's complete spelling layer, emitted whenever it changes.
    ///
    /// Unsolicited: it carries no request id, because it describes the document
    /// rather than answering a scan. A client holding workspace scan results
    /// should replace everything it has for `path` with these hits — the document
    /// is the authority for a file that is open, and this is what keeps a results
    /// list from claiming a misspelling the editor is not underlining.
    SpellingUpdated {
        /// The document's path.
        path: PathBuf,
        /// Every misspelling in it now; empty when the file is clean or is no
        /// longer being checked at all.
        hits: Vec<SpellingHit>,
    },
    /// A workspace spelling scan reached a terminal state, answering
    /// [`Command::ScanWorkspaceSpelling`]. Exactly one arrives per scan.
    SpellingScanFinished {
        /// How many files the scan visited in total.
        files_scanned: usize,
        /// The scan stopped at its result limit; more misspellings exist.
        truncated: bool,
        /// The scan stopped early because of a [`Command::Cancel`].
        cancelled: bool,
    },
    /// A workspace replace-all finished, answering [`Command::SearchReplaceAll`].
    SearchReplaced {
        /// The number of files written.
        files_changed: usize,
        /// The total number of replacements applied.
        replacements: usize,
    },
    /// A package's seams were indexed, answering [`Command::IndexSeams`],
    /// [`Command::ReindexSeams`], or [`Command::SetSeamConfiguration`].
    ///
    /// Carries the whole flattened tree rather than a page of it: the presentation layer
    /// holds a copy so navigation, lens toggles, and rerooting are answered locally,
    /// which is the only way a cascading navigator stays responsive.
    SeamIndexed {
        /// What the index amounts to, for the header and the empty states.
        summary: SeamSummary,
        /// Every node, flattened.
        nodes: Vec<SeamNodeView>,
    },
    /// One package's seams are in, part-way through a [`Command::IndexSeams`].
    ///
    /// Packages are read concurrently and reported as each finishes, so a repository's
    /// first rows appear long before its last crate is parsed. Arrival order is completion
    /// order, not discovery order — the view keys nodes by identity, so it merges these in
    /// whatever order they land.
    ///
    /// Each package is *final* when it is sent: rollups and configuration membership are
    /// already resolved, and nothing later revises it. A package root is a subtree root,
    /// which is what makes that true.
    SeamPackageIndexed {
        /// Where this package sits in discovery order.
        ///
        /// Carried because arrival order is completion order: without it the view's first
        /// column — the package list — would be ordered by whichever core finished first,
        /// and would come out differently on every sync.
        order: usize,
        /// The package's own root identity, so a re-sync replaces rather than duplicates.
        root: String,
        /// Every node in this package, flattened, parents before children.
        nodes: Vec<SeamNodeView>,
        /// Modules in this package whose text could not be found.
        unresolved_modules: Vec<(String, Vec<PathBuf>)>,
    },
    /// An index is complete, closing a [`Command::IndexSeams`].
    ///
    /// Exactly one arrives per request, after every [`Event::SeamPackageIndexed`] for it.
    SeamIndexFinished {
        /// What the finished index amounts to, for the header and the empty states.
        summary: SeamSummary,
        /// How many files had to be parsed rather than replayed from the stored index.
        ///
        /// Zero means every file was unchanged since the last sync. This is what lets the
        /// view say whether a sync did anything.
        parsed: usize,
        /// How many source files the sync covered in total.
        ///
        /// The files the walk actually read or replayed — not the index's whole file
        /// table, which also holds each package's manifest as an anchor.
        files: usize,
    },
    /// A package could not be indexed, answering [`Command::IndexSeams`].
    SeamIndexFailed {
        /// Why, phrased for the reader.
        message: String,
    },
    /// A seam query was evaluated, answering [`Command::SeamQuery`].
    SeamQueryResult {
        /// The matching node identities.
        nodes: Vec<String>,
        /// The configuration the query asked to be evaluated under, if it named one.
        configuration: Option<String>,
        /// The parse failure, when the query did not parse.
        ///
        /// Present *instead of* results rather than alongside empty ones, so an
        /// unreadable query is never mistaken for a query that matched nothing.
        error: Option<SeamQueryError>,
    },
    /// One seam node's edges, answering [`Command::SeamNode`].
    SeamNodeDetail {
        /// The node the detail belongs to.
        node: String,
        /// Its edges, in both directions.
        edges: Vec<SeamEdgeView>,
        /// Its source lines with context, or a reader-facing reason there are none.
        ///
        /// Carried by the same event as the edges rather than by a second round trip:
        /// both answer "is pressing Enter worth it", both are invalidated by the same
        /// move of the selection, and splitting them would let the pane show one node's
        /// source beneath another node's relations.
        preview: Result<SeamPreview, String>,
    },
    /// A dictionary word was persisted, answering [`Command::AddDictionaryWord`].
    DictionaryWordAdded {
        /// The accepted word.
        word: String,
        /// The settings file that received it.
        path: PathBuf,
    },
    /// Adding a project dictionary word needs explicit confirmation because the
    /// project settings tree does not exist yet.
    ProjectSettingsCreationRequired {
        /// The word awaiting the confirmed write.
        word: String,
        /// The settings file that would be created.
        path: PathBuf,
    },
    /// Repository/remote facts answering [`Command::RemoteFacts`].
    RemoteFacts {
        /// The file the facts describe.
        path: PathBuf,
        /// The facts, or a user-facing reason they are unavailable (outside a
        /// repository, no origin remote, outside the worktree).
        facts: Result<RemoteFacts, String>,
    },
    /// One status entry's displayable diff, answering [`Command::PrepareChange`].
    ChangePrepared {
        /// The changed file's path, as requested.
        path: PathBuf,
        /// Which section was requested (`true` = staged).
        staged: bool,
        /// The prepared diff, or a user-facing reason it is unavailable (e.g.
        /// the entry no longer exists in that section).
        result: Result<Box<PreparedChange>, String>,
    },
    /// A document converted to markdown, answering [`Command::ConvertDocument`].
    DocumentConverted {
        /// The converted document's path, as requested.
        path: PathBuf,
        /// The markdown text, or a user-facing reason conversion failed.
        markdown: Result<String, String>,
    },
    /// An ad-hoc prepared diff, answering [`Command::PrepareDiff`] or
    /// [`Command::DiffWithRev`].
    DiffPrepared {
        /// The path the diff describes, as requested.
        path: PathBuf,
        /// The prepared diff, or a user-facing reason it is unavailable (e.g.
        /// the file does not exist at the requested revision).
        result: Result<Box<PreparedChange>, String>,
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
        /// The staged changes (identity and line counts; no contents).
        staged: Vec<ChangeSummary>,
        /// The working-tree changes (unstaged, untracked, conflicted).
        working: Vec<ChangeSummary>,
    },
    /// The read-only committed sides of an unresolved merge conflict.
    MergeConflictReady {
        /// The path requested by [`Command::MergeConflict`].
        path: PathBuf,
        /// The current branch's text (Git index stage 2).
        current: String,
        /// The incoming branch's text (Git index stage 3).
        incoming: String,
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
        /// Every ref per commit hash (branches, remotes, tags, detached
        /// `HEAD`), refreshed with each page.
        labels: std::collections::HashMap<String, Vec<karet_vcs::RefLabel>>,
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
        /// The files this commit changed relative to its first parent, prepared
        /// for the diff view.
        changes: Vec<PreparedChange>,
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
        /// The files that differ between the two points, prepared for the diff view.
        changes: Vec<PreparedChange>,
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
    /// The debug session's lifecycle changed (unsolicited).
    DebugState {
        /// The new state.
        state: DebugSessionState,
        /// A short human-readable detail (configuration name, stop reason,
        /// error text) for the status line.
        detail: String,
    },
    /// The debuggee stopped; inspection is now valid (unsolicited).
    DebugStopped {
        /// The adapter's reason (`"breakpoint"`, `"step"`, `"exception"`, …).
        reason: String,
        /// The stopped thread the run controls act on.
        thread: i64,
        /// The stop location's file, when the top frame reports one.
        path: Option<std::path::PathBuf>,
        /// The 0-based stop line, when known.
        line: Option<u32>,
    },
    /// The debuggee resumed (unsolicited).
    DebugContinued,
    /// The adapter or debuggee produced output (unsolicited; text may carry
    /// ANSI styling — render through `karet_widgets::ansi`).
    DebugOutput {
        /// The stream (`"console"`, `"stdout"`, `"stderr"`, …).
        category: String,
        /// The text, possibly multi-line.
        text: String,
    },
    /// The stopped thread's call stack (answers [`Command::DebugStackTrace`];
    /// empty when the debuggee is not stopped).
    DebugStack {
        /// Top frame first.
        frames: Vec<DebugFrame>,
    },
    /// One frame's variable scopes (answers [`Command::DebugScopes`]).
    DebugScopes {
        /// The frame the scopes belong to.
        frame: i64,
        /// The scopes, adapter order.
        scopes: Vec<DebugScope>,
    },
    /// One reference's children (answers [`Command::DebugVariables`]).
    DebugVariables {
        /// The handle the variables belong to.
        reference: i64,
        /// The variables, adapter order.
        variables: Vec<DebugVariable>,
    },
    /// An evaluation result (answers [`Command::DebugEvaluate`]; a rejected
    /// expression answers with the adapter's error text as the result).
    DebugEvaluated {
        /// The rendered result (or error text).
        result: String,
        /// Non-zero when the result has fetchable children.
        reference: i64,
    },
    /// The notebook kernel's state, for the status line (unsolicited:
    /// starting/ready/running/interrupted/failed text).
    NotebookKernelStatus {
        /// The notebook whose kernel this is.
        path: std::path::PathBuf,
        /// A short human-readable status.
        text: String,
    },
    /// One notebook cell finished (unsolicited; the refreshed preview rides
    /// [`Event::DocumentConverted`](Self::DocumentConverted) separately).
    NotebookCellDone {
        /// The notebook.
        path: std::path::PathBuf,
        /// The cell index.
        cell: usize,
        /// Whether the cell raised (an errored cell stops a Run All).
        errored: bool,
    },
    /// The acknowledged breakpoints of one file (answers
    /// [`Command::DebugSetBreakpoints`]; also unsolicited on late
    /// verification).
    DebugBreakpoints {
        /// The source file.
        path: std::path::PathBuf,
        /// The acknowledged set, in the submitted order.
        breakpoints: Vec<DebugBreakpoint>,
    },
}
