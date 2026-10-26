//! The session's two outbound streams: [`Event`]s and render-only document
//! snapshots. Every module answers through these.

use super::*;

impl Session {
    pub(super) fn emit(&self, id: Option<RequestId>, event: Event) {
        self.events.send((id, event)).ok();
    }

    /// Push a render-only snapshot of `doc_id` on the snapshot stream. `cursor` is
    /// `Some` only for undo/redo, carrying the caret the editor should jump to; every
    /// other publish passes `None` and leaves the UI's cursor untouched.
    pub(super) fn publish(&self, doc_id: DocumentId, cursor: Option<CursorState>) {
        if let Some(doc) = self.store.docs.get(&doc_id) {
            let snapshot = Arc::new(DocSnapshot {
                version: doc.buffer.version(),
                buffer: doc.buffer.content_snapshot(),
                highlights: doc.display_highlights(),
                folds: doc.folds.clone(),
                semantic_blocks: doc.semantic_blocks.clone(),
                syntax_error_lines: doc.error_lines.clone(),
                table_lines: doc.table_lines.clone(),
                language: doc.language,
                dirty: doc.buffer.is_dirty(),
                cursor,
            });
            self.snapshots.send((doc_id, snapshot)).ok();
        }
    }
}
