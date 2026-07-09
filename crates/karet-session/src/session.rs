//! The owned, headless editor model: [`Session`] and its read/event surface.
//!
//! A [`Session`] owns a [`DocumentStore`] of open documents, a single tree-sitter
//! [`ParserPool`] and per-language [`Highlighter`]s reused across documents, and
//! the senders for the neutral [`Event`] stream and the local snapshot stream. It
//! applies [`Command`]s synchronously (the fast paths — open/apply/save/undo — are
//! inline) and emits [`Event`]s plus [`DocSnapshot`]s.

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
use karet_syntax::Highlighter;
use karet_syntax::Highlights;
use karet_text::AppliedEdit;
use karet_text::EditCause;
use karet_text::EditContext;
use karet_text::LoadError;
use karet_text::TextBuffer;
use karet_text::TextError;
use karet_treesitter::LanguageId;
use karet_treesitter::ParserPool;
use karet_treesitter::SyntaxTree;
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
use crate::local::DocSnapshot;
use crate::local::SnapshotRx;

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
    tree: Option<SyntaxTree>,
    highlights: Arc<Highlights>,
    folds: Arc<FoldRegions>,
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
    events: mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    snapshots: mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>)>,
    store: DocumentStore,
    pool: ParserPool,
    highlighters: HashMap<LanguageId, Highlighter>,
    clock: Instant,
    /// The workspace file-watcher, kept alive for the session's lifetime.
    watcher: Option<Watcher>,
    /// The watcher's event stream, taken by [`crate::backend::local`] for the actor.
    fs_rx: Option<mpsc::UnboundedReceiver<FsEvent>>,
    /// The source-control repository for the first workspace root, if any.
    vcs: Option<Repository>,
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
        // Best-effort: a watcher failure (or no roots) just disables external-change
        // detection; editing still works.
        let (watcher, fs_rx) = if config.roots.is_empty() {
            (None, None)
        } else {
            match Watcher::spawn(&config.roots, &git_dirs) {
                Ok((w, rx)) => (Some(w), Some(rx)),
                Err(_) => (None, None),
            }
        };
        // Seed the tip so the first ref change reconciles against a known baseline.
        let last_head = vcs.as_ref().and_then(|r| r.head_hash().ok().flatten());
        // Open this session's swap store and scan for swaps a previous run left behind
        // (a crash, or a save that failed). They are offered to the UI for recovery.
        let session_id = u64::from(std::process::id());
        let swaps = config
            .swap_dir
            .clone()
            .filter(|_| config.settings.files.backup)
            .map(|dir| SwapStore::with_dir(dir, session_id));
        let pending_swaps = swaps
            .as_ref()
            .map(|store| scan(store.dir()))
            .unwrap_or_default();
        let mut session = Self {
            config,
            events,
            snapshots,
            store: DocumentStore {
                next: 1,
                ..DocumentStore::default()
            },
            pool: ParserPool::new(),
            highlighters: HashMap::new(),
            clock: Instant::now(),
            watcher,
            fs_rx,
            vcs,
            last_vcs: None,
            last_head,
            swaps,
            pending_swaps,
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
            // The caret is UI-local; `SetCursor` becomes meaningful when producers
            // (LSP at a position, multi-view sync) need it.
            Command::SetCursor { .. } => {},
            Command::Stage { paths } => self.vcs_write(id, |repo| repo.stage(&paths)),
            Command::Unstage { paths } => self.vcs_write(id, |repo| repo.unstage(&paths)),
            Command::Discard { paths } => self.vcs_write(id, |repo| repo.discard(&paths)),
            Command::StageAll => self.vcs_write(id, Repository::stage_all),
            Command::UnstageAll => self.vcs_write(id, Repository::unstage_all),
            Command::Commit { message } => self.commit(id, &message),
            Command::RefreshVcs => self.emit_vcs_status(Some(id)),
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
            // Language-intelligence and search commands are wired in later milestones.
            _ => {},
        }
    }

    // --- source control ---------------------------------------------------

    /// Fetch a page of the commit log and emit it. Requests one extra commit to
    /// detect whether more remain, then trims to `limit`. A no-op without a repo.
    /// A requested page tags the answering event with `id`; a spontaneous reload
    /// (`id` is `None`) makes the client reset its loaded log to this first page.
    fn emit_vcs_log(&mut self, id: Option<RequestId>, skip: usize, limit: usize) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match repo.log(skip, limit.saturating_add(1)) {
            Ok(mut commits) => {
                let has_more = commits.len() > limit;
                commits.truncate(limit);
                self.emit(
                    id,
                    Event::VcsLog {
                        skip,
                        commits,
                        has_more,
                    },
                );
            },
            Err(e) => self.emit(
                id,
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Load one commit's full detail plus the files it changed, and emit them together.
    /// A read failure (e.g. an unknown revision) becomes a VCS notification. No-op
    /// without a repository.
    fn emit_commit_detail(&mut self, id: RequestId, rev: &str) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match (repo.commit_detail(rev), repo.commit_changes(rev)) {
            (Ok(detail), Ok(changes)) => {
                self.emit(
                    Some(id),
                    Event::CommitReady {
                        detail: Box::new(detail),
                        changes,
                    },
                );
            },
            (Err(e), _) | (_, Err(e)) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Resolve a [`RangeSpec`] against the repository (upstream / base branch / merge
    /// base) and emit the diff between the two points as [`Event::RangeReady`]. A
    /// resolution failure — no upstream, no detectable base, a bad revision, or unrelated
    /// histories — becomes a VCS notification. No-op without a repository.
    fn emit_range_changes(&mut self, id: RequestId, spec: RangeSpec) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        // Resolve and compute everything that needs the repo borrow up front, into owned
        // data, so the `self.emit` below is free to borrow `self` mutably.
        let outcome: Result<(String, String, bool, Vec<FileChange>), String> = (|| {
            let (base_rev, head_rev, merge_base, base_label, head_label) = match &spec {
                RangeSpec::Unpushed => {
                    let up = repo
                        .upstream_of_head()
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| {
                            "no upstream branch is set for the current branch".to_string()
                        })?;
                    (up.clone(), "HEAD".to_string(), true, up, "HEAD".to_string())
                },
                RangeSpec::SinceBase { base } => {
                    let b = base
                        .clone()
                        .or_else(|| repo.default_base_branch())
                        .ok_or_else(|| {
                            "could not determine a base branch; use a range like main...HEAD"
                                .to_string()
                        })?;
                    (b.clone(), "HEAD".to_string(), true, b, "HEAD".to_string())
                },
                RangeSpec::Between {
                    base,
                    head,
                    merge_base,
                } => (
                    base.clone(),
                    head.clone(),
                    *merge_base,
                    base.clone(),
                    head.clone(),
                ),
            };
            let changes = repo
                .range_changes(&base_rev, &head_rev, merge_base)
                .map_err(|e| e.to_string())?;
            Ok((base_label, head_label, merge_base, changes))
        })();
        match outcome {
            Ok((base_label, head_label, merge_base, changes)) => self.emit(
                Some(id),
                Event::RangeReady {
                    base_label,
                    head_label,
                    merge_base,
                    changes,
                },
            ),
            Err(message) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message,
                },
            ),
        }
    }

    /// Fetch a page of a file's history and emit it, requesting one extra commit to
    /// detect whether more remain. No-op without a repository.
    fn emit_file_history(&mut self, id: RequestId, path: PathBuf, skip: usize, limit: usize) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match repo.file_history(&path, skip, limit.saturating_add(1)) {
            Ok(mut commits) => {
                let has_more = commits.len() > limit;
                commits.truncate(limit);
                self.emit(
                    Some(id),
                    Event::FileHistory {
                        path,
                        skip,
                        commits,
                        has_more,
                    },
                );
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Lazily fetch a commit's GitHub "Verified" status on a worker thread, emitting an
    /// [`Event::CommitVerification`] on success. Silent on any failure (offline, no
    /// GitHub remote, rate-limited): the client simply keeps the offline "Signed" badge.
    /// A no-op when the `github` feature is disabled.
    #[cfg(feature = "github")]
    fn fetch_commit_verification(&mut self, id: RequestId, hash: String) {
        let Some(url) = self.vcs.as_ref().and_then(Repository::origin_url) else {
            return;
        };
        let Some((owner, repo)) = karet_github::parse_remote(&url) else {
            return;
        };
        let events = self.events.clone();
        // Blocking HTTP off the actor thread; drop the handle (fire-and-forget).
        std::thread::spawn(move || {
            if let Ok(v) = karet_github::commit_verification(&owner, &repo, &hash) {
                let status = GithubVerification {
                    verified: v.verified,
                    reason: v.reason,
                    signer: v.signer,
                };
                events
                    .send((Some(id), Event::CommitVerification { hash, status }))
                    .ok();
            }
        });
    }

    /// Without the `github` feature, commit verification is unavailable — a no-op.
    #[cfg(not(feature = "github"))]
    fn fetch_commit_verification(&mut self, _id: RequestId, _hash: String) {}

    /// Reconcile the commit log after a filesystem event. Reads the (cheap) `HEAD`
    /// hash; if the tip moved, prepends only the new commits, falling back to a fresh
    /// first page when history was rewritten or too many commits arrived at once.
    fn reconcile_vcs_log(&mut self) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        let head = repo.head_hash().ok().flatten();
        if head == self.last_head {
            return; // The tip is unchanged — nothing to do.
        }
        let prev = self.last_head.take();
        self.last_head = head.clone();
        // The branch became unborn (e.g. a hard reset to before the first commit):
        // there is nothing to prepend, and the client's next open will refetch.
        if head.is_none() {
            return;
        }
        match repo.commits_since(prev.as_deref(), LOG_RECONCILE_CAP) {
            // A clean, bounded set of new commits anchored on a known tip → prepend.
            Ok(commits)
                if prev.is_some() && !commits.is_empty() && commits.len() < LOG_RECONCILE_CAP =>
            {
                self.emit(None, Event::VcsCommitsPrepended { commits });
            },
            // No prior anchor, or history was rewritten / a large batch arrived:
            // emit a fresh first page so the client resets its log cleanly.
            Ok(commits) if !commits.is_empty() => self.emit_vcs_log(None, 0, LOG_RELOAD_PAGE),
            // Tip moved but no newer commits (e.g. checkout to an ancestor): refresh.
            Ok(_) => self.emit_vcs_log(None, 0, LOG_RELOAD_PAGE),
            Err(_) => {},
        }
    }

    /// Compute the current `(staged, working)` change sets, or `None` when there is
    /// no repository. A read failure yields empty sets rather than erroring.
    fn compute_vcs(&self) -> Option<(Vec<FileChange>, Vec<FileChange>)> {
        let repo = self.vcs.as_ref()?;
        let staged = repo.changes(VcsSelection::Staged, None).unwrap_or_default();
        let working = repo
            .changes(VcsSelection::Unstaged, None)
            .unwrap_or_default();
        Some((staged, working))
    }

    /// Recompute the source-control status and emit it. A requested refresh (`id`
    /// set) always emits; a spontaneous one (from a filesystem event) emits only
    /// when the status changed, collapsing event bursts and absorbing the feedback
    /// from the session's own index writes.
    fn emit_vcs_status(&mut self, id: Option<RequestId>) {
        let Some(status) = self.compute_vcs() else {
            return;
        };
        if id.is_none() && self.last_vcs.as_ref() == Some(&status) {
            return;
        }
        let (staged, working) = status.clone();
        self.last_vcs = Some(status);
        self.emit(id, Event::VcsStatus { staged, working });
    }

    /// Run a write action against the repository, then force a fresh status (so the
    /// user always sees the result of their action). Failures surface as an
    /// [`Event::Notification`].
    fn vcs_write(
        &mut self,
        id: RequestId,
        action: impl FnOnce(&Repository) -> Result<(), VcsError>,
    ) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match action(repo) {
            Ok(()) => {
                self.last_vcs = None;
                self.emit_vcs_status(Some(id));
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Commit the staged changes, emitting [`Event::Committed`] then a fresh status,
    /// or a [`Event::Notification`] on failure (e.g. conflicts or no identity).
    fn commit(&mut self, id: RequestId, message: &str) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match repo.commit(message) {
            Ok(oid) => {
                self.emit(Some(id), Event::Committed { oid });
                self.last_vcs = None;
                self.emit_vcs_status(Some(id));
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// The session's configuration (workspace roots, format-on-save, spell-check).
    #[must_use]
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// Take the file-watcher and its event stream, to be driven by the actor.
    ///
    /// The watcher is returned (rather than kept on the session) so the actor can
    /// hold it alive for exactly as long as it is consuming events.
    pub(crate) fn take_watch(
        &mut self,
    ) -> (Option<Watcher>, Option<mpsc::UnboundedReceiver<FsEvent>>) {
        (self.watcher.take(), self.fs_rx.take())
    }

    /// React to a debounced filesystem event by reloading or flagging any open
    /// document whose file changed underneath it.
    pub(crate) fn handle_fs_event(&mut self, event: FsEvent) {
        if event.kind == karet_watch::FsEventKind::WatchDegraded {
            self.emit(
                None,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Io,
                    message: "filesystem watch limit reached; some paths are polled".to_string(),
                },
            );
            return;
        }
        for path in &event.paths {
            if let Some(&doc_id) = self.store.by_path.get(path) {
                self.on_external_change(doc_id, path);
            }
        }
        // A generic "something changed" signal for anything else the client
        // derives from the workspace (e.g. a live-updating search) — distinct
        // from the specific reactions below, which only cover open documents and
        // VCS state.
        self.emit(
            None,
            Event::FsChanged {
                paths: event.paths.clone(),
            },
        );
        // Any worktree edit or watched git-metadata change can alter status. The
        // event is already debounced and the emit is change-gated, so a burst (and
        // the session's own index writes) collapse to at most one update.
        self.emit_vcs_status(None);
        // A watched `refs/**` / `HEAD` change may mean new commits; reconcile the log
        // incrementally. The head read is cheap and this early-returns when unchanged.
        self.reconcile_vcs_log();
    }

    /// Borrow a read-only view of a document for local-mode rendering or tests.
    #[must_use]
    pub fn document(&self, doc: DocumentId) -> Option<DocumentView<'_>> {
        let d = self.store.docs.get(&doc)?;
        Some(DocumentView {
            buffer: &d.buffer,
            highlights: d.highlights.as_ref(),
            decorations: d.decorations.as_slice(),
            version: d.buffer.version(),
        })
    }

    // --- command handlers -------------------------------------------------

    fn open(&mut self, id: RequestId, path: PathBuf, language: Option<&str>) {
        if let Some(&existing) = self.store.by_path.get(&path) {
            if let Some(doc) = self.store.docs.get_mut(&existing) {
                doc.refs += 1;
                let version = doc.buffer.version();
                self.emit(
                    Some(id),
                    Event::Opened {
                        doc: existing,
                        version,
                    },
                );
                self.publish(existing, None);
            }
            return;
        }
        let (buffer, format) = match load_document(&path) {
            Ok(loaded) => loaded,
            Err(LoadError::NotUtf8 { .. }) => {
                // Full non-UTF-8 editing isn't supported; tell the client so it can
                // fall back to a read-only view instead of leaving this path's tab
                // registered with no document forever.
                self.emit(Some(id), Event::NotUtf8 { path });
                return;
            },
            Err(e) => {
                self.emit(
                    Some(id),
                    Event::Notification {
                        severity: Severity::Error,
                        kind: NotificationKind::Io,
                        message: format!("could not open {}: {e}", path.display()),
                    },
                );
                return;
            },
        };
        let lang_id = language_id_from_path(&path);
        let language = language
            .and_then(name_for_language)
            .or_else(|| language_name_from_path(&path));
        let doc_id = DocumentId(self.store.next);
        self.store.next += 1;
        let mut doc = Document {
            path: path.clone(),
            language,
            lang_id,
            buffer,
            format,
            tree: None,
            highlights: Arc::new(Highlights::default()),
            folds: Arc::new(FoldRegions::default()),
            decorations: Vec::new(),
            refs: 1,
            dirty_since: None,
            backed_up_version: None,
        };
        update_syntax(&mut self.pool, &mut self.highlighters, &mut doc, None);
        let version = doc.buffer.version();
        self.store.by_path.insert(path, doc_id);
        self.store.docs.insert(doc_id, doc);
        self.emit(
            Some(id),
            Event::Opened {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id, None);
    }

    fn apply(&mut self, id: RequestId, doc_id: DocumentId, change: &Change, cause: EditCause) {
        let tick = self.elapsed_ms();
        let ctx = edit_context(tick, cause, change);
        // `None` means the change was stale or overlapping (the client's local
        // speculative state has diverged from ours); either way we still publish
        // below so the authoritative buffer flows back down to the client instead
        // of leaving it stuck rejecting every future edit forever.
        let version = {
            let pool = &mut self.pool;
            let highlighters = &mut self.highlighters;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                self.events.send((Some(id), unknown_document(doc_id))).ok();
                return;
            };
            match doc.buffer.apply(change, ctx) {
                Ok(applied) => {
                    update_syntax(pool, highlighters, doc, Some(&applied.edits));
                    // Arm the backup clock on the clean→dirty transition (see
                    // `backup_tick`).
                    doc.sync_dirty_since(tick);
                    // LSP seam: this is the single apply site. When a server is
                    // attached for `doc.lang_id`, forward an incremental
                    // `did_change(&doc.path, version, change.edits)` here
                    // (translated to the negotiated encoding); a no-op while no
                    // server is attached.
                    Some(applied.version)
                },
                Err(_) => None,
            }
        };
        match version {
            Some(version) => self.emit(
                Some(id),
                Event::Applied {
                    doc: doc_id,
                    version,
                },
            ),
            None => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Io,
                    message: "edit couldn't be applied — refreshing from disk".to_string(),
                },
            ),
        }
        self.publish(doc_id, None);
    }

    fn undo_redo(&mut self, id: RequestId, doc_id: DocumentId, undo: bool) {
        let tick = self.elapsed_ms();
        let (version, cursor) = {
            let pool = &mut self.pool;
            let highlighters = &mut self.highlighters;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                return;
            };
            let applied = if undo {
                doc.buffer.undo()
            } else {
                doc.buffer.redo()
            };
            let Some(applied) = applied else {
                return; // nothing to undo/redo
            };
            update_syntax(pool, highlighters, doc, Some(&applied.edits));
            // Undoing back to the save point clears dirtiness (and any pending backup).
            doc.sync_dirty_since(tick);
            // Jump the caret to the change: undo restores the exact pre-edit cursor;
            // redo (which records none) lands at the end of the re-applied edit that
            // reaches furthest into the document.
            let cursor = applied.restored_cursor.clone().or_else(|| {
                applied
                    .edits
                    .iter()
                    .max_by_key(|e| e.new_end_byte)
                    .map(|e| {
                        let pos = doc.buffer.byte_to_line_col(BytePos(e.new_end_byte));
                        CursorState::single(Selection::caret(pos))
                    })
            });
            (applied.version, cursor)
        };
        self.emit(
            Some(id),
            Event::Applied {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id, cursor);
    }

    fn save(&mut self, id: RequestId, doc_id: DocumentId) {
        let result = self.store.docs.get_mut(&doc_id).map(save_document);
        match result {
            Some(Ok(_)) => {
                // The file is safely on disk: drop the backup and disarm the clock.
                if let Some(doc) = self.store.docs.get_mut(&doc_id) {
                    doc.dirty_since = None;
                    doc.backed_up_version = None;
                    let path = doc.path.clone();
                    if let Some(store) = self.swaps.as_ref() {
                        store.remove(&path);
                    }
                }
                self.emit(Some(id), Event::Saved { doc: doc_id });
            },
            Some(Err(TextError::Conflict)) => {
                // The file changed on disk since it was last read — writing now would
                // silently clobber someone else's change. Back up the in-memory edits
                // (same as any other failed save) and let the client prompt the user,
                // reusing the same event an external change to a dirty doc already
                // triggers reactively.
                self.write_swap(doc_id);
                self.emit(Some(id), Event::ExternalConflict { doc: doc_id });
            },
            Some(Err(e)) => {
                // A failed save is exactly when a backup matters most: capture the
                // unsaved buffer to a swap immediately, then surface the error.
                self.write_swap(doc_id);
                self.emit(
                    Some(id),
                    Event::Notification {
                        severity: Severity::Error,
                        kind: NotificationKind::Io,
                        message: format!("save failed (unsaved changes backed up): {e}"),
                    },
                );
            },
            None => self.emit(Some(id), unknown_document(doc_id)),
        }
    }

    fn close(&mut self, id: RequestId, doc_id: DocumentId) {
        let removed = match self.store.docs.get_mut(&doc_id) {
            Some(doc) => {
                doc.refs = doc.refs.saturating_sub(1);
                doc.refs == 0
            },
            None => return,
        };
        if removed {
            if let Some(doc) = self.store.docs.remove(&doc_id) {
                self.store.by_path.remove(&doc.path);
                // The document is gone from the editor: skipping a save is an explicit
                // decision, so clean up its swap.
                if let Some(store) = self.swaps.as_ref() {
                    store.remove(&doc.path);
                }
            }
            self.emit(Some(id), Event::Closed { doc: doc_id });
        }
    }

    // --- crash-recovery swaps ---------------------------------------------

    /// Announce the swaps left by previous sessions so the UI can prompt the user to
    /// recover or discard them. A no-op when there are none.
    fn announce_pending_swaps(&mut self) {
        if self.pending_swaps.is_empty() {
            return;
        }
        let swaps = self
            .pending_swaps
            .iter()
            .map(|record| SwapInfo {
                original: record.meta.original.clone(),
                updated_unix_ms: record.meta.updated_unix_ms,
                conflict: record.conflicts_with_disk(),
            })
            .collect();
        self.emit(None, Event::SwapsFound { swaps });
    }

    /// Recover every pending swap: open its document and replace the buffer with the
    /// backed-up (unsaved) content, leaving it dirty so the user re-saves. Each swap
    /// file is removed once its content is restored; a swap whose original cannot be
    /// opened is left on disk for a later attempt.
    fn recover_swaps(&mut self, id: RequestId) {
        for record in std::mem::take(&mut self.pending_swaps) {
            self.open(id, record.meta.original.clone(), None);
            let Some(&doc_id) = self.store.by_path.get(&record.meta.original) else {
                continue;
            };
            let change = self
                .store
                .docs
                .get(&doc_id)
                .and_then(|doc| whole_document_change(doc, record.content.clone()));
            if let Some(change) = change {
                self.apply(id, doc_id, &change, EditCause::Replace);
                discard(&record.swap_path);
            }
        }
    }

    /// Discard every pending swap without recovering (the user declined).
    fn discard_swaps(&mut self) {
        for record in std::mem::take(&mut self.pending_swaps) {
            discard(&record.swap_path);
        }
    }

    // --- visualizations ---------------------------------------------------

    /// Build the workspace package-dependency graph and emit it, or surface a failure
    /// (no lockfile / parse error) as a notification.
    fn emit_dependency_graph(&mut self, id: RequestId) {
        let Some(root) = self.config.roots.first() else {
            return;
        };
        match crate::viz::dependency_graph(root) {
            Ok(view) => {
                let title = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                self.emit(
                    Some(id),
                    Event::GraphReady {
                        kind: crate::api::GraphKind::Dependency,
                        title,
                        view,
                    },
                );
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::System,
                    message: format!("dependency graph: {e}"),
                },
            ),
        }
    }

    /// Write a crash-recovery swap for `doc_id` immediately (used when a save fails).
    fn write_swap(&mut self, doc_id: DocumentId) {
        let Session { swaps, store, .. } = self;
        let Some(swap_store) = swaps.as_ref() else {
            return;
        };
        let Some(doc) = store.docs.get_mut(&doc_id) else {
            return;
        };
        let (hash, size) = doc
            .buffer
            .saved_state()
            .map(|s| (Some(s.hash), Some(s.size)))
            .unwrap_or((None, None));
        let version = doc.buffer.version();
        if swap_store
            .write(&doc.path, &doc.buffer.text(), hash, size, version)
            .is_ok()
        {
            doc.backed_up_version = Some(version);
        }
    }

    /// Back up every document that has been dirty past the configured backup interval
    /// (and changed since its last swap). Called on a timer by the backend actor.
    pub(crate) fn backup_tick(&mut self) {
        let Session {
            swaps,
            store,
            config,
            clock,
            ..
        } = self;
        if !config.settings.files.backup {
            return;
        }
        let Some(store_ref) = swaps.as_ref() else {
            return;
        };
        let interval = config.settings.files.backup_interval;
        let now = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        for doc in store.docs.values_mut() {
            if !doc.buffer.is_dirty() {
                continue;
            }
            let Some(since) = doc.dirty_since else {
                continue;
            };
            if now.saturating_sub(since) < interval {
                continue;
            }
            let version = doc.buffer.version();
            if doc.backed_up_version == Some(version) {
                continue; // already backed up at this version
            }
            let (hash, size) = doc
                .buffer
                .saved_state()
                .map(|s| (Some(s.hash), Some(s.size)))
                .unwrap_or((None, None));
            if store_ref
                .write(&doc.path, &doc.buffer.text(), hash, size, version)
                .is_ok()
            {
                doc.backed_up_version = Some(version);
            }
        }
    }

    /// Decide what an external change to `doc_id`'s file means: ignore our own
    /// write, reload a clean buffer, or flag a conflict on a dirty one.
    fn on_external_change(&mut self, doc_id: DocumentId, path: &Path) {
        // Our own save? Compare the on-disk stat to the fingerprint we recorded when
        // we last read or wrote the file (match on size + mtime, never inode — an
        // atomic save renames a new inode over the target).
        let our_write = match (std::fs::metadata(path), self.store.docs.get(&doc_id)) {
            (Ok(meta), Some(doc)) => doc.buffer.saved_state().is_some_and(|saved| {
                meta.len() == saved.size && meta.modified().is_ok_and(|m| m == saved.mtime)
            }),
            _ => false,
        };
        if our_write {
            return;
        }
        if self
            .store
            .docs
            .get(&doc_id)
            .is_some_and(|d| d.buffer.is_dirty())
        {
            self.emit(None, Event::ExternalConflict { doc: doc_id });
        } else {
            self.reload(doc_id);
        }
    }

    /// Reload a clean document from disk (history reset, version bumped), then emit
    /// [`Event::Reloaded`] and publish the fresh snapshot.
    fn reload(&mut self, doc_id: DocumentId) {
        let version = {
            let pool = &mut self.pool;
            let highlighters = &mut self.highlighters;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                return;
            };
            let Ok((fresh, _)) = load_document(&doc.path) else {
                return; // file vanished or became unreadable; leave the buffer as-is
            };
            doc.buffer.adopt_content(fresh);
            doc.tree = None;
            update_syntax(pool, highlighters, doc, None);
            doc.buffer.version()
        };
        self.emit(
            None,
            Event::Reloaded {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id, None);
    }

    // --- helpers ----------------------------------------------------------

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn emit(&self, id: Option<RequestId>, event: Event) {
        self.events.send((id, event)).ok();
    }

    /// Push a render-only snapshot of `doc_id` on the snapshot stream. `cursor` is
    /// `Some` only for undo/redo, carrying the caret the editor should jump to; every
    /// other publish passes `None` and leaves the UI's cursor untouched.
    fn publish(&self, doc_id: DocumentId, cursor: Option<CursorState>) {
        if let Some(doc) = self.store.docs.get(&doc_id) {
            let snapshot = Arc::new(DocSnapshot {
                version: doc.buffer.version(),
                buffer: doc.buffer.content_snapshot(),
                highlights: doc.highlights.clone(),
                folds: doc.folds.clone(),
                decorations: Arc::new(doc.decorations.clone()),
                language: doc.language,
                dirty: doc.buffer.is_dirty(),
                cursor,
            });
            self.snapshots.send((doc_id, snapshot)).ok();
        }
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
    pool: &mut ParserPool,
    highlighters: &mut HashMap<LanguageId, Highlighter>,
    doc: &mut Document,
    edits: Option<&[AppliedEdit]>,
) {
    let Some(lang) = doc.lang_id else {
        doc.tree = None;
        doc.highlights = Arc::new(Highlights::default());
        doc.folds = Arc::new(FoldRegions::default());
        return;
    };

    let mut reparsed = false;
    if let (Some(edits), Some(tree)) = (edits, doc.tree.as_mut()) {
        for ae in edits {
            tree.edit(&to_edit(ae));
        }
        reparsed = tree
            .reparse_with(pool, |byte| doc.buffer.byte_chunk(byte))
            .is_ok();
    }

    let text = doc.buffer.text();
    if !reparsed {
        doc.tree = SyntaxTree::parse(pool, lang, &text).ok();
    }

    ensure_highlighter(highlighters, lang);
    doc.highlights = match (doc.tree.as_ref(), highlighters.get(&lang)) {
        (Some(tree), Some(hl)) => Arc::new(hl.highlight(tree, &text).unwrap_or_default()),
        _ => Arc::new(Highlights::default()),
    };
    // Fold regions are grammar-agnostic (any multi-line node), so they come straight
    // off the tree with no per-language highlighter.
    doc.folds = match doc.tree.as_ref() {
        Some(tree) => Arc::new(karet_syntax::fold(tree)),
        None => Arc::new(FoldRegions::default()),
    };
}

/// Compile and cache a highlighter for `lang` if one is not present. A language
/// with no compiled-in grammar simply leaves the slot empty (retried each call).
fn ensure_highlighter(highlighters: &mut HashMap<LanguageId, Highlighter>, lang: LanguageId) {
    use std::collections::hash_map::Entry;
    if let Entry::Vacant(slot) = highlighters.entry(lang)
        && let Ok(highlighter) = Highlighter::new(lang)
    {
        slot.insert(highlighter);
    }
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

#[cfg(test)]
mod tests {
    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::TextEdit;

    use super::*;
    use crate::api::Command;

    fn write_temp(name: &str, body: &str) -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join(name);
        std::fs::write(&path, body).ok()?;
        Some((dir, path))
    }

    fn opened_doc(events: &mut EventRx) -> Option<DocumentId> {
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Opened { doc, .. } = ev {
                found = Some(doc);
            }
        }
        found
    }

    #[test]
    fn session_constructs_with_streams() {
        let (_session, _events, _snaps) = Session::new(SessionConfig::default());
    }

    #[test]
    fn opening_a_non_utf8_file_reports_not_utf8_instead_of_a_generic_error() {
        let Some((_dir, path)) = write_temp("bad.rs", "") else {
            return;
        };
        if std::fs::write(&path, [0x66, 0x6e, 0xff, 0x00]).is_err() {
            return;
        }
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let mut not_utf8_path = None;
        let mut opened = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::NotUtf8 { path } => not_utf8_path = Some(path),
                Event::Opened { .. } => opened = true,
                _ => {},
            }
        }
        assert_eq!(not_utf8_path, Some(path));
        assert!(!opened, "a non-UTF-8 file must not report as Opened");
        assert!(
            snaps.try_recv().is_none(),
            "no document was registered, so no snapshot should follow"
        );
    }

    #[test]
    fn open_apply_save_undo_flow() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let doc = opened_doc(&mut events);
        assert!(doc.is_some(), "expected an Opened event");
        let Some(doc) = doc else { return };
        assert!(snaps.try_recv().is_some(), "open publishes a snapshot");

        // Insert "!" after the body's closing brace position (line 0, col 11).
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 12),
                    end: LineCol::new(0, 12),
                },
                new_text: "\nfn x() {}".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        // Applied event with version 1.
        let mut applied_version = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Applied { version, .. } = ev {
                applied_version = Some(version);
            }
        }
        assert_eq!(applied_version, Some(1));
        // A fresh snapshot reflects the edit.
        let mut last_snap = None;
        while let Some((_, s)) = snaps.try_recv() {
            last_snap = Some(s);
        }
        assert!(last_snap.is_some(), "expected a snapshot after apply");
        let Some(snap) = last_snap else { return };
        assert_eq!(snap.version, 1);
        assert!(snap.dirty);
        // "fn main() {}\n" + inserted "\nfn x() {}" → three lines.
        assert_eq!(snap.buffer.line_count(), 3);

        // Save: the file on disk reflects the edit and the doc goes clean.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Saved { .. } = ev {
                saved = true;
            }
        }
        assert!(saved);
        assert!(
            session
                .document(doc)
                .is_some_and(|v| !v.buffer().is_dirty())
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap_or_default()
                .contains("fn x()")
        );

        // Undo restores the original content ("fn main() {}\n" → two lines).
        session.handle(RequestId(4), Command::Undo { doc });
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().line_count() == 2)
        );
    }

    #[test]
    fn save_refuses_to_overwrite_a_file_changed_on_disk_since_it_was_read() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        let _ = snaps.try_recv();

        // Dirty the in-memory buffer without touching the file yet.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "// edited\n".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}
        let _ = snaps.try_recv();

        // Someone else changes the file on disk before we save.
        if std::fs::write(&path, "fn main() { /* external */ }\n").is_err() {
            return;
        }

        session.handle(RequestId(3), Command::Save { doc });
        let mut conflict = false;
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::ExternalConflict { .. } => conflict = true,
                Event::Saved { .. } => saved = true,
                _ => {},
            }
        }
        assert!(
            conflict,
            "save must report an ExternalConflict, not just fail silently"
        );
        assert!(!saved, "a conflicting save must not report as Saved");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            "fn main() { /* external */ }\n",
            "a refused save must not overwrite the externally-changed file"
        );
        // The in-memory edit is still there (unsaved, not discarded).
        assert!(session.document(doc).is_some_and(|v| v.buffer().is_dirty()));
    }

    #[test]
    fn apply_against_a_stale_version_resyncs_instead_of_dropping_silently() {
        // Regression: a client whose local speculative version has diverged from
        // the backend's (e.g. after a dropped/duplicate message) used to have its
        // edit silently discarded with no way to recover — every subsequent edit
        // on that document would then fail the same way forever. It must instead
        // be told and get a fresh snapshot back so it can resync.
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        let _ = snaps.try_recv(); // drain the open snapshot

        // Base the change on a version that doesn't exist yet (the real base is 0).
        let change = Change::new(
            7,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: "!".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );

        let mut notified = false;
        let mut applied = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Notification { .. } => notified = true,
                Event::Applied { .. } => applied = true,
                _ => {},
            }
        }
        assert!(notified, "a stale-version conflict must notify the client");
        assert!(!applied, "the rejected edit must not report as Applied");
        assert!(
            snaps.try_recv().is_some(),
            "the client must still get a fresh snapshot to resync from, not be left stuck"
        );
        // The document itself is untouched by the rejected edit.
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().text() == "fn main() {}\n")
        );
    }

    #[test]
    fn undo_redo_snapshot_carries_caret_but_edits_do_not() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };

        // Helper: drain the snapshot stream and return the most recent snapshot.
        fn drain(snaps: &mut SnapshotRx) -> Option<std::sync::Arc<DocSnapshot>> {
            let mut last = None;
            while let Some((_, s)) = snaps.try_recv() {
                last = Some(s);
            }
            last
        }
        let _ = drain(&mut snaps); // discard the open snapshot

        // An ordinary edit publishes a snapshot with no caret (the UI owns the caret).
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(1, 0),
                    end: LineCol::new(1, 0),
                },
                new_text: "fn x() {}\n".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        assert_eq!(
            drain(&mut snaps).and_then(|s| s.cursor.clone()),
            None,
            "an ordinary edit must not carry a caret"
        );

        // Undo publishes a snapshot that carries the caret to jump to.
        session.handle(RequestId(3), Command::Undo { doc });
        assert!(
            drain(&mut snaps).is_some_and(|s| s.cursor.is_some()),
            "undo must carry a caret so the editor jumps to the change"
        );

        // Redo (which records no cursor) still carries a derived caret at the edit.
        session.handle(RequestId(4), Command::Redo { doc });
        assert!(
            drain(&mut snaps).is_some_and(|s| s.cursor.is_some()),
            "redo must carry a caret derived from the re-applied edit"
        );
    }

    #[test]
    fn cbor_opens_decoded_and_save_reencodes() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("data.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let Ok(bytes) = karet_cbor::encode(&original) else {
            return;
        };
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }

        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        // The buffer holds decoded diagnostic notation, not the raw CBOR bytes.
        let text = session.document(doc).map(|v| v.buffer().text());
        assert_eq!(text.as_deref(), Some("[\n  1,\n  2\n]"));
        while snaps.try_recv().is_some() {}

        // Edit the "2" (line 2, col 2) to "3".
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(2, 2),
                    end: LineCol::new(2, 3),
                },
                new_text: "3".to_string(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}

        // Save re-encodes to CBOR; the file on disk decodes to the edited value.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Saved { .. } = ev {
                saved = true;
            }
        }
        assert!(saved, "a cbor save should succeed");
        let disk = std::fs::read(&path).unwrap_or_default();
        let expected = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(3),
        ]);
        assert_eq!(karet_cbor::decode(&disk).ok(), Some(expected));
    }

    #[test]
    fn cbor_save_of_malformed_edit_leaves_file_untouched() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("bad.cbor");
        let original = karet_cbor::CborValue::Array(vec![
            karet_cbor::CborValue::Integer(1),
            karet_cbor::CborValue::Integer(2),
        ]);
        let Ok(bytes) = karet_cbor::encode(&original) else {
            return;
        };
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };

        // Delete the closing ']' (line 3, col 0), making the text un-parseable.
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(3, 0),
                    end: LineCol::new(3, 1),
                },
                new_text: String::new(),
            }],
        );
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change,
                cause: EditCause::Replace,
            },
        );
        while events.try_recv().is_some() {}

        // Save fails to encode; no Saved event, and the file is unchanged.
        session.handle(RequestId(3), Command::Save { doc });
        let mut saved = false;
        let mut failed = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Saved { .. } => saved = true,
                Event::Notification {
                    severity: Severity::Error,
                    ..
                } => failed = true,
                _ => {},
            }
        }
        assert!(!saved, "a malformed cbor buffer must not save");
        assert!(
            failed,
            "the failure should surface as an error notification"
        );
        assert_eq!(
            std::fs::read(&path).unwrap_or_default(),
            bytes,
            "the file is untouched"
        );
    }

    #[test]
    fn external_change_reloads_clean_buffer() {
        let Some((_dir, path)) = write_temp("ext.txt", "one\n") else {
            return;
        };
        let (mut session, mut events, mut snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        while snaps.try_recv().is_some() {}

        // The file changes on disk (the buffer is clean, so this should reload).
        let _ = std::fs::write(&path, "one\ntwo\n");
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![path],
        });

        let mut reloaded = false;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::Reloaded { .. } = ev {
                reloaded = true;
            }
        }
        assert!(reloaded, "a clean external change should reload");
        assert!(
            session
                .document(doc)
                .is_some_and(|v| v.buffer().line_count() == 3)
        );
        // The reload bumped the version (kept monotonic) and a snapshot was published.
        assert!(snaps.try_recv().is_some());
    }

    #[test]
    fn open_dedups_by_path_and_refcounts_close() {
        let Some((_dir, path)) = write_temp("a.txt", "hi\n") else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        // Second open of the same path reuses the document.
        session.handle(
            RequestId(2),
            Command::OpenDocument {
                path,
                language: None,
            },
        );
        let same = opened_doc(&mut events);
        assert_eq!(same, Some(doc));
        // Two opens → two refs; one close keeps it, the second drops it.
        session.handle(RequestId(3), Command::CloseDocument { doc });
        assert!(session.document(doc).is_some());
        session.handle(RequestId(4), Command::CloseDocument { doc });
        assert!(session.document(doc).is_none());
    }

    /// Initialize a temp git repository with one untracked `a.txt`, returning the
    /// temp dir, its root path, and the repo-relative file path. `None` if `git`
    /// isn't available.
    fn init_temp_repo() -> Option<(tempfile::TempDir, PathBuf, PathBuf)> {
        let dir = tempfile::tempdir().ok()?;
        let root = dir.path().to_path_buf();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        run(&["init", "-q"])?;
        run(&["config", "user.email", "test@example.com"])?;
        run(&["config", "user.name", "karet test"])?;
        std::fs::write(root.join("a.txt"), "hello\n").ok()?;
        Some((dir, root, PathBuf::from("a.txt")))
    }

    /// Drain the queued events and return the most recent [`Event::VcsStatus`].
    fn latest_vcs_status(events: &mut EventRx) -> Option<(Vec<FileChange>, Vec<FileChange>)> {
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::VcsStatus { staged, working } = ev {
                found = Some((staged, working));
            }
        }
        found
    }

    #[test]
    fn staging_through_the_session_updates_status() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        // The actor normally calls this; here we drive the session directly.
        session.start();

        // The session seeds an initial status: the file is untracked in `working`.
        let Some((staged, working)) = latest_vcs_status(&mut events) else {
            return;
        };
        assert!(staged.is_empty());
        assert!(
            working
                .iter()
                .any(|c| c.path == file && c.status == karet_vcs::StatusKind::Untracked)
        );

        // Stage it → a fresh status with the file staged as Added.
        session.handle(
            RequestId(1),
            Command::Stage {
                paths: vec![file.clone()],
            },
        );
        let Some((staged, _working)) = latest_vcs_status(&mut events) else {
            return;
        };
        assert!(
            staged
                .iter()
                .any(|c| c.path == file && c.status == karet_vcs::StatusKind::Added)
        );
    }

    #[test]
    fn commit_detail_and_file_history_round_trip() {
        let Some((_dir, root, file)) = init_temp_repo() else {
            return;
        };
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        // One commit touching a.txt, one touching only b.txt.
        if run(&["add", "a.txt"]).is_none() || run(&["commit", "-q", "-m", "add a"]).is_none() {
            return;
        }
        std::fs::write(root.join("b.txt"), "b\n").ok();
        run(&["add", "b.txt"]);
        run(&["commit", "-q", "-m", "add b"]);
        // The app passes the file's absolute path (a relative path would resolve
        // against the process CWD, not the repo root — see `Repository::file_history`).
        let file_abs = root.join(&file);

        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        while events.try_recv().is_some() {} // drain the seeded status/log

        // CommitDetail(HEAD) answers with the "add b" commit and its single change.
        session.handle(
            RequestId(1),
            Command::CommitDetail {
                rev: "HEAD".to_string(),
            },
        );
        let mut ready = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::CommitReady { detail, changes } = ev {
                ready = Some((detail, changes));
            }
        }
        let Some((detail, changes)) = ready else {
            return;
        };
        assert_eq!(detail.summary, "add b");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("b.txt"));

        // FileHistory(a.txt) answers with exactly the "add a" commit.
        session.handle(
            RequestId(2),
            Command::FileHistory {
                path: file_abs,
                skip: 0,
                limit: 10,
            },
        );
        let mut hist = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::FileHistory { commits, .. } = ev {
                hist = Some(commits);
            }
        }
        let Some(commits) = hist else {
            return;
        };
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "add a");
    }

    #[test]
    fn range_changes_between_two_revs_round_trip() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .ok()
                .filter(std::process::ExitStatus::success)
        };
        // c0 adds a.txt; c1 modifies a.txt and adds b.txt.
        if run(&["add", "a.txt"]).is_none() || run(&["commit", "-q", "-m", "c0"]).is_none() {
            return;
        }
        std::fs::write(root.join("a.txt"), "hello\nworld\n").ok();
        std::fs::write(root.join("b.txt"), "b\n").ok();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "c1"]);

        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root],
            ..SessionConfig::default()
        });
        session.start();
        while events.try_recv().is_some() {} // drain the seeded status/log

        // A two-dot HEAD~1..HEAD range answers with a.txt (modified) and b.txt (added).
        session.handle(
            RequestId(1),
            Command::RangeChanges {
                spec: RangeSpec::Between {
                    base: "HEAD~1".to_string(),
                    head: "HEAD".to_string(),
                    merge_base: false,
                },
            },
        );
        let mut ready = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::RangeReady {
                base_label,
                head_label,
                changes,
                ..
            } = ev
            {
                ready = Some((base_label, head_label, changes));
            }
        }
        let Some((base_label, head_label, changes)) = ready else {
            return;
        };
        assert_eq!(base_label, "HEAD~1");
        assert_eq!(head_label, "HEAD");
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from("a.txt")));
        assert!(paths.contains(&PathBuf::from("b.txt")));

        // Unpushed with no configured upstream is a graceful notification, not a panic.
        session.handle(
            RequestId(2),
            Command::RangeChanges {
                spec: RangeSpec::Unpushed,
            },
        );
        let mut notified = false;
        let mut range_ready = false;
        while let Some((_, ev)) = events.try_recv() {
            match ev {
                Event::Notification {
                    kind: NotificationKind::Vcs,
                    ..
                } => {
                    notified = true;
                },
                Event::RangeReady { .. } => range_ready = true,
                _ => {},
            }
        }
        assert!(notified, "no upstream yields a VCS notification");
        assert!(!range_ready, "an unresolvable range emits no RangeReady");
    }

    #[test]
    fn filesystem_event_refreshes_vcs_status() {
        let Some((_dir, root, _file)) = init_temp_repo() else {
            return;
        };
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: vec![root.clone()],
            ..SessionConfig::default()
        });
        // The actor normally calls this; here we drive the session directly.
        session.start();
        // Initial status: just the seeded `a.txt`.
        let Some((_staged, working)) = latest_vcs_status(&mut events) else {
            return;
        };
        assert_eq!(working.len(), 1);

        // A new file appears on disk; the debounced watcher would deliver this event.
        if std::fs::write(root.join("b.txt"), "hi\n").is_err() {
            return;
        }
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Created,
            paths: vec![root.join("b.txt")],
        });

        // The recompute re-emits a status that now lists both untracked files.
        let refreshed = latest_vcs_status(&mut events);
        assert!(refreshed.is_some(), "fs event should refresh the status");
        if let Some((_staged, working)) = refreshed {
            assert_eq!(working.len(), 2);
        }
    }

    #[test]
    fn filesystem_event_emits_fs_changed_with_the_affected_paths() {
        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        let path = PathBuf::from("/work/touched.rs");
        session.handle_fs_event(karet_watch::FsEvent {
            kind: karet_watch::FsEventKind::Modified,
            paths: vec![path.clone()],
        });
        let mut seen = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::FsChanged { paths } = ev {
                seen = Some(paths);
            }
        }
        assert_eq!(seen, Some(vec![path]));
    }

    /// A whole-buffer insertion at the start of the document (base version 0).
    fn insert_change(text: &str) -> Change {
        Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 0),
                    end: LineCol::new(0, 0),
                },
                new_text: text.to_string(),
            }],
        )
    }

    #[test]
    fn backup_tick_writes_a_swap_for_a_dirty_doc_and_save_removes_it() {
        let Some((_dir, path)) = write_temp("main.rs", "fn main() {}\n") else {
            return;
        };
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let mut settings = crate::config::Settings::default();
        settings.files.backup_interval = 0; // any dirty doc is immediately due
        let (mut session, mut events, _snaps) = Session::new(SessionConfig {
            roots: Vec::new(),
            settings,
            swap_dir: None,
        });
        // Redirect swaps to a temp directory instead of the real data dir.
        session.swaps = Some(SwapStore::with_dir(swapdir.path().to_path_buf(), 1));

        session.handle(
            RequestId(1),
            Command::OpenDocument {
                path: path.clone(),
                language: None,
            },
        );
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        session.handle(
            RequestId(2),
            Command::ApplyChange {
                doc,
                change: insert_change("x"),
                cause: EditCause::Replace,
            },
        );

        // No swap until the tick decides the doc is due.
        assert!(scan(swapdir.path()).is_empty());
        session.backup_tick();
        assert_eq!(scan(swapdir.path()).len(), 1, "dirty doc backed up");

        // A successful save clears the swap.
        session.handle(RequestId(3), Command::Save { doc });
        assert!(scan(swapdir.path()).is_empty(), "save removes the swap");
    }

    #[test]
    fn recover_swaps_restores_a_dirty_buffer() {
        let Some((_dir, path)) = write_temp("r.rs", "on disk\n") else {
            return;
        };
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 9);
        // A swap left by a previous session holds unsaved content.
        if store.write(&path, "recovered!\n", None, None, 1).is_err() {
            return;
        }

        let (mut session, mut events, _snaps) = Session::new(SessionConfig::default());
        session.swaps = Some(SwapStore::with_dir(swapdir.path().to_path_buf(), 9));
        session.pending_swaps = scan(swapdir.path());
        assert_eq!(session.pending_swaps.len(), 1);

        session.recover_swaps(RequestId(1));
        let Some(doc) = opened_doc(&mut events) else {
            return;
        };
        let Some(document) = session.store.docs.get(&doc) else {
            return;
        };
        assert_eq!(document.buffer.text(), "recovered!\n");
        assert!(document.buffer.is_dirty(), "recovered content is unsaved");
        // The swap is consumed once recovered.
        assert!(scan(swapdir.path()).is_empty());
    }

    #[test]
    fn new_session_announces_swaps_left_in_its_swap_dir() {
        let Some(swapdir) = tempfile::tempdir().ok() else {
            return;
        };
        let store = SwapStore::with_dir(swapdir.path().to_path_buf(), 5);
        if store
            .write(Path::new("/work/x.rs"), "unsaved\n", None, None, 1)
            .is_err()
        {
            return;
        }
        // A session pointed at that swap dir scans it on construction and announces.
        let (_session, mut events, _snaps) = Session::new(SessionConfig {
            roots: Vec::new(),
            settings: crate::config::Settings::default(),
            swap_dir: Some(swapdir.path().to_path_buf()),
        });
        let mut found = None;
        while let Some((_, ev)) = events.try_recv() {
            if let Event::SwapsFound { swaps } = ev {
                found = Some(swaps);
            }
        }
        assert!(found.is_some(), "startup announces recoverable swaps");
        if let Some(swaps) = found {
            assert_eq!(swaps.len(), 1);
            assert_eq!(swaps[0].original, PathBuf::from("/work/x.rs"));
        }
    }
}
