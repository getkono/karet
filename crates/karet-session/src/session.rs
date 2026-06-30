//! The owned, headless editor model: [`Session`] and its read/event surface.
//!
//! A [`Session`] owns a [`DocumentStore`] of open documents, a single tree-sitter
//! [`ParserPool`] and per-language [`Highlighter`]s reused across documents, and
//! the senders for the neutral [`Event`] stream and the local snapshot stream. It
//! applies [`Command`]s synchronously (the fast paths — open/apply/save/undo — are
//! inline) and emits [`Event`]s plus [`DocSnapshot`]s.

use crate::api::{Command, DocumentId, Event, RequestId};
use crate::local::{DocSnapshot, SnapshotRx};
use karet_core::{Change, CursorState, Decoration, Selection};
use karet_syntax::{Highlighter, Highlights};
use karet_text::{AppliedEdit, EditCause, EditContext, TextBuffer};
use karet_treesitter::{
    LanguageId, ParserPool, SyntaxTree, language_id_from_path, language_name_from_path,
};
use karet_watch::{FsEvent, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

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
    /// Run format-on-save.
    pub format_on_save: bool,
    /// Enable spell-checking of comments/strings.
    pub spellcheck: bool,
}

/// One open document and its derived state.
struct Document {
    path: PathBuf,
    language: Option<&'static str>,
    lang_id: Option<LanguageId>,
    buffer: TextBuffer,
    tree: Option<SyntaxTree>,
    highlights: Arc<Highlights>,
    decorations: Vec<Decoration>,
    /// Open reference count (a path opened in N views shares one document).
    refs: u32,
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
}

impl Session {
    /// Create a session and its paired event and snapshot receivers.
    #[must_use]
    pub fn new(config: SessionConfig) -> (Self, EventRx, SnapshotRx) {
        let (events, erx) = mpsc::unbounded_channel();
        let (snapshots, srx) = mpsc::unbounded_channel();
        // Best-effort: a watcher failure (or no roots) just disables external-change
        // detection; editing still works.
        let (watcher, fs_rx) = if config.roots.is_empty() {
            (None, None)
        } else {
            match Watcher::spawn(&config.roots, &[]) {
                Ok((w, rx)) => (Some(w), Some(rx)),
                Err(_) => (None, None),
            }
        };
        let session = Self {
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
        };
        (session, EventRx(erx), SnapshotRx(srx))
    }

    /// Handle one request. The editing fast paths resolve inline; the answering
    /// [`Event`] is tagged with `id`.
    pub fn handle(&mut self, id: RequestId, command: Command) {
        match command {
            Command::OpenDocument { path, language } => self.open(id, path, language.as_deref()),
            Command::CloseDocument { doc } => self.close(id, doc),
            Command::ApplyChange { doc, change } => self.apply(id, doc, &change),
            Command::Undo { doc } => self.undo_redo(id, doc, true),
            Command::Redo { doc } => self.undo_redo(id, doc, false),
            Command::Save { doc } => self.save(id, doc),
            // The caret is UI-local; `SetCursor` becomes meaningful when producers
            // (LSP at a position, multi-view sync) need it.
            Command::SetCursor { .. } => {}
            // Language-intelligence and search commands are wired in later milestones.
            _ => {}
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
        for path in &event.paths {
            if let Some(&doc_id) = self.store.by_path.get(path) {
                self.on_external_change(doc_id, path);
            }
        }
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
                self.publish(existing);
            }
            return;
        }
        let buffer = match TextBuffer::load(&path) {
            Ok(b) => b,
            Err(e) => {
                self.emit(
                    Some(id),
                    Event::Progress {
                        message: format!("could not open {}: {e}", path.display()),
                        percent: None,
                    },
                );
                return;
            }
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
            tree: None,
            highlights: Arc::new(Highlights::default()),
            decorations: Vec::new(),
            refs: 1,
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
        self.publish(doc_id);
    }

    fn apply(&mut self, id: RequestId, doc_id: DocumentId, change: &Change) {
        let tick = self.elapsed_ms();
        let ctx = edit_context(tick, change);
        let version = {
            let pool = &mut self.pool;
            let highlighters = &mut self.highlighters;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                self.events.send((Some(id), unknown_document(doc_id))).ok();
                return;
            };
            let applied = match doc.buffer.apply(change, ctx) {
                Ok(a) => a,
                Err(_) => return, // stale or overlapping; the caller will resync
            };
            update_syntax(pool, highlighters, doc, Some(&applied.edits));
            // LSP seam: this is the single apply site. When a server is attached for
            // `doc.lang_id`, forward an incremental `did_change(&doc.path, version,
            // change.edits)` here (translated to the negotiated encoding); a no-op
            // while no server is attached.
            applied.version
        };
        self.emit(
            Some(id),
            Event::Applied {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id);
    }

    fn undo_redo(&mut self, id: RequestId, doc_id: DocumentId, undo: bool) {
        let version = {
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
            applied.version
        };
        self.emit(
            Some(id),
            Event::Applied {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id);
    }

    fn save(&mut self, id: RequestId, doc_id: DocumentId) {
        let result = match self.store.docs.get_mut(&doc_id) {
            Some(doc) => Some(doc.buffer.save(&doc.path)),
            None => None,
        };
        match result {
            Some(Ok(_)) => self.emit(Some(id), Event::Saved { doc: doc_id }),
            Some(Err(e)) => self.emit(
                Some(id),
                Event::Progress {
                    message: format!("save failed: {e}"),
                    percent: None,
                },
            ),
            None => self.emit(Some(id), unknown_document(doc_id)),
        }
    }

    fn close(&mut self, id: RequestId, doc_id: DocumentId) {
        let removed = match self.store.docs.get_mut(&doc_id) {
            Some(doc) => {
                doc.refs = doc.refs.saturating_sub(1);
                doc.refs == 0
            }
            None => return,
        };
        if removed {
            if let Some(doc) = self.store.docs.remove(&doc_id) {
                self.store.by_path.remove(&doc.path);
            }
            self.emit(Some(id), Event::Closed { doc: doc_id });
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
            let Ok(fresh) = TextBuffer::load(&doc.path) else {
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
        self.publish(doc_id);
    }

    // --- helpers ----------------------------------------------------------

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn emit(&self, id: Option<RequestId>, event: Event) {
        self.events.send((id, event)).ok();
    }

    fn publish(&self, doc_id: DocumentId) {
        if let Some(doc) = self.store.docs.get(&doc_id) {
            let snapshot = Arc::new(DocSnapshot {
                version: doc.buffer.version(),
                buffer: doc.buffer.content_snapshot(),
                highlights: doc.highlights.clone(),
                decorations: Arc::new(doc.decorations.clone()),
                language: doc.language,
                dirty: doc.buffer.is_dirty(),
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
fn edit_context(tick_ms: u64, change: &Change) -> EditContext {
    let cursor_before = change.edits.first().map_or_else(CursorState::default, |e| {
        CursorState::single(Selection::caret(e.range.start))
    });
    let cause = if is_single_char_insert(change) {
        EditCause::Type
    } else {
        EditCause::Replace
    };
    EditContext {
        tick_ms,
        cause,
        cursor_before,
    }
}

/// Whether `change` is a single insertion of exactly one non-newline `char`.
fn is_single_char_insert(change: &Change) -> bool {
    let [edit] = change.edits.as_slice() else {
        return false;
    };
    if edit.range.start != edit.range.end {
        return false;
    }
    let mut chars = edit.new_text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c != '\n')
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
    Event::Progress {
        message: format!("unknown document {}", doc.0),
        percent: None,
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
    use super::*;
    use crate::api::Command;
    use karet_core::{Change, LineCol, Range, TextEdit};

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
        session.handle(RequestId(2), Command::ApplyChange { doc, change });
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
}
