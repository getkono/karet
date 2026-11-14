//! Local-mode renderable snapshots and the snapshot stream.
//!
//! In local mode the UI renders from owned [`DocSnapshot`]s the session pushes on
//! the snapshot stream, rather than borrowing document state
//! across the actor task boundary. A snapshot is cheap to produce — the buffer
//! clone shares the rope (O(1) structural sharing) and highlights/folds are
//! `Arc`-shared — so a snapshot can be minted on every applied edit.
//!
//! Snapshots ride a dedicated local channel rather than [`Event`](crate::api::Event)
//! so the neutral protocol in [`api`](crate::api) stays serialization-friendly for a
//! future remote split (a remote client reconstructs its own replica from the
//! `Change`-bearing events instead).

use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use karet_core::CursorState;
use karet_core::FoldRegions;
use karet_core::Highlights;
use karet_core::SemanticBlocks;
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
    /// Foldable regions for this version: the primary language server's
    /// folding ranges when it offers them, else tree-sitter's fold ranges (or
    /// indentation's, for text no grammar parses).
    pub folds: Arc<FoldRegions>,
    /// Semantic block headers and scope ranges for sticky-scroll rendering.
    pub semantic_blocks: Arc<SemanticBlocks>,
    /// Inclusive 0-based line ranges covered by syntax errors in the last
    /// computed parse (tree-sitter `ERROR`/missing nodes). Like
    /// [`highlights`](Self::highlights), this lags edits by one background
    /// worker round-trip. Drives the completion auto-trigger's
    /// "no outright errors on the caret's line" gate.
    pub syntax_error_lines: Arc<Vec<(u32, u32)>>,
    /// Inclusive 0-based line ranges occupied by GFM tables, read from the
    /// highlight worker's parse (tree-sitter-md `pipe_table` nodes, so a table
    /// shape inside a fenced code block is not one). Empty for a language with
    /// no tables. Like [`syntax_error_lines`](Self::syntax_error_lines), this
    /// lags edits by one background worker round-trip: it describes the last
    /// version the worker parsed, which is [`version`](Self::version) once
    /// highlighting has caught up.
    pub table_lines: Arc<Vec<RangeInclusive<u32>>>,
    /// The display language name, if detected.
    pub language: Option<&'static str>,
    /// Whether the buffer has unsaved changes.
    pub dirty: bool,
    /// The full selection set to restore when this snapshot is applied. Set
    /// only for undo/redo, and meant only for the view that issued the
    /// command: undo restores the set from before the undone step, redo the
    /// set the client reported (through `SetCursor`) right after the step.
    /// `None` for ordinary publishes.
    ///
    /// The snapshot does not carry the change set itself: a view whose text
    /// is older than [`version`](Self::version) derives it from its own text
    /// and [`buffer`](Self::buffer), and maps its carets through it.
    pub cursor: Option<CursorState>,
}

/// The sending half of the snapshot stream: each snapshot travels with the
/// connection position (`seq`) of the backend frame it was rebuilt from, or 0
/// when it has none (a local session's, or a remote client's own echo).
pub(crate) type SnapshotTx = mpsc::UnboundedSender<(DocumentId, Arc<DocSnapshot>, u64)>;

/// The receiving half of the local snapshot stream (one entry per renderable
/// change, coalescable last-per-document by the UI).
pub struct SnapshotRx {
    rx: mpsc::UnboundedReceiver<(DocumentId, Arc<DocSnapshot>, u64)>,
    /// The newest connection position among the snapshots the presentation
    /// layer has taken. A remote client stamps it on every command as what the
    /// presentation layer had applied when it composed the command, which is
    /// the one thing the backend's edit fence can trust: the connection has
    /// often received a newer frame that the presentation layer has not.
    taken: Arc<AtomicU64>,
}

/// Build a snapshot stream.
///
/// A remote client reconstructs snapshots from the render updates on its
/// connection and pushes them here, so the renderer draws from the same stream
/// shape in either mode. The channel is "local" in the sense that matters — it
/// never crosses a process — regardless of where the data came from.
#[must_use]
pub(crate) fn snapshot_channel() -> (SnapshotTx, SnapshotRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        tx,
        SnapshotRx {
            rx,
            taken: Arc::new(AtomicU64::new(0)),
        },
    )
}

impl SnapshotRx {
    /// Await the next snapshot, or `None` once the session has shut down.
    pub async fn recv(&mut self) -> Option<(DocumentId, Arc<DocSnapshot>)> {
        let item = self.rx.recv().await;
        item.map(|item| self.take(item))
    }

    /// Take the next ready snapshot without awaiting, or `None` if none is queued.
    pub fn try_recv(&mut self) -> Option<(DocumentId, Arc<DocSnapshot>)> {
        let item = self.rx.try_recv().ok();
        item.map(|item| self.take(item))
    }

    /// The shared record of the newest connection position taken, for the
    /// sender that stamps it on commands.
    pub(crate) fn taken(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.taken)
    }

    fn take(
        &self,
        (doc, snapshot, seq): (DocumentId, Arc<DocSnapshot>, u64),
    ) -> (DocumentId, Arc<DocSnapshot>) {
        self.taken.fetch_max(seq, Ordering::Relaxed);
        (doc, snapshot)
    }
}
