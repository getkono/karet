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

use karet_core::CursorState;
use karet_syntax::FoldRegions;
use karet_syntax::Highlights;
use karet_syntax::SemanticBlocks;
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
