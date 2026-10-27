//! What the session keeps on disk across restarts: crash-recovery swaps (and
//! the timer that writes them) and persisted undo history.
//!
//! This module decides when to write, recover, or discard; the on-disk formats
//! and the store itself belong to [`crate::backup`].

use super::*;

impl Session {
    // --- crash-recovery swaps ---------------------------------------------

    /// Announce the swaps left by previous sessions so the UI can prompt the user to
    /// recover or discard them. A no-op when there are none.
    pub(super) fn announce_pending_swaps(&mut self) {
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
    pub(super) fn recover_swaps(&mut self, id: RequestId) {
        for record in std::mem::take(&mut self.pending_swaps) {
            self.open(Some(id), record.meta.original.clone(), None);
            let Some(&doc_id) = self.store.by_path.get(&record.meta.original) else {
                continue;
            };
            let change = self
                .store
                .docs
                .get(&doc_id)
                .map(|doc| whole_document_change(doc, record.content.clone()));
            if let Some(change) = change {
                self.apply(id, doc_id, &change, EditCause::Replace);
                discard(&record.swap_path);
            }
        }
    }

    /// Discard every pending swap without recovering (the user declined).
    pub(super) fn discard_swaps(&mut self) {
        for record in std::mem::take(&mut self.pending_swaps) {
            discard(&record.swap_path);
        }
    }

    // --- persisted undo ---------------------------------------------------

    /// Give a freshly loaded `buffer` for `path` the configured undo cap, and —
    /// with `files.persistentUndo` on — the undo history a previous session
    /// left for it. A history whose content fingerprint no longer matches the
    /// file (it was edited elsewhere since) is deleted rather than restored.
    pub(super) fn prepare_undo(&self, path: &Path, buffer: &mut TextBuffer) {
        buffer.set_undo_limit(self.config.settings.files.undo_cap());
        if !self.config.settings.files.persistent_undo {
            return;
        }
        let Some(store) = self.swaps.as_ref() else {
            return;
        };
        if let Some(history) = store.read_undo(path)
            && buffer.restore_history(history).is_err()
        {
            store.remove_undo(path);
        }
    }

    /// Persist `doc_id`'s undo history, if its buffer matches what is on disk.
    /// Called after a successful save and when a clean document closes — the
    /// two moments the history describes the file's text. With
    /// `files.persistentUndo` off, any history kept earlier is deleted instead,
    /// so switching it off does not leave old edits on disk.
    pub(super) fn persist_undo(&self, doc_id: DocumentId) {
        let (Some(store), Some(doc)) = (self.swaps.as_ref(), self.store.docs.get(&doc_id)) else {
            return;
        };
        if !self.config.settings.files.persistent_undo {
            store.remove_undo(&doc.path);
            return;
        }
        if doc.must_create || doc.buffer.is_dirty() {
            return;
        }
        // A failed write only costs undo across the next restart.
        let _ = store.write_undo(&doc.path, &doc.buffer.history_snapshot());
    }

    // --- crash-recovery backups -------------------------------------------

    /// Write a crash-recovery swap for `doc_id` now, whatever the buffer's state.
    ///
    /// The one place a swap is actually written. A failed save calls it directly
    /// — that is a data-loss event, and the backup is the whole of the recovery
    /// — while every routine backup arrives through [`Self::back_up_document`],
    /// which asks the settings and staleness questions this deliberately does not.
    pub(super) fn write_swap(&mut self, doc_id: DocumentId) {
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
    /// One period of the session's own clock: every sweep that has to happen
    /// whether or not the user is typing.
    ///
    /// The format-on-save deadline runs first and unconditionally — it is not
    /// part of crash recovery, and [`Self::backup_tick`] declines to do anything
    /// at all when backups are switched off.
    pub(crate) fn tick(&mut self) {
        self.tick_at(self.elapsed_ms());
    }

    /// [`Self::tick`] against a given clock reading, so a test can reach the
    /// format-on-save deadline without waiting out its wall-clock duration —
    /// the same seam, and for the same reason, as
    /// [`Self::expire_format_on_save`].
    pub(crate) fn tick_at(&mut self, now: u64) {
        self.expire_format_on_save(now);
        self.backup_tick();
    }

    pub(crate) fn backup_tick(&mut self) {
        // Cheap short-circuits for a sweep that usually has nothing to do.
        // [`Self::back_up_document`] asks the same two questions per document,
        // so a swap is never written past them whichever way it is reached.
        if !self.config.settings.files.backup || self.swaps.is_none() {
            return;
        }
        let interval = self.config.settings.files.backup_interval;
        let now = self.elapsed_ms();
        let due: Vec<DocumentId> = self
            .store
            .docs
            .iter()
            .filter(|(_, doc)| {
                doc.dirty_since
                    .is_some_and(|since| now.saturating_sub(since) >= interval)
            })
            .map(|(doc_id, _)| *doc_id)
            .collect();
        for doc_id in due {
            self.back_up_document(doc_id);
        }
    }

    /// Back `doc_id` up now, regardless of how long it has been dirty.
    ///
    /// The interval is the only thing this skips. A swap still needs backups to
    /// be switched on, a store to write to, and a dirty buffer that has changed
    /// since its last swap — backing up an unchanged one would only rewrite the
    /// swap it already has.
    ///
    /// Called by [`Self::backup_tick`] for every document whose interval has
    /// come due, and by a save that parks on a formatter: that save's disk write
    /// is deferred for as long as the formatter takes, and for that whole window
    /// the swap is the only other copy of the buffer there is.
    pub(super) fn back_up_document(&mut self, doc_id: DocumentId) {
        if !self.config.settings.files.backup {
            return;
        }
        let due = self.store.docs.get(&doc_id).is_some_and(|doc| {
            doc.buffer.is_dirty() && doc.backed_up_version != Some(doc.buffer.version())
        });
        if due {
            self.write_swap(doc_id);
        }
    }

    // --- helpers ----------------------------------------------------------

    pub(super) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}
