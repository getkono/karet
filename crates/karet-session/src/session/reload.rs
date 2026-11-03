//! Reconciling an open document with a change to its file on disk: ignore our
//! own write, reload a clean buffer, or flag a conflict on a dirty one.

use super::*;

impl Session {
    /// Decide what an external change to `doc_id`'s file means: ignore our own
    /// write, reload a clean buffer, or flag a conflict on a dirty one.
    pub(super) fn on_external_change(&mut self, doc_id: DocumentId, path: &Path) {
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
    pub(super) fn reload(&mut self, doc_id: DocumentId) {
        let (version, spell_without_syntax, retired) = {
            let highlight_tx = &self.highlight_tx;
            let settings = self.config.settings();
            let lsp = &mut self.lsp;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                return;
            };
            let Ok((fresh, _)) = load_document(&doc.path) else {
                return; // file vanished or became unreadable; leave the buffer as-is
            };
            doc.buffer.adopt_content(fresh);
            apply_serialization_settings(&mut doc.buffer, doc.settings);
            // The buffer was replaced wholesale, so the worker's retained tree is void:
            // `None` edits force it to start over.
            let spell_without_syntax = update_syntax(settings, highlight_tx, doc_id, doc, None);
            // The on-disk content is the new truth; keep the server in sync. No
            // edit produced it, so it travels as the full text.
            let retired = lsp.document_changed(doc.language_selector, &doc.path, &doc.buffer, None);
            (doc.buffer.version(), spell_without_syntax, retired)
        };
        self.adopt_retirement(retired);
        self.emit(
            None,
            Event::Reloaded {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id, None);
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
    }
}
