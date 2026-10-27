//! The request half of the seam: [`Command`], a small enum of families, and one
//! enum per family.
//!
//! Each family is handled by value, and exhaustively, by the session module
//! that owns it, so a new variant nobody routed fails to compile rather than
//! being dropped. Every family converts into a [`Command`] (`From`), so a
//! caller submitting through an `impl Into<Command>` parameter names only the
//! family variant: `DocumentCommand::Save { doc, cause }`.

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
    /// A [`DocumentCommand::Save`] still waiting on its formatter is cancellable
    /// too: it writes nothing and is answered with a `save cancelled` warning.
    Cancel {
        /// The original request to cancel.
        request: RequestId,
    },
    /// Open, edit, save, and close documents.
    Document(DocumentCommand),
    /// Language intelligence requests about an open document.
    Lsp(LspCommand),
    /// Manage the language servers themselves: install, update, restart,
    /// accounts, and their window traffic.
    LanguageServer(LanguageServerCommand),
    /// Source control: status, staging, history, diffs, and blame.
    Vcs(VcsCommand),
    /// GitHub forge calls. A build without the `github` feature answers every
    /// one with an [`Event::GithubError`] whose operation is
    /// [`GithubOperation::FeatureDisabled`].
    Github(GithubCommand),
    /// Drive the debugger.
    Debug(DebugCommand),
    /// Drive a notebook's kernel. A build without the `notebook-kernel`
    /// feature has no kernel to drive and ignores these.
    Notebook(NotebookCommand),
    /// Index and query the repository's seams.
    Seam(SeamCommand),
    /// Workspace-wide work: search, scans, edits across files, settings.
    Workspace(WorkspaceCommand),
}

/// Open, edit, save, and close documents ([`Command::Document`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum DocumentCommand {
    /// Open a document.
    Open {
        /// The file path to open.
        path: PathBuf,
        /// An explicit language id, or `None` to detect from the path.
        ///
        /// Matched against the file-type registry
        /// ([`karet_filetype::file_type_for_language`]): an LSP language id
        /// (`"typescriptreact"`), a configuration selector (`"c++"`), or a display
        /// name (`"Rust"`). A known id sets the document's grammar, display name,
        /// settings selector, and language-server id together; an unknown id is
        /// ignored and the language is detected from the path. It applies when the
        /// document is first opened: opening an already-open path reuses that
        /// document as it is, and [`DocumentCommand::Retarget`] re-detects from
        /// the new path.
        language: Option<String>,
    },
    /// Close a document.
    Close {
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
    ///
    /// While the save waits on a format-on-save formatter it is cancellable
    /// through [`Command::Cancel`]: nothing is written, the buffer stays dirty,
    /// and the request is answered with a [`Severity::Warning`]
    /// [`Event::Notification`] reading `save cancelled` instead of a save
    /// event. A save not waiting on a formatter completes before a cancel can
    /// reach it.
    Save {
        /// The document to save.
        doc: DocumentId,
        /// What prompted the save; see [`SaveCause`].
        cause: SaveCause,
    },
    /// Retarget an open document to a new path after a filesystem rename/move.
    Retarget {
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
    /// Report the client's cursor/selection state for a view.
    SetCursor {
        /// The target document.
        doc: DocumentId,
        /// The view whose cursors changed.
        view: ViewId,
        /// The new cursor state.
        cursors: CursorState,
    },
    /// Apply every markdownlint autofix to a document as one undoable change,
    /// under the workspace lint configuration -- the `.markdownlint.json(c)`
    /// the diagnostics layer lints with, found across every root.
    ///
    /// The fixes land as a new snapshot, so they undo as one step and every
    /// caret follows the text it sat in. Answered by
    /// [`Event::MarkdownLintFixed`], whose count is `0` when nothing was
    /// fixable (the document is then left as it was). The caller decides
    /// what is Markdown: the document is linted whatever its language. A build
    /// without the `mdlint` feature answers with an [`Event::Notification`].
    FixMarkdownLint {
        /// The target document.
        doc: DocumentId,
    },
    /// Compile the LaTeX root containing an editable TeX document and produce a PDF.
    BuildLatex {
        /// The open TeX document that initiated the build.
        doc: DocumentId,
    },
    /// Convert a binary document (DOCX) to markdown for a read-only preview;
    /// answered with [`Event::DocumentConverted`].
    Convert {
        /// The document to convert.
        path: PathBuf,
    },
    /// Re-run the dependency-freshness check for one open manifest.
    RefreshManifestHints {
        /// The manifest document.
        doc: DocumentId,
    },
    /// Recover the crash-recovery swaps announced by [`Event::SwapsFound`]: restore
    /// each backed-up buffer as an unsaved (dirty) document.
    RecoverSwaps,
    /// Discard the crash-recovery swaps announced by [`Event::SwapsFound`] without
    /// recovering them.
    DiscardSwaps,
}

/// Language intelligence requests about an open document ([`Command::Lsp`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum LspCommand {
    /// Request completions at a position. Answered by [`Event::Completions`],
    /// always: empty when no server completes there.
    Completion {
        /// The target document.
        doc: DocumentId,
        /// The position to complete at.
        position: LineCol,
        /// Why: invoked, a trigger character typed, or narrowing an incomplete
        /// list.
        trigger: CompletionTrigger,
    },
    /// Fill in what the server deferred for one of `doc`'s completion items
    /// -- its documentation, detail and additional edits -- when the user
    /// selects it. Answered by [`Event::CompletionResolved`], always.
    ResolveCompletion {
        /// The document the item completes in.
        doc: DocumentId,
        /// The item as an [`Event::Completions`] answer delivered it, payload
        /// included.
        item: CompletionItem,
    },
    /// Request inlay hints for a range of a document.
    ///
    /// Ranged rather than whole-document because a hint set is a function of
    /// what is on screen: asking for a 10,000-line file to annotate a 40-line
    /// viewport costs the server the whole file's inference.
    InlayHints {
        /// The target document.
        doc: DocumentId,
        /// The range to annotate, usually the visible viewport.
        range: Range,
    },
    /// Fill in what the server deferred for one of `doc`'s inlay hints --
    /// its tooltip, its label parts' tooltips and locations -- when a hover
    /// or a Ctrl+click needs it. Answered by [`Event::InlayHintResolved`],
    /// always: with the hint unchanged when there is no server to ask, it does
    /// not resolve hints, or the request failed.
    ResolveInlayHint {
        /// The document the hint annotates.
        doc: DocumentId,
        /// The hint as an [`Event::InlayHints`] answer delivered it, payload
        /// included.
        hint: InlayHint,
    },
    /// Ask for ghost text at a position: every language server attached to
    /// the document is asked, not only the language's primary, since inline
    /// completions usually come from a companion such as Copilot.
    ///
    /// Answered by one [`Event::InlineCompletions`] per server that has
    /// something to propose, and by nothing at all when none has -- no
    /// server, none offering inline completions, or nothing to say. Whether
    /// to ask at all (`editor.inlineCompletion`) is the client's to read, as
    /// it reads `editor.completion` before a [`LspCommand::Completion`]. A
    /// client shows no ghost text while it waits, which is also what
    /// "nothing" shows, so there is no empty answer to wait for. A client
    /// debounces this itself: a newer ask for the same document replaces one
    /// a server has not yet been sent.
    InlineCompletion {
        /// The target document.
        doc: DocumentId,
        /// The caret, in buffer columns.
        position: LineCol,
        /// Whether the user asked, or the editor asked on its own.
        trigger: InlineCompletionTrigger,
    },
    /// Tell the server that proposed `item` that the user accepted it, after
    /// the client applied its edit -- so the server runs whatever the item
    /// asked to have run on acceptance. Unanswered: nothing waits on it.
    AcceptInlineCompletion {
        /// The document the item was proposed for.
        doc: DocumentId,
        /// The server that proposed it, as [`Event::InlineCompletions`] named.
        server: LanguageServerId,
        /// The item as delivered, payload included.
        item: InlineCompletionItem,
    },
    /// Request a document's code lenses from its primary server, resolving
    /// the ones on `lines` it deferred. Answered by [`Event::CodeLenses`], or
    /// by [`Event::CodeLensesFailed`] when the server failed to answer.
    ///
    /// Lenses are listed for the whole document, which is cheap, but each
    /// deferred lens costs a server a search to resolve ("3 references"), so
    /// only those on `lines` -- usually the viewport -- are resolved.
    CodeLenses {
        /// The target document.
        doc: DocumentId,
        /// The first and last line (inclusive) whose lenses to resolve.
        lines: (u32, u32),
    },
    /// Run the command of one lens from an [`Event::CodeLenses`] answer
    /// (`workspace/executeCommand`). Answered by [`Event::WorkspaceEdit`],
    /// labelled with the lens's title, once the command has run or failed;
    /// edits the server makes while running it arrive before that,
    /// unrequested.
    RunCodeLens {
        /// The document the lens was listed for.
        doc: DocumentId,
        /// The lens, as the answer listed it.
        lens: CodeLens,
    },
    /// Request a document's links and colours from its primary server
    /// (`textDocument/documentLink` and `textDocument/documentColor`, asked
    /// together). Answered by [`Event::LinksAndColors`].
    LinksAndColors {
        /// The target document.
        doc: DocumentId,
    },
    /// Fill in the target of a link from an [`Event::LinksAndColors`] answer
    /// that arrived without one (`documentLink/resolve`). Answered by
    /// [`Event::DocumentLinkResolved`].
    ResolveDocumentLink {
        /// The document the link was listed for.
        doc: DocumentId,
        /// The link, as the answer listed it.
        link: DocumentLink,
    },
    /// Request hover information at a position.
    Hover {
        /// The target document.
        doc: DocumentId,
        /// The position to hover.
        position: LineCol,
    },
    /// Request signature help -- the parameter hints of the call enclosing a
    /// position. Answered by [`Event::SignatureHelp`], always: with no help
    /// when no server serves the document, it does not offer signature help,
    /// or the caret is not inside a call it recognises.
    SignatureHelp {
        /// The target document.
        doc: DocumentId,
        /// The caret, inside the call's argument list.
        position: LineCol,
        /// Whether the user asked by hand rather than by typing a trigger
        /// character. Only a manual request is reported as
        /// [`Event::FeatureUnsupported`] when the server does not offer
        /// signature help: a notice on every typed `(` would be noise.
        manual: bool,
    },
    /// Request the language server's selection ranges -- the chain of
    /// ever-larger syntactic ranges Expand Selection grows through -- around
    /// each of `positions`. Answered by [`Event::SelectionRanges`], always:
    /// with an empty chain per position when no server serves the document,
    /// it does not offer selection ranges, or the request failed, so the
    /// client falls back to its own syntax tree. A refusal is never reported
    /// as [`Event::FeatureUnsupported`], because that fallback stands in for
    /// the server without the user noticing.
    SelectionRanges {
        /// The target document.
        doc: DocumentId,
        /// One position per caret, in buffer columns.
        positions: Vec<LineCol>,
    },
    /// Request the language server's document highlights -- the occurrences
    /// of the symbol at `position`, each a read, a write, or text -- for the
    /// occurrence highlight. Answered by [`Event::DocumentHighlights`],
    /// always: with `highlights: None` when no server serves the document, it
    /// does not offer document highlights, or the request failed, so the
    /// client can fall back to matching the word as text. A refusal is never
    /// reported as [`Event::FeatureUnsupported`]: the request is made every
    /// time the caret rests, never by hand.
    DocumentHighlights {
        /// The target document.
        doc: DocumentId,
        /// The caret, in buffer columns.
        position: LineCol,
    },
    /// Request the ranges the language server links to the one at
    /// `position`, such as an HTML tag's name and its closing tag's (Start
    /// Linked Editing). Answered by [`Event::LinkedEditingRanges`], always:
    /// with `ranges: None` when no server serves the document, it has no
    /// linked ranges there, or the request failed. A server that does not
    /// offer linked editing is reported first as
    /// [`Event::FeatureUnsupported`], since this is only asked for by hand.
    LinkedEditingRanges {
        /// The target document.
        doc: DocumentId,
        /// The caret, in buffer columns.
        position: LineCol,
    },
    /// Request the edits the language server makes after `ch` was typed
    /// (`textDocument/onTypeFormatting`), `position` being the caret just
    /// after it and `\n` standing for a newline. Answered by
    /// [`Event::OnTypeFormatting`], always, with the edits for the client to
    /// apply: the session leaves the text alone, since the user may have typed
    /// on while the server worked. A character the server does not format on
    /// costs no request. A refusal is never reported as
    /// [`Event::FeatureUnsupported`]: the request follows typing, not a
    /// command.
    FormatOnType {
        /// The target document.
        doc: DocumentId,
        /// The caret after the typed character, in buffer columns.
        position: LineCol,
        /// The character typed.
        ch: char,
    },
    /// Resolve the definition of the symbol at a position.
    Definition {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
    },
    /// Navigate from the symbol at a position: its declaration, its type's
    /// definition, its implementations, or its references (answered by
    /// [`Event::Locations`]).
    FindLocations {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
        /// What to find.
        kind: LocationKind,
    },
    /// Find the symbol at a position a call or type hierarchy grows from:
    /// the roots of the tree. Answered by [`Event::Hierarchy`], always; a
    /// server that does not offer the hierarchy is reported first as
    /// [`Event::FeatureUnsupported`].
    PrepareHierarchy {
        /// The target document.
        doc: DocumentId,
        /// The position to resolve.
        position: LineCol,
        /// Which way the hierarchy will grow, which decides whether calls or
        /// types are asked about.
        direction: HierarchyDirection,
    },
    /// Grow a hierarchy one level from `item`: its callers, callees,
    /// supertypes or subtypes. Answered by [`Event::Hierarchy`], always.
    ExpandHierarchy {
        /// The document the hierarchy was asked from, whose server produced
        /// `item` and is asked again.
        doc: DocumentId,
        /// An item an earlier [`Event::Hierarchy`] carried, unchanged.
        item: Box<HierarchyItem>,
        /// Which way to grow.
        direction: HierarchyDirection,
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
    /// Ask whether the symbol at a position can be renamed, and what span and
    /// name a rename prompt should show. Answered by [`Event::RenamePrepared`].
    PrepareRename {
        /// The target document.
        doc: DocumentId,
        /// The position of the symbol.
        position: LineCol,
    },
    /// Rename the symbol at a position, and apply the language server's edit.
    ///
    /// Answered by [`Event::WorkspaceEdit`] once the edit is applied (or
    /// refused: the document moved on since the request, the server found
    /// nothing to rename, or a step failed).
    Rename {
        /// The target document.
        doc: DocumentId,
        /// The position of the symbol.
        position: LineCol,
        /// The new name.
        new_name: String,
    },
    /// Format a document, or ranges of it, with the formatter its language
    /// names (`formatter`, else its primary language server; TOML's built-in
    /// formatter when no server formats the whole document).
    ///
    /// The edits land as one undoable change and a new snapshot. Refusals are
    /// answered on the request: [`Event::FeatureUnsupported`] when the server
    /// does not offer the method, and an [`Event::Notification`] when nothing
    /// can format the document or it changed before the answer arrived.
    Format(FormatRequest),
    /// Ask the document's language server which code actions it offers over
    /// `range` -- the diagnostics there go along as context. Answered by
    /// [`Event::CodeActions`].
    CodeActions {
        /// The target document.
        doc: DocumentId,
        /// The span asked about, in buffer columns; empty at the caret.
        range: Range,
        /// Only these kinds and the kinds nested under them (`source`
        /// admits `source.organizeImports`); empty asks for every kind.
        kinds: Vec<String>,
        /// Whether the user asked, or the client is probing on its own.
        trigger: CodeActionTrigger,
    },
    /// Run one action from an [`Event::CodeActions`] answer: fill in its edit
    /// when the server deferred it, apply the edit through the one
    /// workspace-edit path, then run its command.
    ///
    /// Answered by [`Event::WorkspaceEdit`], labelled with the action's title,
    /// once all of that is done or a step failed (the document moved on from
    /// `version`, the action is disabled, or the server refused). Edits the
    /// server makes while running the command arrive before it, unrequested.
    ApplyCodeAction {
        /// The document the actions were listed for.
        doc: DocumentId,
        /// The document version they were listed at.
        version: u64,
        /// The action, as the answer listed it.
        action: CodeAction,
    },
}

/// Manage the language servers themselves ([`Command::LanguageServer`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum LanguageServerCommand {
    /// Query the managed language-server registry without performing network I/O.
    Status,
    /// Explicitly approve discovery and installation of one missing server.
    Install {
        /// Provider to install at its latest stable version.
        server: LanguageServerId,
    },
    /// Record that the user declined to install one missing provider, so the
    /// prompt is not raised again.
    ///
    /// The refusal is unconditional by construction: the prompt is raised before
    /// any discovery, so no version has been resolved to scope it to.
    Decline {
        /// Provider the user refused to install.
        server: LanguageServerId,
    },
    /// Forget a recorded refusal, so the provider may be offered again.
    Undecline {
        /// Provider to offer again.
        server: LanguageServerId,
    },
    /// Explicitly perform network metadata checks for installed servers.
    CheckUpdates {
        /// One provider to check, or `None` to force-check every installed provider.
        server: Option<LanguageServerId>,
    },
    /// Apply part or all of the exact update plan previously returned by the backend.
    ApplyPlan {
        /// Opaque plan identifier.
        plan: LanguageServerPlanId,
        /// Providers from the plan to apply. An empty set is rejected.
        servers: Vec<LanguageServerId>,
    },
    /// Deactivate a Karet-managed provider and safely retire its payload.
    Uninstall {
        /// Managed provider to uninstall.
        server: LanguageServerId,
    },
    /// Restart this session's processes for an already-approved active version.
    Restart {
        /// Provider whose running slots should restart.
        server: LanguageServerId,
    },
    /// Ask a language server to cancel work it reported as cancellable
    /// ([`Event::LanguageServerWork`]). The server still reports the `End`.
    CancelWork {
        /// The provider doing the work.
        server: LanguageServerId,
        /// The repository root of its connection.
        root: PathBuf,
        /// The work, as the event named it.
        token: WorkToken,
    },
    /// Answer a language server's question
    /// ([`Event::LanguageServerMessageRequest`]) with the index of the chosen
    /// action, or `None` for none. Answering one already answered is a no-op.
    AnswerMessage {
        /// The question being answered.
        request: LanguageServerMessageId,
        /// Index into the question's actions.
        action: Option<usize>,
    },
    /// Read one provider's log, answered by [`Event::LanguageServerLog`].
    Log {
        /// The provider whose log to read.
        server: LanguageServerId,
    },
    /// Sign in to the account a language server proposes for -- GitHub, for
    /// `copilot-language-server` -- through the server's own `signIn` flow.
    /// Answered by [`Event::LanguageServerAccount`], always; a device code
    /// ([`AccountOutcome::DeviceCode`]) is followed by a second one, with the
    /// same request id, once the server says whether the sign-in landed --
    /// unless a later sign-in or sign-out for the same server supersedes it
    /// first or the server stops, in which case that sign-in gets no second
    /// outcome.
    SignIn {
        /// The server, which must be running (attached to an open document).
        server: LanguageServerId,
    },
    /// Sign out of that account (`signOut`). Answered by
    /// [`Event::LanguageServerAccount`], always.
    SignOut {
        /// The server, which must be running.
        server: LanguageServerId,
    },
}

/// Source control ([`Command::Vcs`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum VcsCommand {
    /// Recompute and re-emit the source-control status.
    Refresh,
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
    /// Generate a commit message from the staged diff, under the
    /// `git.aiCommit.*` settings.
    ///
    /// Answered by [`Event::CommitMessageGenerated`] on success and
    /// [`Event::CommitMessageFailed`] otherwise — including the cases where
    /// nothing is staged or generation is unavailable, so a client can render
    /// the outcome where it asked rather than as a detached notification.
    ///
    /// The round-trip takes seconds. It is cancellable through
    /// [`Command::Cancel`], which kills the agent process, and a newer request
    /// supersedes an older one. Whether it can run at all is knowable in advance
    /// from [`Event::AiCommitAvailability`].
    GenerateCommitMessage,
    /// Re-probe the configured agent CLIs and re-emit
    /// [`Event::AiCommitAvailability`].
    ///
    /// Availability is already pushed at startup and whenever settings reload;
    /// this is for the moments a client wants a fresh answer, such as opening a
    /// configuration surface or retrying after the user installed something.
    ProbeAiCommit,
    /// Persist `git.aiCommit.*` to the user settings layer, retaining JSONC
    /// comments and unrelated keys, then re-probe and re-emit
    /// [`Event::AiCommitAvailability`].
    SetAiCommitOptions {
        /// The settings to write.
        options: Box<crate::config::schema::AiCommit>,
    },
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
    Action {
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
    /// Persist the inline-blame toggle to the user settings layer.
    SetBlameEnabled {
        /// Whether inline blame is enabled.
        enabled: bool,
    },
    /// Mark the document's current buffer lines that differ from `HEAD`,
    /// answered by [`Event::LineChanges`]. An unknown document answers with no
    /// markers rather than an error, since closing a tab races this request.
    LineChanges {
        /// Open document to compare.
        doc: DocumentId,
    },
    /// Fetch a page of the commit-history log (newest first), for lazy loading.
    Log {
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
    /// Prepare an ad-hoc diff of two provided texts for display (e.g. the
    /// client's two-file diff mode) on the VCS worker; answered with
    /// [`Event::DiffPrepared`].
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
}

/// GitHub forge calls ([`Command::Github`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum GithubCommand {
    /// Re-evaluate GitHub eligibility and authentication for the workspace root.
    Refresh,
    /// Authenticate the GitHub manager for this session with a personal access token.
    /// The backend consumes the token immediately and never includes it in an event.
    Login {
        /// Personal access token entered through the presentation's masked control.
        token: GithubToken,
    },
    /// Search repository issues with GitHub query syntax.
    SearchIssues {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Search repository pull requests with GitHub query syntax.
    SearchPullRequests {
        /// User query without the repository/object scope controlled by the backend.
        query: String,
        /// One-based result page.
        page: u32,
    },
    /// Load repository Actions workflows and recent runs.
    Actions {
        /// One-based result page.
        page: u32,
    },
    /// Load one issue and its complete conversation comments.
    Issue {
        /// Repository-local issue number.
        number: u64,
    },
    /// Load one pull request's canonical primary resource.
    PullRequest {
        /// Repository-local pull request number.
        number: u64,
    },
    /// Replace a pull request's Markdown description.
    UpdatePullRequestBody {
        /// Repository-local pull-request number.
        number: u64,
        /// New Markdown body.
        body: String,
    },
    /// Add a Markdown comment to a pull request conversation.
    CommentPullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Comment Markdown.
        body: String,
    },
    /// Merge a pull request at its currently displayed head SHA.
    MergePullRequest {
        /// Repository-local pull-request number.
        number: u64,
        /// Expected head SHA, preventing an unseen update from being merged.
        head_sha: String,
    },
    /// Convert a pull request to draft or mark it ready for review.
    SetPullRequestDraft {
        /// GraphQL pull-request node identifier.
        node_id: String,
        /// Repository-local pull-request number, used to refresh after mutation.
        number: u64,
        /// Desired draft state.
        draft: bool,
    },
    /// Load repository-aware options for the new-issue form.
    IssueMetadata,
    /// Create a repository issue.
    CreateIssue {
        /// The complete primary create payload.
        issue: GithubNewIssue,
    },
    /// Create a repository pull request.
    CreatePullRequest {
        /// The complete primary create payload.
        pull_request: GithubNewPullRequest,
    },
}

/// Drive the debugger ([`Command::Debug`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum DebugCommand {
    /// Start a debug session from a `debug.configurations` entry (the first
    /// one when unnamed). Progress and outcomes arrive as unsolicited
    /// `Debug*` events.
    Start {
        /// The configuration name; `None` = the first configuration.
        configuration: Option<String>,
    },
    /// End the debug session, terminating the debuggee when the adapter
    /// supports it.
    Stop,
    /// Resume the stopped thread.
    Continue,
    /// Step over the current line.
    StepOver,
    /// Step into the call at the stop location.
    StepIn,
    /// Step out of the current frame.
    StepOut,
    /// Pause the running debuggee.
    Pause,
    /// The stopped thread's call stack; answered by [`Event::DebugStack`].
    StackTrace,
    /// The variable scopes of one frame; answered by [`Event::DebugScopes`].
    Scopes {
        /// The frame id (from [`Event::DebugStack`]).
        frame: FrameId,
    },
    /// The children of a variables reference (a scope handle or a structured
    /// variable's); answered by [`Event::DebugVariables`]. Fetch lazily, on
    /// expand — references can be arbitrarily deep.
    Variables {
        /// The `variablesReference` handle.
        reference: VariablesRef,
    },
    /// Evaluate an expression in the debuggee (the REPL); answered by
    /// [`Event::DebugEvaluated`].
    Evaluate {
        /// The expression.
        expression: String,
        /// The frame to evaluate in, when one is selected.
        frame: Option<FrameId>,
    },
    /// Replace the breakpoints of one file (the full set, not a delta —
    /// `setBreakpoints` is full-replace per file by design). Stored so a
    /// session started later replays them; forwarded live to a running one,
    /// answered by [`Event::DebugBreakpoints`].
    SetBreakpoints {
        /// The source file.
        path: PathBuf,
        /// The 0-based breakpoint lines.
        lines: Vec<u32>,
    },
}

/// Drive a notebook's kernel ([`Command::Notebook`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum NotebookCommand {
    /// Run every code cell of a notebook, top to bottom, on its kernel
    /// (started on first use; `notebook.kernel.autoStart` starts it at open).
    /// Progress arrives as unsolicited `Notebook*` events plus refreshed
    /// [`Event::DocumentConverted`] previews; an errored cell stops the run.
    RunAll {
        /// The `.ipynb` path.
        path: PathBuf,
    },
    /// Run one code cell (by its index among the notebook's cells).
    RunCell {
        /// The `.ipynb` path.
        path: PathBuf,
        /// The cell index (all cells, not only code cells).
        cell: usize,
    },
    /// Interrupt the running cell (out-of-band, on the control channel).
    Interrupt,
    /// Restart the kernel; every cell's outputs are marked stale (cleared).
    Restart,
}

/// Index and query the repository's seams ([`Command::Seam`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum SeamCommand {
    /// Index a repository's seams, answering with a stream of
    /// [`Event::SeamPackageIndexed`] closed by one [`Event::SeamIndexFinished`].
    Index {
        /// The package root to index. Defaults to the first workspace root when absent.
        root: Option<PathBuf>,
        /// Whether to trust the stored index or rebuild it from source.
        mode: SeamSync,
    },
    /// Re-index one file whose text changed, keeping the rest of the tree.
    Reindex {
        /// The file that changed.
        path: PathBuf,
        /// Its current text, which may be unsaved buffer content.
        text: String,
    },
    /// Evaluate a seam query, answering with [`Event::SeamQueryResult`].
    Query {
        /// The query text, exactly as typed.
        text: String,
    },
    /// Fetch one seam node's edges and source, answering with [`Event::SeamNodeDetail`].
    Node {
        /// The node's identity, as its semantic path.
        path: String,
    },
    /// Switch the active configuration, answering with a re-evaluated
    /// [`Event::SeamIndexed`].
    SetConfiguration {
        /// The configuration to activate.
        name: String,
    },
}

/// Workspace-wide work ([`Command::Workspace`]).
#[derive(Clone, Debug)]
#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum WorkspaceCommand {
    /// Run a streaming workspace search on the backend's search worker; answered
    /// with a run of [`Event::SearchProgress`] batches and exactly one
    /// [`Event::SearchFinished`].
    ///
    /// Cancellable through [`Command::Cancel`]; a newer search also supersedes an
    /// unstarted one, which still receives its terminal event.
    Search {
        /// The search query and options; `excludes` are merged with the
        /// workspace's configured `search.exclude`.
        query: karet_search::SearchQuery,
        /// Stop the walk once this many files have matched.
        file_limit: usize,
        /// Stop the walk once this many matches have been found in total.
        match_limit: usize,
    },
    /// Replace across every workspace match on the search worker; answered with
    /// [`Event::SearchReplaced`]. Open buffers pick the edits up through the
    /// file watcher.
    ReplaceAll {
        /// The query selecting the text to replace.
        query: karet_search::SearchQuery,
        /// The replacement text.
        replacement: String,
    },
    /// Spell-check the whole workspace on the backend's scan worker; answered with
    /// a stream of [`Event::SpellingScanProgress`] batches and one final
    /// [`Event::SpellingScanFinished`].
    ///
    /// Open documents are answered from their live buffers rather than from disk,
    /// so an unsaved edit is never reported stale. A no-op when spell-checking is
    /// disabled (the finish event still arrives, with nothing scanned), and
    /// cancellable through [`Command::Cancel`].
    ScanSpelling {
        /// Keep at most this many misspellings; the scan stops once it is reached
        /// and reports `truncated`.
        limit: usize,
    },
    /// Scan the workspace for codetag comments (`TODO`, `FIXME`, …), streaming
    /// results; cancellable through [`Command::Cancel`]. The tag vocabulary is
    /// `editor.semanticComments.tags` — the same set the editor tints.
    ScanTodos {
        /// Stop after this many hits, reporting truncation.
        limit: usize,
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
    /// Apply a workspace edit -- text edits to open and unopened files, and
    /// file creates, renames, and deletes -- answered by
    /// [`Event::WorkspaceEdit`].
    ///
    /// Ranges are in buffer (character) columns, as everywhere on this seam.
    ApplyEdit {
        /// The edit.
        edit: WorkspaceEdit,
        /// A label for the edit, echoed in the answer.
        label: Option<String>,
    },
    /// Build the workspace package-dependency graph (answered by [`Event::GraphReady`]).
    DependencyGraph,
    /// Return the loaded settings and their in-memory provenance for this session.
    LoadedConfig,
}

/// `From<Family> for Command`, so a family variant submits wherever an
/// `impl Into<Command>` is taken.
macro_rules! family {
    ($($family:ident => $variant:ident),* $(,)?) => {
        $(
            impl From<$family> for Command {
                fn from(command: $family) -> Self {
                    Self::$variant(command)
                }
            }
        )*
    };
}

family! {
    DocumentCommand => Document,
    LspCommand => Lsp,
    LanguageServerCommand => LanguageServer,
    VcsCommand => Vcs,
    GithubCommand => Github,
    DebugCommand => Debug,
    NotebookCommand => Notebook,
    SeamCommand => Seam,
    WorkspaceCommand => Workspace,
}
