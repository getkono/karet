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

mod documents;
mod persistence;
mod updates;
mod vcs;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use karet_core::BytePos;
use karet_core::Change;
use karet_core::CursorState;
use karet_core::Decoration;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_core::Selection;
use karet_core::Severity;
use karet_core::TextEdit;
use karet_filetype::FileKind;
use karet_filetype::classify_ignoring_size;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
use karet_text::AppliedEdit;
use karet_text::EditCause;
use karet_text::EditContext;
use karet_text::LoadError;
use karet_text::TextBuffer;
use karet_text::TextError;
use karet_treesitter::LanguageId;
use karet_treesitter::language_id_from_path;
use karet_treesitter::language_name_from_path;
use karet_vcs::FileChange;
use karet_vcs::Repository;
use karet_vcs::Selection as VcsSelection;
use karet_vcs::VcsError;
use karet_watch::FsEvent;
use karet_watch::Watcher;
use tokio::sync::mpsc;

use crate::api::Command;
use crate::api::DocumentId;
use crate::api::Event;
#[cfg(feature = "github")]
use crate::api::GithubVerification;
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

/// Errors produced by the backend session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// A command referenced a document that is not open.
    #[error("unknown document")]
    UnknownDocument,
    /// An underlying engine reported an error.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Configuration for a [`Session`].
#[derive(Clone, Debug, Default)]
pub struct SessionConfig {
    /// Workspace root directories.
    pub roots: Vec<PathBuf>,
    /// The loaded, verified settings (see [`crate::config`]). Producers read editing
    /// behaviour (format-on-save, spell-check, …) from here.
    pub settings: crate::config::Settings,
    /// The loaded settings plus layer and explicit-key provenance for inspection.
    pub loaded_config: crate::config::LoadedConfig,
    /// Directory for crash-recovery swap files. The application sets this to the real
    /// user data directory ([`crate::backup::default_swap_dir`]); left `None` (as in
    /// tests) the session keeps no backups and never touches the user's data dir.
    pub swap_dir: Option<PathBuf>,
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
    Cbor,
}

/// How many leading bytes to sample when classifying a document's on-disk format.
const CLASSIFY_HEAD: usize = 8192;

/// Load `path` into an editable buffer, decoding a known binary format (CBOR) to
/// text, and report the [`DocFormat`] to re-encode with on save.
///
/// The buffer records the on-disk fingerprint of the *original* bytes so the
/// file-watcher can still recognize the editor's own writes.
fn load_document(path: &Path) -> Result<(TextBuffer, DocFormat), LoadError> {
    let bytes = std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?;
    let head = &bytes[..bytes.len().min(CLASSIFY_HEAD)];
    // Format detection ignores the size guard: once the session is asked to open a
    // document it must decode it correctly regardless of size (the guard is an
    // app-level *routing* choice), so a large CBOR still decodes rather than being
    // mistaken for plain text.
    if classify_ignoring_size(path, head) == FileKind::Cbor {
        let text = karet_cbor::decode_to_text(&bytes).map_err(|e| LoadError::Io(e.to_string()))?;
        let mut buffer = TextBuffer::from_text(&text);
        buffer.record_disk_state(path, &bytes);
        Ok((buffer, DocFormat::Cbor))
    } else {
        let mut buffer = TextBuffer::from_bytes(&bytes)?;
        buffer.record_disk_state(path, &bytes);
        Ok((buffer, DocFormat::Text))
    }
}

/// Save `doc` to disk, re-encoding a decoded binary format (CBOR) from its edit
/// text. A CBOR encode error (e.g. malformed diagnostic notation after editing)
/// leaves the file untouched and surfaces as a save failure. Returns
/// [`TextError::Conflict`] distinctly (rather than a generic IO error) so the
/// caller can prompt the user instead of just reporting a failure.
fn save_document(doc: &mut Document) -> Result<(), TextError> {
    match doc.format {
        DocFormat::Text => doc.buffer.save(&doc.path).map(|_| ()),
        DocFormat::Cbor => {
            let text = doc.buffer.text();
            let bytes =
                karet_cbor::encode_from_text(&text).map_err(|e| TextError::Io(e.to_string()))?;
            doc.buffer.save_bytes(&doc.path, &bytes).map(|_| ())
        },
    }
}

/// One open document and its derived state.
struct Document {
    path: PathBuf,
    language: Option<&'static str>,
    lang_id: Option<LanguageId>,
    buffer: TextBuffer,
    /// How the buffer is (de)serialized on disk.
    format: DocFormat,
    /// The last highlights the worker produced, translated across any edits applied
    /// since. The parsed trees themselves live on the worker, not here.
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
    /// Semantic block scopes produced by the syntax worker for this version.
    semantic_blocks: Arc<SemanticBlocks>,
    /// Syntax-error line ranges from the worker's last parse (see
    /// [`DocSnapshot::syntax_error_lines`]).
    error_lines: Arc<Vec<(u32, u32)>>,
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
    clock: Instant,
    /// The workspace file-watcher, kept alive for the session's lifetime.
    watcher: Option<Watcher>,
    /// The watcher's event stream, taken by [`crate::backend::local`] for the actor.
    fs_rx: Option<mpsc::UnboundedReceiver<FsEvent>>,
    /// The source-control repository for the first workspace root, if any.
    vcs: Option<Repository>,
    /// Ordered background repository actions and network reads.
    vcs_worker: std::sync::mpsc::Sender<crate::vcs_worker::VcsJob>,
    /// The last emitted `(staged, working)` status. Spontaneous recomputes (from
    /// filesystem events) emit only when this changes, which absorbs the feedback
    /// from the session's own index writes.
    last_vcs: Option<(Vec<FileChange>, Vec<FileChange>)>,
    /// The last observed `HEAD` commit hash. A filesystem event that moves the tip
    /// away from this triggers an incremental commit-log reconciliation.
    last_head: Option<String>,
    /// This session's crash-recovery swap store (`None` if no data directory).
    swaps: Option<SwapStore>,
    /// Swaps found on startup awaiting the user's recover/discard decision.
    pending_swaps: Vec<SwapRecord>,
    /// Language-server orchestration (lazy per-language tasks; see [`crate::lsp`]).
    lsp: LspManager,
    /// The LSP tasks' results, taken by [`crate::backend::local`] for the actor.
    lsp_rx: Option<mpsc::UnboundedReceiver<LspUpdate>>,
}

/// The most new commits [`Session::reconcile_vcs_log`] will prepend at once. Beyond
/// this the history is assumed rewritten (rebase/force-push) and the log is reloaded.
const LOG_RECONCILE_CAP: usize = 256;

/// The first-page size used when a reconciliation falls back to a full log reload.
const LOG_RELOAD_PAGE: usize = 25;

impl Session {
    /// Create a session and its paired event and snapshot receivers.
    #[must_use]
    pub fn new(config: SessionConfig) -> (Self, EventRx, SnapshotRx) {
        let (events, erx) = mpsc::unbounded_channel();
        let (snapshots, srx) = mpsc::unbounded_channel();
        // Discover the source-control repository (from the first root) before the
        // watcher, so its git-metadata directories can be watched for index/HEAD/refs
        // changes — that is what keeps the status fresh after external `git` commands.
        let vcs = config
            .roots
            .first()
            .and_then(|root| Repository::discover(root).ok());
        let git_dirs = vcs
            .as_ref()
            .map(Repository::metadata_dirs)
            .unwrap_or_default();
        let config_manager = ConfigManager::from_loaded(&config.loaded_config);
        let config_paths = config_manager
            .as_ref()
            .map(ConfigManager::paths)
            .unwrap_or_default();
        // Best-effort: a watcher failure (or no roots) just disables external-change
        // detection; editing still works.
        let (watcher, fs_rx) = if config.roots.is_empty() && config_paths.is_empty() {
            (None, None)
        } else {
            match Watcher::spawn_with_paths(&config.roots, &git_dirs, &config_paths) {
                Ok((w, rx)) => (Some(w), Some(rx)),
                Err(_) => (None, None),
            }
        };
        // Seed the tip so the first ref change reconciles against a known baseline.
        let last_head = vcs.as_ref().and_then(|r| r.head_hash().ok().flatten());
        let vcs_worker = crate::vcs_worker::spawn(config.roots.first().cloned(), events.clone());
        // Open this session's swap store and scan for swaps a previous run left behind
        // (a crash, or a save that failed). They are offered to the UI for recovery.
        let session_id = u64::from(std::process::id());
        let swaps = config
            .swap_dir
            .clone()
            .map(|dir| SwapStore::with_dir(dir, session_id));
        let pending_swaps = if config.settings.files.backup {
            swaps
                .as_ref()
                .map(|store| scan(store.dir()))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // Layered highlighting runs on its own thread; the actor only sends it text and
        // applies the spans it sends back. Each request carries the document's resolved
        // semantic-comment settings, so language overrides can update live.
        let (highlight_tx, highlight_rx) = crate::highlight::spawn();
        // Language servers spawn lazily, per language, on the first matching open.
        let (lsp, lsp_rx) =
            LspManager::new(config.settings.lsp.clone(), config.roots.first().cloned());
        let mut session = Self {
            config,
            config_manager,
            events,
            snapshots,
            store: DocumentStore {
                next: 1,
                ..DocumentStore::default()
            },
            highlight_tx,
            highlight_rx: Some(highlight_rx),
            clock: Instant::now(),
            watcher,
            fs_rx,
            vcs,
            vcs_worker,
            last_vcs: None,
            last_head,
            swaps,
            pending_swaps,
            lsp,
            lsp_rx: Some(lsp_rx),
        };
        // Announce any recoverable swaps so the UI can prompt on the first frame.
        session.announce_pending_swaps();
        (session, EventRx(erx), SnapshotRx(srx))
    }

    /// Kick off the work deferred until the session is actually being driven: the
    /// initial VCS status. Computing it eagerly in [`Session::new`] would run a full
    /// `git status` on the construction thread — for a huge repository that can stall
    /// the caller before the UI ever renders. Call this once, from the actor task that
    /// drives [`Session::handle`]/[`Session::handle_fs_event`], so it runs
    /// concurrently with the first frame instead of blocking it.
    pub(crate) fn start(&mut self) {
        // Seed the client with the initial status; it buffers until the UI reads it.
        self.emit_vcs_status(None);
    }

    /// Handle one request. The editing fast paths resolve inline; the answering
    /// [`Event`] is tagged with `id`.
    pub fn handle(&mut self, id: RequestId, command: Command) {
        match command {
            Command::OpenDocument { path, language } => self.open(id, path, language.as_deref()),
            Command::CloseDocument { doc } => self.close(id, doc),
            Command::ApplyChange { doc, change, cause } => self.apply(id, doc, &change, cause),
            Command::Undo { doc } => self.undo_redo(id, doc, true),
            Command::Redo { doc } => self.undo_redo(id, doc, false),
            Command::Save { doc } => self.save(id, doc),
            Command::RetargetDocument { doc, path } => self.retarget(id, doc, path),
            // The caret is UI-local; `SetCursor` becomes meaningful when producers
            // (LSP at a position, multi-view sync) need it.
            Command::SetCursor { .. } => {},
            Command::Stage { paths } => self.vcs_write(id, |repo| repo.stage(&paths)),
            Command::Unstage { paths } => self.vcs_write(id, |repo| repo.unstage(&paths)),
            Command::Discard { paths } => self.vcs_write(id, |repo| repo.discard(&paths)),
            Command::StageAll => self.vcs_write(id, Repository::stage_all),
            Command::UnstageAll => self.vcs_write(id, Repository::unstage_all),
            Command::Commit { message } => self.commit(id, &message),
            Command::GenerateCommitMessage => self.generate_commit_message(id),
            Command::RefreshVcs => self.emit_vcs_status(Some(id)),
            Command::RepositorySnapshot => {
                let _ = self
                    .vcs_worker
                    .send(crate::vcs_worker::VcsJob::Snapshot { id });
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
                let _ = self
                    .vcs_worker
                    .send(crate::vcs_worker::VcsJob::PullRequests {
                        id,
                        remote,
                        page,
                        per_page,
                    });
            },
            Command::Blame { doc, version, line } => self.request_blame(id, doc, version, line),
            Command::VcsLog { skip, limit } => self.emit_vcs_log(Some(id), skip, limit),
            Command::CommitDetail { rev } => self.emit_commit_detail(id, &rev),
            Command::RangeChanges { spec } => self.emit_range_changes(id, spec),
            Command::FileHistory { path, skip, limit } => {
                self.emit_file_history(id, path, skip, limit)
            },
            Command::FetchCommitVerification { hash } => self.fetch_commit_verification(id, hash),
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
            // The remaining language-intelligence and search commands are wired in
            // later milestones.
            _ => {},
        }
    }

    // --- source control ---------------------------------------------------

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
        });
    }
}
/// Re-(or incrementally) parse `doc` and recompute its highlights.
///
/// When `edits` is `Some` and a tree already exists, the tree is edited in place
/// and reparsed incrementally (streaming the rope, no whole-file `String`);
/// otherwise a full parse runs. Highlights are recomputed against the resulting
/// tree (the query still materializes the text — the rope-native query is a
/// follow-up).
fn update_syntax(
    settings: &crate::config::Settings,
    highlight_tx: &std::sync::mpsc::Sender<HighlightJob>,
    doc_id: DocumentId,
    doc: &mut Document,
    edits: Option<&[AppliedEdit]>,
) {
    let Some(lang) = doc.lang_id else {
        // Plaintext: nothing to parse, and no worker round-trip to wait for.
        doc.highlights = Arc::new(Highlights::default());
        doc.folds = Arc::new(FoldRegions::default());
        doc.semantic_blocks = Arc::new(SemanticBlocks::default());
        return;
    };

    // Keep the spans we already have usable until the worker answers. Rendering them
    // unshifted would smear color across the text the edit moved.
    if let Some(edits) = edits {
        // Block scopes are line-based and cannot be translated safely across an
        // arbitrary edit. Hide them briefly rather than render stale source context.
        doc.semantic_blocks = Arc::new(SemanticBlocks::default());
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
                .for_language(doc.language)
                .semantic_comments();
            semantic
                .enabled()
                .then(|| karet_syntax::SemanticCommentConfig {
                    tags: semantic.tags().to_vec(),
                })
        },
        edits: edits.map(|es| es.iter().map(to_edit).collect()),
    };
    // A dead worker only means no highlights; editing carries on.
    highlight_tx.send(HighlightJob::Update(request)).ok();
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

/// Convert a `karet-text` applied edit into the parse host's neutral edit.
fn to_edit(ae: &AppliedEdit) -> karet_treesitter::Edit {
    karet_treesitter::Edit {
        start_byte: ae.start_byte,
        old_end_byte: ae.old_end_byte,
        new_end_byte: ae.new_end_byte,
        start_point: ae.start_point,
        old_end_point: ae.old_end_point,
        new_end_point: ae.new_end_point,
    }
}

/// Map an explicit LSP-style language id (e.g. `"rust"`) to karet's display name,
/// when one is supplied on open.
fn name_for_language(_id: &str) -> Option<&'static str> {
    // The display name is derived from the path today; an explicit override table
    // lands with the LSP language registry.
    None
}

fn unknown_document(doc: DocumentId) -> Event {
    Event::Notification {
        severity: Severity::Error,
        kind: NotificationKind::System,
        message: format!("unknown document {}", doc.0),
    }
}

/// A read-only borrow of a document's renderable state (local mode).
///
/// In a future remote split this is replaced by a client-side snapshot replicated
/// from [`Event`]s; the renderer (`karet-editor`) consumes the same data either way.
pub struct DocumentView<'a> {
    buffer: &'a TextBuffer,
    highlights: &'a Highlights,
    decorations: &'a [Decoration],
    version: u64,
}

impl DocumentView<'_> {
    /// The document's text buffer.
    #[must_use]
    pub fn buffer(&self) -> &TextBuffer {
        self.buffer
    }

    /// The document's syntax highlights.
    #[must_use]
    pub fn highlights(&self) -> &Highlights {
        self.highlights
    }

    /// The document's decorations (merged across producers).
    #[must_use]
    pub fn decorations(&self) -> &[Decoration] {
        self.decorations
    }

    /// The document's current version.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
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
