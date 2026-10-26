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

mod code_actions;
mod columns;
mod completion;
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
mod edit_requests;
#[cfg(feature = "github")]
mod github;
mod hints;
mod inline_completion;
mod lifecycle;
mod lsp_commands;
mod lsp_registry_updates;
mod lsp_requests;
mod lsp_window;
#[cfg(feature = "mdlint")]
mod mdlint;
#[cfg(feature = "notebook-kernel")]
mod notebooks;
mod notify_text;
mod persistence;
mod reload;
mod retirement;
mod search;
mod semantic_tokens;
mod server_folds;
mod spelling;
mod streams;
mod syntax_state;
mod updates;
mod vcs;
mod viz;
mod workspace_edit;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use columns::Columns;
#[cfg(feature = "github")]
use github::GithubJob;
use karet_core::BytePos;
use karet_core::Change;
use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Selection;
use karet_core::Severity;
use karet_core::Symbol;
use karet_core::TextEdit;
#[cfg(feature = "cbor")]
use karet_filetype::FileKind;
#[cfg(feature = "cbor")]
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
use syntax_state::SyntaxState;
use tokio::sync::mpsc;

use crate::api::Command;
use crate::api::DocumentCommand;
use crate::api::DocumentEncoding;
use crate::api::DocumentId;
use crate::api::DocumentLineEnding;
use crate::api::DocumentSettings;
use crate::api::Event;
#[cfg(test)]
use crate::api::RangeSpec;
use crate::api::RequestId;
use crate::api::SaveCause;
use crate::api::SeamCommand;
use crate::api::SwapInfo;
use crate::api::WorkspaceCommand;
use crate::backup::SwapRecord;
use crate::backup::SwapStore;
use crate::backup::discard;
use crate::backup::scan;
use crate::config::load::ConfigManager;
use crate::event_tx::EventTx;
use crate::highlight::HighlightJob;
use crate::highlight::HighlightRequest;
use crate::highlight::HighlightResult;
use crate::local::DocSnapshot;
use crate::local::SnapshotRx;
use crate::lsp::FormattingAsk;
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
    // Only CBOR has a decode step today; the variant stays ungated so the
    // open path's match arm in `documents.rs` reads the same in every build.
    #[cfg_attr(
        not(feature = "cbor"),
        expect(dead_code, reason = "constructed only by the cbor decode path")
    )]
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
    /// What the syntax worker last derived, translated across any edits
    /// applied since.
    syntax: SyntaxState,
    /// Last spell-check diagnostics emitted for this exact document state.
    spell_diagnostics: Vec<karet_core::Diagnostic>,
    /// Last markdown-lint diagnostics (empty for non-Markdown documents or
    /// when the `mdlint` feature is off).
    lint_diagnostics: Vec<karet_core::Diagnostic>,
    /// Last language-server diagnostics accepted for this document version.
    ///
    /// One layer per [`SlotKey`](crate::lsp::SlotKey) -- one provider at one
    /// repository root -- so a server that dies or is retired takes only its own
    /// markers with it, and the same provider serving another root keeps its.
    /// Each slot has a layer per feed ([`DiagnosticLayer`](crate::lsp::DiagnosticLayer)),
    /// so a server that both publishes and answers pulls never has one set
    /// erase the other.
    lsp_diagnostics: HashMap<crate::lsp::DiagnosticLayer, Vec<karet_core::Diagnostic>>,
    /// The semantic-token owner's last overlay, translated across edits like
    /// [`highlights`](Self::highlights) (see [`semantic_tokens`]).
    semantic_tokens: Option<semantic_tokens::SemanticLayer>,
    /// The primary server's folding ranges, preferred over the grammar's folds
    /// on every snapshot while held (see [`server_folds`]).
    server_folds: Option<server_folds::ServerFolds>,
    /// Open reference count (a path opened in N views shares one document).
    refs: u32,
    /// When the buffer first became dirty (session-clock ms), or `None` when clean.
    /// Drives the backup interval.
    dirty_since: Option<u64>,
    /// The buffer version last written to a crash-recovery swap, so a tick does not
    /// rewrite an unchanged buffer.
    backed_up_version: Option<u64>,
    /// The caret the client last reported through [`Command::SetCursor`], with
    /// the buffer version it arrived at. A save's own rewrites record it as the
    /// cursor their undo restores, but only while that version is still
    /// current: once any edit lands after it, the report no longer says where
    /// the user is.
    reported_cursor: Option<(u64, CursorState)>,
    /// The buffer version a save's formatter edit left behind, so the same
    /// save's whitespace cleanup joins its undo step instead of adding one.
    formatted_version: Option<u64>,
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
    events: EventTx,
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
    /// Whether something this unit of actor work did left a client's cached
    /// inventory describing a session that no longer exists.
    ///
    /// Set by [`Session::adopt_retirement`] and drained by
    /// [`Session::settle_lsp_inventory`] once the work finishes, so a restart
    /// signals staleness after its replacement is up rather than during the gap.
    lsp_inventory_stale: bool,
    /// The LSP tasks' results, taken by [`crate::backend::local`] for the actor.
    lsp_rx: Option<mpsc::UnboundedReceiver<LspUpdate>>,
    /// Explicit install/update work for the shared managed-server registry.
    lsp_registry: std::sync::mpsc::Sender<crate::lsp_registry::RegistryJob>,
    /// Registry results, taken by the local backend actor.
    lsp_registry_rx: Option<mpsc::UnboundedReceiver<crate::lsp_registry::RegistryUpdate>>,
    /// Saves waiting on `textDocument/formatting` before the disk write.
    pending_format_saves: HashMap<RequestId, PendingFormatSave>,
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
    ///
    /// Each family is handed, by value, to the one handler that owns it, and
    /// every handler matches its family exhaustively: a variant nobody routed
    /// fails to compile rather than being dropped.
    pub fn handle(&mut self, id: RequestId, command: impl Into<Command>) {
        match command.into() {
            Command::Cancel { request } => {
                // A save parked on its formatter is not a worker job, so the
                // registry alone would let it write anyway once the answer (or
                // the deadline) arrived.
                self.cancel_format_on_save(request);
                self.cancellations.cancel(request);
            },
            Command::Document(command) => self.handle_document(id, command),
            Command::Lsp(command) => self.handle_lsp(id, command),
            Command::LanguageServer(command) => self.handle_language_server(id, command),
            Command::Vcs(command) => self.handle_vcs(id, command),
            #[cfg(feature = "github")]
            Command::Github(command) => self.handle_github(id, command),
            #[cfg(not(feature = "github"))]
            Command::Github(_) => self.emit(
                Some(id),
                Event::GithubError {
                    operation: crate::api::GithubOperation::FeatureDisabled,
                    message: "the backend was built without the github feature".to_string(),
                },
            ),
            Command::Debug(command) => self.handle_debug(id, command),
            #[cfg(feature = "notebook-kernel")]
            Command::Notebook(command) => self.handle_notebook(command),
            // A build without the `notebook-kernel` feature has no kernel to
            // drive, so there is nothing to run, interrupt, or restart.
            #[cfg(not(feature = "notebook-kernel"))]
            Command::Notebook(_) => {},
            Command::Seam(command) => self.handle_seam(id, command),
            Command::Workspace(command) => self.handle_workspace(id, command),
        }
    }

    /// Serve one [`DocumentCommand`].
    fn handle_document(&mut self, id: RequestId, command: DocumentCommand) {
        match command {
            DocumentCommand::Open { path, language } => {
                self.open(Some(id), path, language.as_deref());
            },
            DocumentCommand::Close { doc } => self.close(id, doc),
            DocumentCommand::ApplyChange { doc, change, cause } => {
                self.apply(id, doc, &change, cause);
            },
            DocumentCommand::Undo { doc } => self.undo_redo(id, doc, true),
            DocumentCommand::Redo { doc } => self.undo_redo(id, doc, false),
            DocumentCommand::Save { doc, cause } => self.save(id, doc, cause),
            DocumentCommand::Retarget { doc, path } => self.retarget(Some(id), doc, path),
            DocumentCommand::BuildLatex { doc } => self.request_latex_build(id, doc),
            // The caret is UI-local; the backend keeps only the last report per
            // document, so the next edit -- the client's own, or a save's
            // rewrite -- can undo back to it. The first report after an edit
            // is also the cursor redoing that edit restores.
            DocumentCommand::SetCursor { doc, cursors, .. } => {
                if let Some(doc) = self.store.docs.get_mut(&doc) {
                    doc.buffer.note_cursor_after(&cursors);
                    doc.reported_cursor = Some((doc.buffer.version(), cursors));
                }
            },
            DocumentCommand::Convert { path } => {
                #[cfg(feature = "notebook-kernel")]
                self.warm_notebook_kernel(&path);
                self.convert_document(id, path);
            },
            DocumentCommand::FixMarkdownLint { doc } => self.fix_markdown_lint(id, doc),
            DocumentCommand::RefreshManifestHints { doc } => self.refresh_manifest_hints(doc),
            DocumentCommand::RecoverSwaps => self.recover_swaps(id),
            DocumentCommand::DiscardSwaps => self.discard_swaps(),
        }
    }

    /// Serve one [`WorkspaceCommand`].
    fn handle_workspace(&mut self, id: RequestId, command: WorkspaceCommand) {
        match command {
            WorkspaceCommand::Search {
                query,
                file_limit,
                match_limit,
            } => self.search_workspace(id, query, file_limit, match_limit),
            WorkspaceCommand::ReplaceAll { query, replacement } => {
                self.search_replace_all(id, query, replacement);
            },
            WorkspaceCommand::ScanSpelling { limit } => self.scan_workspace_spelling(id, limit),
            WorkspaceCommand::ScanTodos { limit } => self.scan_workspace_todos(id, limit),
            WorkspaceCommand::AddDictionaryWord {
                word,
                scope,
                create_project,
            } => self.add_dictionary_word(id, word, scope, create_project),
            WorkspaceCommand::ApplyEdit { edit, label } => {
                let applied = self.apply_workspace_edit(edit, Columns::Buffer, label);
                self.emit(Some(id), Event::WorkspaceEdit { applied });
            },
            WorkspaceCommand::DependencyGraph => self.emit_dependency_graph(id),
            WorkspaceCommand::LoadedConfig => self.emit(
                Some(id),
                Event::LoadedConfig {
                    report: Box::new(self.config.loaded_config.clone()),
                },
            ),
        }
    }

    /// Serve one [`SeamCommand`] on the seam worker.
    fn handle_seam(&mut self, id: RequestId, command: SeamCommand) {
        use crate::seam_worker::SeamJob;
        let job = match command {
            SeamCommand::Index { root, mode } => {
                let Some(root) = root.or_else(|| self.config.roots.first().cloned()) else {
                    // Answered, not dropped: a request is never left outstanding
                    // just because there is nothing to index (see `Backend`).
                    self.emit(
                        Some(id),
                        Event::SeamIndexFailed {
                            message: "no workspace root is open to index".to_string(),
                        },
                    );
                    return;
                };
                SeamJob::Index {
                    id,
                    root,
                    mode,
                    // The cap stops being theoretical once the root is a whole
                    // repository rather than one crate.
                    options: karet_seam::IndexOptions {
                        max_files: self.config.settings.seam.max_indexed_files,
                    },
                }
            },
            SeamCommand::Reindex { path, text } => SeamJob::Reindex { id, path, text },
            SeamCommand::Query { text } => SeamJob::Query { id, text },
            SeamCommand::Node { path } => SeamJob::Node { id, path },
            SeamCommand::SetConfiguration { name } => SeamJob::SetConfiguration { id, name },
        };
        let _ = self.seam_worker.send(job);
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
    doc.translate_semantic_tokens(edits);
    doc.translate_server_folds(edits);
    let Some(lang) = doc.lang_id else {
        // Plaintext: nothing to parse, and no worker round-trip to wait for.
        // No worker result will replace this, so ranges from a previous grammar
        // (a Markdown file saved as `.txt`) would otherwise linger on the snapshot.
        doc.syntax.clear(crate::highlight::indent_folds(
            settings,
            doc.language_selector,
            &doc.buffer,
        ));
        return true;
    };

    // Keep the spans we already have usable until the worker answers.
    if let Some(edits) = edits {
        doc.syntax.translate(edits);
    }

    let request = HighlightRequest {
        doc: doc_id,
        version: doc.buffer.version(),
        lang,
        text: doc.buffer.text_snapshot(),
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
/// A save parked on a `textDocument/formatting` answer, and when it was parked.
///
/// The clock reading is what bounds the wait: a server that accepts the request
/// and then never answers would otherwise hold the file unwritten for the
/// JSON-RPC request timeout, which is far longer than anyone expects a save to
/// take. See [`Session::expire_format_on_save`].
///
/// The bound is `editor.formatOnSaveTimeout`, swept on the session's existing
/// backup tick rather than its own timer, so the effective bound is the setting
/// plus up to one tick. The server task bounds its own await on the same
/// answer, and is handed that bound with the request
/// (`lsp::runtime::formatting_deadline`) so the two cannot drift: once the save
/// has given up there is nobody left for a reply to be delivered to, and
/// continuing to wait only keeps that server's task busy.
struct PendingFormatSave {
    /// The document whose disk write is waiting.
    doc: DocumentId,
    /// Session-clock reading, in milliseconds, when the request was dispatched.
    issued_ms: u64,
}

fn whole_document_change(doc: &Document, new_text: String) -> Option<Change> {
    let end = doc.buffer.byte_to_line_col(BytePos(doc.buffer.len_bytes()));
    let range = Range::new(LineCol::new(0, 0), end).ok()?;
    Some(Change::new(
        doc.buffer.version(),
        vec![TextEdit { range, new_text }],
    ))
}

fn edit_context(tick_ms: u64, cause: EditCause, doc: &Document, change: &Change) -> EditContext {
    let cursor_before = reported_cursor(doc).unwrap_or_else(|| {
        change.edits.first().map_or_else(CursorState::default, |e| {
            CursorState::single(Selection::caret(e.range.start))
        })
    });
    EditContext {
        tick_ms,
        cause,
        cursor_before,
    }
}

/// The context for a rewrite the session makes on its own at save time
/// (formatting, whitespace cleanup). Undoing it should put the caret back where
/// the user was, not at the rewrite's first edit -- which, for a whole-document
/// replacement, is the top of the file. See [`edit_context`] for where that is.
fn save_edit_context(tick_ms: u64, doc: &Document, change: &Change) -> EditContext {
    edit_context(tick_ms, EditCause::Replace, doc, change)
}

/// The cursor/selection set the client last reported through
/// [`Command::SetCursor`], clamped to the buffer, as long as no edit has landed
/// since it: that is the whole set an undo of the next edit restores. A stale
/// or missing report yields `None`, and the caller falls back to the first
/// edit's start -- the only position the change itself says anything about.
fn reported_cursor(doc: &Document) -> Option<CursorState> {
    let (version, cursors) = doc.reported_cursor.as_ref()?;
    if *version != doc.buffer.version() || cursors.selections.is_empty() {
        return None;
    }
    Some(clamp_cursor(&doc.buffer, cursors.clone()))
}

/// `cursors` with every anchor and head pulled onto `buffer`'s text.
pub(super) fn clamp_cursor(buffer: &TextBuffer, mut cursors: CursorState) -> CursorState {
    for selection in &mut cursors.selections {
        selection.anchor = buffer.clamp(selection.anchor);
        selection.head = buffer.clamp(selection.head);
    }
    cursors
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
