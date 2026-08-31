//! The owned, headless editor model: [`Session`] and its read/event surface.
//!
//! A [`Session`] owns a [`DocumentStore`] of open documents and the senders for the
//! neutral [`Event`] stream and the local snapshot stream. It applies [`Command`]s
//! synchronously (the fast paths — open/apply/save/undo — are inline) and emits
//! [`Event`]s plus [`DocSnapshot`]s.
//!
//! Syntax highlighting is the one thing it does *not* do inline. Injection-aware
//! layered highlighting re-parses every embedded language, far too much work to hold
//! the command queue on. The session hands the buffer's text to the
//! [`crate::highlight`] worker and adopts the spans it sends back; meanwhile the spans
//! it already has ride each edit via `Highlights::translate`, so the view stays stable
//! in the frames before the worker answers.

mod debug;
mod docfile;
// The document-file helpers stay reachable unqualified from the sibling
// modules that used them before the split.
use docfile::apply_serialization_settings;
use docfile::load_document;
use docfile::normalize_text_for_save;
use docfile::resolve_document_settings;
use docfile::save_document;
mod documents;
#[cfg(feature = "github")]
mod github;
mod hints;
mod lifecycle;
mod lsp_commands;
mod lsp_registry_updates;
mod lsp_requests;
#[cfg(feature = "mdlint")]
mod mdlint;
#[cfg(feature = "notebook-kernel")]
mod notebooks;
mod notify_text;
mod persistence;
mod search;
mod spelling;
mod updates;
mod vcs;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "github")]
use github::GithubJob;
use karet_core::BytePos;
use karet_core::Change;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Selection;
use karet_core::Severity;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_filetype::FileKind;
use karet_filetype::classify_ignoring_size;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::AppliedEdit;
use karet_text::EditCause;
use karet_text::EditContext;
use karet_text::Encoding;
use karet_text::Eol as TextEol;
use karet_text::LoadError;
use karet_text::TextBuffer;
use karet_text::TextError;
use karet_treesitter::LanguageId;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;
use karet_vcs::Repository;
use karet_vcs::VcsError;
use karet_watch::FsEvent;
use karet_watch::Watcher;
use tokio::sync::mpsc;

use crate::api::Command;
use crate::api::DocumentEncoding;
use crate::api::DocumentId;
use crate::api::DocumentLineEnding;
use crate::api::DocumentSettings;
use crate::api::Event;
#[cfg(test)]
use crate::api::RangeSpec;
use crate::api::RequestId;
use crate::api::SwapInfo;
use crate::backup::SwapRecord;
use crate::backup::SwapStore;
use crate::backup::discard;
use crate::backup::scan;
use crate::config::load::ConfigManager;
use crate::highlight::HighlightJob;
use crate::highlight::HighlightRequest;
use crate::highlight::HighlightResult;
use crate::local::DocSnapshot;
use crate::local::SnapshotRx;
use crate::lsp::LspManager;
use crate::lsp::LspUpdate;
use crate::spell::SpellJob;
use crate::spell::SpellResult;

/// Configuration for a [`Session`].
#[derive(Clone, Debug)]
pub struct SessionConfig {
    /// Workspace root directories.
    pub roots: Vec<PathBuf>,
    /// Whether prepared diffs include syntax token runs. The client's
    /// presentation choice (e.g. a `--no-syntax` flag or `NO_COLOR`); defaults
    /// to `true`.
    pub diff_syntax: bool,
    /// The loaded, verified settings (see [`crate::config`]). Producers read editing
    /// behaviour (format-on-save, spell-check, …) from here.
    pub settings: crate::config::Settings,
    /// The loaded settings plus layer and explicit-key provenance for inspection.
    pub loaded_config: crate::config::LoadedConfig,
    /// Directory for crash-recovery swap files. The application sets this to the real
    /// user data directory ([`crate::backup::default_swap_dir`]); left `None` (as in
    /// tests) the session keeps no backups and never touches the user's data dir.
    pub swap_dir: Option<PathBuf>,
    /// Executable that can enter [`karet_supervisor::supervisor`] mode.
    ///
    /// The karet application supplies its own current executable. `None` is the
    /// safe headless/test default: external language servers are not spawned
    /// unless the host explicitly provides crash-safe process ownership.
    pub process_supervisor: Option<PathBuf>,
    /// Per-user, machine-local root for managed language-server installations.
    ///
    /// A headless embedding may leave this unset to disable built-in providers;
    /// configured custom servers remain available through the process supervisor.
    pub lsp_registry_dir: Option<PathBuf>,
    /// Directory for the stored seam index, which turns a cold start into a warm one.
    ///
    /// The application sets this to the real user cache directory
    /// ([`crate::seam_cache::default_cache_dir`]); left `None` (as in tests) every index
    /// is built from source and no user directory is touched.
    pub seam_cache_dir: Option<PathBuf>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            diff_syntax: true,
            settings: crate::config::Settings::default(),
            loaded_config: crate::config::LoadedConfig::default(),
            swap_dir: None,
            process_supervisor: None,
            lsp_registry_dir: None,
            seam_cache_dir: None,
        }
    }
}

impl SessionConfig {
    /// Whether format-on-save is enabled (`editor.formatOnSave`).
    #[must_use]
    pub fn format_on_save(&self) -> bool {
        self.settings.editor.format_on_save
    }

    /// Whether spell-checking is enabled (`spellcheck.enabled`).
    #[must_use]
    pub fn spellcheck(&self) -> bool {
        self.settings.spellcheck.enabled
    }
}

/// How a document's edit buffer maps to its on-disk bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocFormat {
    /// Plain UTF-8 text: the on-disk bytes are the buffer's text.
    Text,
    /// CBOR: the buffer holds diagnostic-notation text; disk holds CBOR bytes.
    /// Decoded on open and re-encoded on save.
    #[cfg(feature = "cbor")]
    Cbor,
}

#[derive(Debug, thiserror::Error)]
enum DocumentLoadError {
    #[error("file does not exist")]
    Missing,
    /// A known binary format whose decode-to-text failed (corrupt CBOR): the
    /// client should fall back to a read-only byte view, like non-UTF-8 text.
    #[error("file is not decodable to text")]
    Undecodable,
    #[error(transparent)]
    Load(#[from] LoadError),
}

/// One open document and its derived state.
struct Document {
    path: PathBuf,
    /// Human-readable label published to presentation clients.
    language: Option<&'static str>,
    /// Stable legacy-compatible key used for editor settings and server selection.
    language_selector: Option<&'static str>,
    /// Protocol identifier sent in `textDocument/didOpen`.
    lsp_language_id: Option<&'static str>,
    lang_id: Option<LanguageId>,
    buffer: TextBuffer,
    /// How the buffer is (de)serialized on disk.
    format: DocFormat,
    /// The path was absent on open, so its first save must use atomic no-clobber.
    must_create: bool,
    /// Per-path behavior after application settings and EditorConfig resolution.
    settings: DocumentSettings,
    /// The last highlights the worker produced, translated across any edits applied
    /// since. The parsed trees themselves live on the worker, not here.
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    /// Semantic block scopes produced by the syntax worker for this version.
    semantic_blocks: Arc<SemanticBlocks>,
    /// Grammar-backed outline used when LSP supplies no symbols.
    syntax_symbols: Arc<Vec<Symbol>>,
    /// Syntax-error line ranges from the worker's last parse (see
    /// [`DocSnapshot::syntax_error_lines`]).
    error_lines: Arc<Vec<(u32, u32)>>,
    /// Last spell-check diagnostics emitted for this exact document state.
    spell_diagnostics: Vec<karet_core::Diagnostic>,
    /// Last markdown-lint diagnostics (empty for non-Markdown documents or
    /// when the `mdlint` feature is off).
    lint_diagnostics: Vec<karet_core::Diagnostic>,
    /// Last language-server diagnostics accepted for this document version.
    lsp_diagnostics: HashMap<String, Vec<karet_core::Diagnostic>>,
    decorations: Vec<Decoration>,
    /// Open reference count (a path opened in N views shares one document).
    refs: u32,
    /// When the buffer first became dirty (session-clock ms), or `None` when clean.
    /// Drives the backup interval.
    dirty_since: Option<u64>,
    /// The buffer version last written to a crash-recovery swap, so a tick does not
    /// rewrite an unchanged buffer.
    backed_up_version: Option<u64>,
}

impl Document {
    /// Reconcile the backup bookkeeping with the buffer's dirty state after an edit:
    /// arm `dirty_since` on the clean→dirty transition, and disarm (dropping any
    /// pending backup) once the buffer is clean again (e.g. undone to the save point).
    fn sync_dirty_since(&mut self, tick: u64) {
        if self.buffer.is_dirty() {
            if self.dirty_since.is_none() {
                self.dirty_since = Some(tick);
            }
        } else {
            self.dirty_since = None;
            self.backed_up_version = None;
        }
    }
}

/// The set of open documents, indexed by id and by path (for de-duplication).
#[derive(Default)]
struct DocumentStore {
    docs: HashMap<DocumentId, Document>,
    by_path: HashMap<PathBuf, DocumentId>,
    next: u64,
}

/// The headless editor backend: owns documents and the workspace, orchestrates
/// the producer engines, applies [`Command`]s and emits [`Event`]s.
///
/// Construct with [`Session::new`], which also returns the [`EventRx`] and
/// [`SnapshotRx`] halves of its output streams; drive it in-process with
/// [`crate::backend::local`].
pub struct Session {
    config: SessionConfig,
    /// Cached configuration layers used for targeted live reloads.
    config_manager: Option<ConfigManager>,
    events: mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
    store: DocumentStore,
    /// Jobs for the background highlight worker (see [`crate::highlight`]). Layered
    /// highlighting is too heavy to run inline on this actor.
    highlight_tx: std::sync::mpsc::Sender<HighlightJob>,
    /// The worker's results, taken by [`crate::backend::local`] for the actor loop.
    highlight_rx: Option<mpsc::UnboundedReceiver<HighlightResult>>,
    /// Jobs for the debounced token-aware spell worker.
    spell_tx: std::sync::mpsc::Sender<SpellJob>,
    /// The workspace's markdown-lint configuration, read once at startup.
    #[cfg(feature = "mdlint")]
    lint_config: karet_markdown::lint::Config,
    /// Spell results, taken by the local backend actor.
    spell_rx: Option<mpsc::UnboundedReceiver<SpellResult>>,
    /// Last dictionary/load error per document, used to suppress edit-time spam.
    spell_errors: HashMap<DocumentId, String>,
    clock: Instant,
    /// The workspace file-watcher, kept alive for the session's lifetime.
    watcher: Option<Watcher>,
    /// The watcher's event stream, taken by [`crate::backend::local`] for the actor.
    fs_rx: Option<mpsc::UnboundedReceiver<FsEvent>>,
    /// The source-control repository for the first workspace root, if any.
    vcs: Option<Repository>,
    /// Ordered background repository actions and network reads.
    vcs_worker: std::sync::mpsc::Sender<crate::vcs_worker::VcsJob>,
    search_worker: std::sync::mpsc::Sender<crate::search_worker::SearchJob>,
    /// The seam indexer, which owns its index rather than rebuilding it per request.
    seam_worker: std::sync::mpsc::Sender<crate::seam_worker::SeamJob>,
    /// The workspace spelling scan, which walks every file rather than the open ones.
    spell_scan_worker: std::sync::mpsc::Sender<crate::spell_scan::SpellScanJob>,
    todo_scan_worker: std::sync::mpsc::Sender<crate::todo_scan::TodoScanJob>,
    /// The WakaTime worker, spawned on the first heartbeat while enabled.
    wakatime_worker: Option<std::sync::mpsc::Sender<crate::wakatime::Beat>>,
    /// The last heartbeat kept, so a repeat for the same file inside the
    /// throttle window is dropped before its payload is built.
    wakatime_last: Option<(PathBuf, std::time::Duration)>,
    /// Monotonic origin for `wakatime_last`, matching the worker's own clock.
    wakatime_clock: std::time::Instant,
    /// The manifest-hints worker, spawned on the first Cargo.toml refresh.
    #[cfg(feature = "deps")]
    manifest_hints_worker: Option<std::sync::mpsc::Sender<crate::manifest_hints::HintJob>>,
    /// Cancellation registry for safely-droppable background reads: repository
    /// reads, LaTeX builds, and the workspace spelling scan.
    cancellations: crate::cancellation::CancellationHub,
    /// The most recent AI commit-message request, so a newer one supersedes it
    /// rather than racing it. Generation drives an external process, and two of
    /// them answering the same input box is never what the user asked for.
    ///
    /// It is not cleared when a request finishes: cancelling a completed id is a
    /// no-op (its registration is dropped with the task), and ids never repeat,
    /// so holding the last one costs nothing and keeps this a plain assignment
    /// on the actor thread rather than something the task has to reach back into.
    #[cfg(feature = "aicommit")]
    ai_commit_request: Option<RequestId>,
    /// Serialized external LaTeX builds.
    latex_worker: std::sync::mpsc::Sender<crate::latex::LatexJob>,
    /// Whether prepared diffs carry syntax token runs (the client's choice; see
    /// [`SessionConfig::diff_syntax`]).
    diff_syntax: bool,
    /// The last observed `HEAD` commit hash. A filesystem event that moves the tip
    /// away from this triggers an incremental commit-log reconciliation.
    last_head: Option<String>,
    /// This session's crash-recovery swap store (`None` if no data directory).
    swaps: Option<SwapStore>,
    /// Swaps found on startup awaiting the user's recover/discard decision.
    pending_swaps: Vec<SwapRecord>,
    /// Debugger orchestration (one active DAP session; see [`crate::dap`]).
    debug: crate::dap::DebugManager,
    /// Notebook kernel orchestration (see [`crate::notebook_kernel`]).
    #[cfg(feature = "notebook-kernel")]
    notebooks: crate::notebook_kernel::NotebookKernels,
    /// Language-server orchestration (lazy per-language tasks; see [`crate::lsp`]).
    lsp: LspManager,
    /// The LSP tasks' results, taken by [`crate::backend::local`] for the actor.
    lsp_rx: Option<mpsc::UnboundedReceiver<LspUpdate>>,
    /// Explicit install/update work for the shared managed-server registry.
    lsp_registry: std::sync::mpsc::Sender<crate::lsp_registry::RegistryJob>,
    /// Registry results, taken by the local backend actor.
    lsp_registry_rx: Option<mpsc::UnboundedReceiver<crate::lsp_registry::RegistryUpdate>>,
    /// Exact-root public-GitHub identity, when this workspace is eligible.
    #[cfg(feature = "github")]
    github_repository: Option<karet_github::RepositoryIdentity>,
    /// Commands for the asynchronous GitHub manager, installed in [`Self::start`].
    #[cfg(feature = "github")]
    github_tx: Option<
        mpsc::UnboundedSender<(
            RequestId,
            GithubJob,
            Option<crate::cancellation::Cancellation>,
        )>,
    >,
}

/// The most new commits [`Session::reconcile_vcs_log`] will prepend at once. Beyond
/// this the history is assumed rewritten (rebase/force-push) and the log is reloaded.
const LOG_RECONCILE_CAP: usize = 256;

/// The first-page size used when a reconciliation falls back to a full log reload.
const LOG_RELOAD_PAGE: usize = 25;

impl Session {
    /// Handle one request. The editing fast paths resolve inline; the answering
    /// [`Event`] is tagged with `id`.
    pub fn handle(&mut self, id: RequestId, command: Command) {
        if self.handle_debug_command(id, &command) {
            return;
        }
        #[cfg(feature = "notebook-kernel")]
        if self.handle_notebook_command(&command) {
            return;
        }
        if self.handle_lsp_command(id, &command) {
            return;
        }
        match command {
            Command::Cancel { request } => self.cancellations.cancel(request),
            Command::OpenDocument { path, language } => self.open(id, path, language.as_deref()),
            Command::CloseDocument { doc } => self.close(id, doc),
            Command::ApplyChange { doc, change, cause } => self.apply(id, doc, &change, cause),
            Command::Undo { doc } => self.undo_redo(id, doc, true),
            Command::Redo { doc } => self.undo_redo(id, doc, false),
            Command::Save { doc } => self.save(id, doc),
            Command::RetargetDocument { doc, path } => self.retarget(id, doc, path),
            Command::BuildLatex { doc } => self.request_latex_build(id, doc),
            // The caret is UI-local; `SetCursor` becomes meaningful when producers
            // (LSP at a position, multi-view sync) need it.
            Command::SetCursor { .. } => {},
            Command::Stage { paths } => self.vcs_write(id, |repo| repo.stage(&paths)),
            Command::ApplyIndexPatch { patch, reverse } => {
                self.vcs_write(id, |repo| repo.apply_index_patch(&patch, reverse));
            },
            Command::Unstage { paths } => self.vcs_write(id, |repo| repo.unstage(&paths)),
            Command::Discard { paths } => self.vcs_write(id, |repo| repo.discard(&paths)),
            Command::StageAll => self.vcs_write(id, Repository::stage_all),
            Command::UnstageAll => self.vcs_write(id, Repository::unstage_all),
            Command::Commit { message } => {
                self.submit_vcs(id, |id, _| crate::vcs_worker::VcsJob::Commit {
                    id,
                    message,
                });
            },
            Command::GenerateCommitMessage => self.generate_commit_message(id),
            Command::ProbeAiCommit => self.emit_ai_commit_availability(Some(id)),
            Command::SetAiCommitOptions { options } => self.set_ai_commit_options(id, *options),
            Command::RefreshVcs => self.emit_vcs_status(Some(id)),
            Command::RepositorySnapshot => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::Snapshot {
                    id,
                    cancel,
                });
            },
            Command::NestedRepositoryStatus { path } => {
                self.submit_vcs(id, |id, cancel| {
                    crate::vcs_worker::VcsJob::NestedRepositoryStatus { id, path, cancel }
                });
            },
            Command::VcsAction { action } => {
                self.emit(
                    Some(id),
                    Event::VcsOperationStarted {
                        action: action.clone(),
                    },
                );
                let _ = self
                    .vcs_worker
                    .send(crate::vcs_worker::VcsJob::Action { id, action });
            },
            Command::PullRequests {
                remote,
                page,
                per_page,
            } => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::PullRequests {
                    id,
                    remote,
                    page,
                    per_page,
                    cancel,
                });
            },
            Command::Blame { doc, version, line } => self.request_blame(id, doc, version, line),
            Command::VcsLog { skip, limit } => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::Log {
                    id,
                    skip,
                    limit,
                    cancel,
                });
            },
            Command::CommitDetail { rev } => {
                let syntax = self.diff_syntax;
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::CommitDetail {
                    id,
                    rev,
                    syntax,
                    cancel,
                });
            },
            Command::RangeChanges { spec } => {
                let syntax = self.diff_syntax;
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::RangeChanges {
                    id,
                    spec,
                    syntax,
                    cancel,
                });
            },
            Command::MergeConflict { path } => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::MergeConflict {
                    id,
                    path,
                    cancel,
                });
            },
            Command::FileHistory { path, skip, limit } => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::FileHistory {
                    id,
                    path,
                    skip,
                    limit,
                    cancel,
                });
            },
            Command::FetchCommitVerification { hash } => self.fetch_commit_verification(id, hash),
            #[cfg(feature = "github")]
            Command::GithubRefresh => self.refresh_github(id),
            #[cfg(feature = "github")]
            Command::GithubLogin { token } => self.send_github(
                id,
                GithubJob::Login {
                    token: token.into_inner(),
                },
            ),
            #[cfg(feature = "github")]
            Command::GithubSearchIssues { query, page } => {
                self.send_github(id, GithubJob::Issues { query, page })
            },
            #[cfg(feature = "github")]
            Command::GithubSearchPullRequests { query, page } => {
                self.send_github(id, GithubJob::PullRequests { query, page })
            },
            #[cfg(feature = "github")]
            Command::GithubActions { page } => self.send_github(id, GithubJob::Actions { page }),
            #[cfg(feature = "github")]
            Command::GithubIssue { number } => self.send_github(id, GithubJob::Issue { number }),
            #[cfg(feature = "github")]
            Command::GithubPullRequest { number } => {
                self.send_github(id, GithubJob::PullRequest { number })
            },
            #[cfg(feature = "github")]
            Command::GithubUpdatePullRequestBody { number, body } => {
                self.send_github(id, GithubJob::UpdatePullRequestBody { number, body })
            },
            #[cfg(feature = "github")]
            Command::GithubCommentPullRequest { number, body } => {
                self.send_github(id, GithubJob::CommentPullRequest { number, body })
            },
            #[cfg(feature = "github")]
            Command::GithubMergePullRequest { number, head_sha } => {
                self.send_github(id, GithubJob::MergePullRequest { number, head_sha })
            },
            #[cfg(feature = "github")]
            Command::GithubSetPullRequestDraft {
                node_id,
                number,
                draft,
            } => self.send_github(
                id,
                GithubJob::SetPullRequestDraft {
                    node_id,
                    number,
                    draft,
                },
            ),
            #[cfg(feature = "github")]
            Command::GithubIssueMetadata => self.send_github(id, GithubJob::IssueMetadata),
            #[cfg(feature = "github")]
            Command::GithubCreateIssue { issue } => {
                self.send_github(id, GithubJob::CreateIssue { issue })
            },
            #[cfg(feature = "github")]
            Command::GithubCreatePullRequest { pull_request } => {
                self.send_github(id, GithubJob::CreatePullRequest { pull_request })
            },
            #[cfg(not(feature = "github"))]
            Command::GithubRefresh
            | Command::GithubLogin { .. }
            | Command::GithubSearchIssues { .. }
            | Command::GithubSearchPullRequests { .. }
            | Command::GithubActions { .. }
            | Command::GithubIssue { .. }
            | Command::GithubPullRequest { .. }
            | Command::GithubUpdatePullRequestBody { .. }
            | Command::GithubCommentPullRequest { .. }
            | Command::GithubMergePullRequest { .. }
            | Command::GithubSetPullRequestDraft { .. }
            | Command::GithubIssueMetadata
            | Command::GithubCreateIssue { .. }
            | Command::GithubCreatePullRequest { .. } => self.emit(
                Some(id),
                Event::GithubError {
                    operation: "github".to_string(),
                    message: "the backend was built without the github feature".to_string(),
                },
            ),
            Command::RecoverSwaps => self.recover_swaps(id),
            Command::DiscardSwaps => self.discard_swaps(),
            Command::DependencyGraph => self.emit_dependency_graph(id),
            Command::LoadedConfig => self.emit(
                Some(id),
                Event::LoadedConfig {
                    report: Box::new(self.config.loaded_config.clone()),
                },
            ),
            Command::Completion { doc, position } => self.completion(id, doc, position),
            Command::Hover { doc, position } => self.hover(id, doc, position),
            Command::Definition { doc, position } => self.definition(id, doc, position),
            Command::DocumentSymbols { doc } => self.document_symbols(id, doc),
            Command::WorkspaceSymbols { query } => self.workspace_symbols(id, query),
            Command::Rename {
                doc,
                position,
                new_name,
            } => self.rename(id, doc, position, new_name),
            Command::FormatOnSave { doc } => self.format_document(id, doc),
            Command::RemoteFacts { path } => {
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::RemoteFacts {
                    id,
                    path,
                    cancel,
                });
            },
            Command::PrepareChange { path, staged } => {
                let syntax = self.diff_syntax;
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::PrepareChange {
                    id,
                    path,
                    staged,
                    syntax,
                    cancel,
                });
            },
            Command::ConvertDocument { path } => self.convert_document(id, path),
            Command::PrepareDiff { path, old, new } => {
                let syntax = self.diff_syntax;
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::PrepareTexts {
                    id,
                    path,
                    old,
                    new,
                    syntax,
                    cancel,
                });
            },
            Command::DiffWithRev { path, rev, live } => {
                let syntax = self.diff_syntax;
                self.submit_vcs(id, |id, cancel| crate::vcs_worker::VcsJob::DiffWithRev {
                    id,
                    path,
                    rev,
                    live,
                    syntax,
                    cancel,
                });
            },
            Command::AddDictionaryWord {
                word,
                scope,
                create_project,
            } => self.add_dictionary_word(id, word, scope, create_project),
            Command::SetBlameEnabled { enabled } => self.set_blame_enabled(id, enabled),
            Command::Search {
                query,
                file_limit,
                match_limit,
            } => self.search_workspace(id, query, file_limit, match_limit),
            Command::IndexSeams { root, mode } => {
                if let Some(root) = root.or_else(|| self.config.roots.first().cloned()) {
                    let _ = self.seam_worker.send(crate::seam_worker::SeamJob::Index {
                        id,
                        root,
                        mode,
                        // The cap stops being theoretical once the root is a whole
                        // repository rather than one crate.
                        options: karet_seam::IndexOptions {
                            max_files: self.config.settings.seam.max_indexed_files,
                        },
                    });
                }
            },
            Command::ReindexSeams { path, text } => {
                let _ =
                    self.seam_worker
                        .send(crate::seam_worker::SeamJob::Reindex { id, path, text });
            },
            Command::SeamQuery { text } => {
                let _ = self
                    .seam_worker
                    .send(crate::seam_worker::SeamJob::Query { id, text });
            },
            Command::SeamNode { path } => {
                let _ = self
                    .seam_worker
                    .send(crate::seam_worker::SeamJob::Node { id, path });
            },
            Command::SetSeamConfiguration { name } => {
                let _ = self
                    .seam_worker
                    .send(crate::seam_worker::SeamJob::SetConfiguration { id, name });
            },
            Command::ScanWorkspaceSpelling { limit } => self.scan_workspace_spelling(id, limit),
            Command::ScanWorkspaceTodos { limit } => self.scan_workspace_todos(id, limit),
            Command::RefreshManifestHints { doc } => self.refresh_manifest_hints(doc),
            Command::SearchReplaceAll { query, replacement } => {
                self.search_replace_all(id, query, replacement);
            },
            // Language-server management commands are consumed by the
            // `handle_lsp_command` pre-dispatch above and never reach this match.
            _ => {},
        }
    }

    // --- source control ---------------------------------------------------

    /// Register a cancellation for `id` and hand the worker a job built from it —
    /// the single submission shape for every cancellable VCS request.
    fn submit_vcs(
        &self,
        id: RequestId,
        make: impl FnOnce(RequestId, crate::cancellation::Cancellation) -> crate::vcs_worker::VcsJob,
    ) {
        let cancel = self.cancellations.register(id);
        let _ = self.vcs_worker.send(make(id, cancel));
    }

    fn request_blame(&self, id: RequestId, doc: DocumentId, version: u64, line: u32) {
        let Some(document) = self.store.docs.get(&doc) else {
            self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: "blame: unknown document".to_string(),
                },
            );
            return;
        };
        if document.buffer.version() != version {
            return;
        }
        let _ = self.vcs_worker.send(crate::vcs_worker::VcsJob::Blame {
            id,
            doc,
            version,
            path: document.path.clone(),
            text: document.buffer.text(),
            line,
            cancel: self.cancellations.register(id),
        });
    }

    fn request_latex_build(&mut self, id: RequestId, doc: DocumentId) {
        match self.save_for_external_build(doc) {
            Ok(source) => {
                if self.enqueue_latex_build(Some(id), id, doc, source).is_err() {
                    self.emit(
                        Some(id),
                        Event::LatexBuildFinished {
                            doc,
                            root: PathBuf::new(),
                            pdf: None,
                            diagnostics: Vec::new(),
                            error: Some("LaTeX build worker is unavailable".to_owned()),
                        },
                    );
                }
            },
            Err(error) => self.emit(
                Some(id),
                Event::LatexBuildFinished {
                    doc,
                    root: PathBuf::new(),
                    pdf: None,
                    diagnostics: Vec::new(),
                    error: Some(error),
                },
            ),
        }
    }

    fn enqueue_latex_build(
        &self,
        event_id: Option<RequestId>,
        cancel_id: RequestId,
        doc: DocumentId,
        source: PathBuf,
    ) -> Result<(), ()> {
        self.latex_worker
            .send(crate::latex::LatexJob {
                id: event_id,
                doc,
                source,
                workspace: self.config.roots.first().cloned(),
                settings: self.config.settings.latex.clone(),
                cancel: self.cancellations.register(cancel_id),
                supervisor: self.config.process_supervisor.clone(),
            })
            .map_err(|_| ())
    }
}
/// Re-(or incrementally) parse `doc` and recompute its highlights.
///
/// When `edits` is `Some` and a tree already exists, the tree is edited in place
/// and reparsed incrementally (streaming the rope, no whole-file `String`);
/// otherwise a full parse runs. Highlights are recomputed against the resulting
/// tree (the query still materializes the text — the rope-native query is a
/// follow-up). Returns `true` for plaintext formats that need their spell job
/// scheduled immediately because no syntax-worker answer will arrive.
fn update_syntax(
    settings: &crate::config::Settings,
    highlight_tx: &std::sync::mpsc::Sender<HighlightJob>,
    doc_id: DocumentId,
    doc: &mut Document,
    edits: Option<&[AppliedEdit]>,
) -> bool {
    let Some(lang) = doc.lang_id else {
        // Plaintext: nothing to parse, and no worker round-trip to wait for.
        doc.highlights = Arc::new(Highlights::default());
        doc.folds = Arc::new(FoldRegions::default());
        doc.semantic_blocks = Arc::new(SemanticBlocks::default());
        doc.syntax_symbols = Arc::default();
        return true;
    };

    // Keep the spans we already have usable until the worker answers. Rendering them
    // unshifted would smear color across the text the edit moved.
    if let Some(edits) = edits {
        // Block scopes are line-based and cannot be translated safely across an
        // arbitrary edit. Hide them briefly rather than render stale source context.
        doc.semantic_blocks = Arc::new(SemanticBlocks::default());
        doc.syntax_symbols = Arc::default();
        for ae in edits {
            doc.highlights = Arc::new(doc.highlights.translate(
                BytePos(ae.start_byte),
                BytePos(ae.old_end_byte),
                BytePos(ae.new_end_byte),
            ));
        }
    }

    let request = HighlightRequest {
        doc: doc_id,
        version: doc.buffer.version(),
        lang,
        text: doc.buffer.text(),
        semantic: {
            let semantic = settings
                .editor
                .for_language(doc.language_selector)
                .semantic_comments();
            semantic
                .enabled()
                .then(|| karet_syntax::SemanticCommentConfig {
                    tags: semantic.tags().to_vec(),
                })
        },
        // `karet-text`'s applied edits *are* the parse host's edit type (both are
        // `karet_core::AppliedEdit`), so they pass through unconverted.
        edits: edits.map(<[AppliedEdit]>::to_vec),
    };
    // A dead worker only means no highlights; editing carries on.
    highlight_tx.send(HighlightJob::Update(request)).ok();
    false
}

/// Derive an [`EditContext`] from a change's geometry: a single-`char` insertion is
/// [`EditCause::Type`] (so consecutive typing coalesces into one undo step), and the
/// pre-edit caret is the first edit's start (so coalescing's adjacency check works
/// without the client reporting the cursor on every keystroke).
/// Build a [`Change`] that replaces the entirety of `doc`'s buffer with `new_text`,
/// based on the buffer's current version. Used to restore a recovered swap's content
/// as a dirty edit (undo returns to the on-disk version).
fn whole_document_change(doc: &Document, new_text: String) -> Option<Change> {
    let end = doc.buffer.byte_to_line_col(BytePos(doc.buffer.len_bytes()));
    let range = Range::new(LineCol::new(0, 0), end).ok()?;
    Some(Change::new(
        doc.buffer.version(),
        vec![TextEdit { range, new_text }],
    ))
}

fn edit_context(tick_ms: u64, cause: EditCause, change: &Change) -> EditContext {
    let cursor_before = change.edits.first().map_or_else(CursorState::default, |e| {
        CursorState::single(Selection::caret(e.range.start))
    });
    EditContext {
        tick_ms,
        cause,
        cursor_before,
    }
}

/// Map an explicit LSP-style language id (e.g. `"rust"`) to karet's display name,
/// when one is supplied on open.
fn name_for_language(_id: &str) -> Option<&'static str> {
    // The display name is derived from the path today; an explicit override table
    // lands with the LSP language registry.
    None
}

/// Resolve an editor language even when Karet has no Tree-sitter grammar for
/// it yet. LSP routing follows the broader file-type registry; syntax parsing
/// remains independently optional.
pub(crate) fn language_name_for_path(path: &Path) -> Option<&'static str> {
    language_name_from_path(path).or_else(|| {
        let file_type = karet_filetype::file_type_for_path(path);
        let name = file_type.name();
        (!matches!(name, "Plain Text" | "Unknown" | "Binary")).then_some(name)
    })
}

/// Resolve the stable per-language configuration/server selector for a path.
pub(crate) fn language_selector_for_path(path: &Path) -> Option<&'static str> {
    karet_filetype::file_type_for_path(path).config_selector()
}

/// Resolve the protocol language identifier for a path.
fn lsp_language_id_for_path(path: &Path) -> Option<&'static str> {
    karet_filetype::file_type_for_path(path).lsp_language_id()
}

fn unknown_document(doc: DocumentId) -> Event {
    Event::Notification {
        severity: Severity::Error,
        kind: NotificationKind::System,
        message: format!("unknown document {}", doc.0),
    }
}

/// The receiving half of a session's server→client event stream.
pub struct EventRx(mpsc::UnboundedReceiver<(Option<RequestId>, Event)>);

impl EventRx {
    /// Receive the next event, with the [`RequestId`] it answers (if any).
    ///
    /// Returns `None` once the session has shut down.
    pub async fn recv(&mut self) -> Option<(Option<RequestId>, Event)> {
        self.0.recv().await
    }

    /// Take the next ready event without awaiting, or `None` if none is queued.
    pub fn try_recv(&mut self) -> Option<(Option<RequestId>, Event)> {
        self.0.try_recv().ok()
    }
}
