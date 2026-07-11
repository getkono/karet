//! Local-mode renderable snapshots and the snapshot stream.
//!
//! In local mode the UI renders from owned [`DocSnapshot`]s the session pushes on
//! the snapshot stream, rather than borrowing a [`DocumentView`](crate::session::DocumentView)
//! across the actor task boundary. A snapshot is cheap to produce — the buffer
//! clone shares the rope (O(1) structural sharing) and highlights/decorations are
//! `Arc`-shared — so a snapshot can be minted on every applied edit.
//!
//! Snapshots ride a dedicated local channel rather than [`Event`](crate::api::Event)
//! so the neutral protocol in [`api`](crate::api) stays serialization-friendly for a
//! future remote split (a remote client reconstructs its own replica from the
//! `Change`-bearing events instead).

use std::sync::Arc;

use karet_core::CursorState;
use karet_core::Decoration;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_text::TextBuffer;
use tokio::sync::mpsc;

use crate::api::DocumentId;

/// An owned, render-only snapshot of a document at a particular version.
#[derive(Clone)]
pub struct DocSnapshot {
    /// The document version this snapshot reflects.
    pub version: u64,
    /// A render-only buffer clone (shares the rope; carries no history).
    pub buffer: TextBuffer,
    /// Syntax highlights for this version.
    pub highlights: Arc<Highlights>,
    /// Foldable regions for this version (tree-sitter fold ranges).
    pub folds: Arc<FoldRegions>,
    /// Decorations merged across producers (empty until producers attach).
    pub decorations: Arc<Vec<Decoration>>,
    /// Inclusive 0-based line ranges covered by syntax errors in the last
    /// computed parse (tree-sitter `ERROR`/missing nodes). Like
    /// [`highlights`](Self::highlights), this lags edits by one background
    /// worker round-trip. Drives the completion auto-trigger's
    /// "no outright errors on the caret's line" gate.
    pub syntax_error_lines: Arc<Vec<(u32, u32)>>,
    /// The display language name, if detected.
    pub language: Option<&'static str>,
    /// Whether the buffer has unsaved changes.
    pub dirty: bool,
    /// A caret to move the editor to when this snapshot is applied. Set only for
    /// undo/redo (so the caret jumps to the edit site); `None` for ordinary
    /// publishes, which leave the editor's cursor where the UI placed it.
    pub cursor: Option<CursorState>,
}

/// The receiving half of the local snapshot stream (one entry per renderable
/// change, coalescable last-per-document by the UI).
pub struct SnapshotRx(pub(crate) mpsc::UnboundedReceiver<(DocumentId, Arc<DocSnapshot>)>);

impl SnapshotRx {
    /// Await the next snapshot, or `None` once the session has shut down.
    pub async fn recv(&mut self) -> Option<(DocumentId, Arc<DocSnapshot>)> {
        self.0.recv().await
    }

    /// Take the next ready snapshot without awaiting, or `None` if none is queued.
    pub fn try_recv(&mut self) -> Option<(DocumentId, Arc<DocSnapshot>)> {
        self.0.try_recv().ok()
    }
}
