use super::*;

impl Session {
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

    pub(super) fn open(&mut self, id: RequestId, path: PathBuf, language: Option<&str>) {
        if let Some(&existing) = self.store.by_path.get(&path) {
            if let Some(doc) = self.store.docs.get_mut(&existing) {
                doc.refs += 1;
                let version = doc.buffer.version();
                let settings = doc.settings;
                self.emit(
                    Some(id),
                    Event::Opened {
                        doc: existing,
                        version,
                    },
                );
                self.emit(
                    None,
                    Event::DocumentSettingsChanged {
                        doc: existing,
                        settings,
                    },
                );
                self.publish(existing, None);
            }
            return;
        }
        let (mut buffer, format) = match load_document(&path) {
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
        let (document_settings, editorconfig_error) =
            resolve_document_settings(&path, language, &self.config.settings);
        apply_serialization_settings(&mut buffer, document_settings);
        if let Some(message) = editorconfig_error {
            self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Io,
                    message,
                },
            );
        }
        let doc_id = DocumentId(self.store.next);
        self.store.next += 1;
        let mut doc = Document {
            path: path.clone(),
            language,
            lang_id,
            buffer,
            format,
            settings: document_settings,
            highlights: Arc::new(Highlights::default()),
            folds: Arc::new(FoldRegions::default()),
            semantic_blocks: Arc::new(SemanticBlocks::default()),
            error_lines: Arc::default(),
            decorations: Vec::new(),
            refs: 1,
            dirty_since: None,
            backed_up_version: None,
        };
        update_syntax(
            &self.config.settings,
            &self.highlight_tx,
            doc_id,
            &mut doc,
            None,
        );
        let version = doc.buffer.version();
        // Lazily start (or address) this language's server and announce the open.
        self.lsp
            .document_opened(doc.language, &doc.path, version, || doc.buffer.text());
        self.store.by_path.insert(path, doc_id);
        self.store.docs.insert(doc_id, doc);
        self.emit(
            Some(id),
            Event::Opened {
                doc: doc_id,
                version,
            },
        );
        self.emit(
            None,
            Event::DocumentSettingsChanged {
                doc: doc_id,
                settings: document_settings,
            },
        );
        self.publish(doc_id, None);
    }

    pub(super) fn apply(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        change: &Change,
        cause: EditCause,
    ) {
        let tick = self.elapsed_ms();
        let ctx = edit_context(tick, cause, change);
        // `None` means the change was stale or overlapping (the client's local
        // speculative state has diverged from ours); either way we still publish
        // below so the authoritative buffer flows back down to the client instead
        // of leaving it stuck rejecting every future edit forever.
        let version = {
            let highlight_tx = &self.highlight_tx;
            let settings = &self.config.settings;
            let lsp = &mut self.lsp;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                self.events.send((Some(id), unknown_document(doc_id))).ok();
                return;
            };
            match doc.buffer.apply(change, ctx) {
                Ok(applied) => {
                    update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
                    // Arm the backup clock on the clean→dirty transition (see
                    // `backup_tick`).
                    doc.sync_dirty_since(tick);
                    // The single LSP apply site: forward the new full text
                    // (debounced by the server task). A no-op while no server is
                    // attached for this language.
                    lsp.document_changed(doc.language, &doc.path, applied.version, || {
                        doc.buffer.text()
                    });
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

    pub(super) fn undo_redo(&mut self, id: RequestId, doc_id: DocumentId, undo: bool) {
        let tick = self.elapsed_ms();
        let (version, cursor) = {
            let highlight_tx = &self.highlight_tx;
            let settings = &self.config.settings;
            let lsp = &mut self.lsp;
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
            update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
            // Undoing back to the save point clears dirtiness (and any pending backup).
            doc.sync_dirty_since(tick);
            // The buffer changed like any other edit: keep the server in sync.
            lsp.document_changed(doc.language, &doc.path, applied.version, || {
                doc.buffer.text()
            });
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

    pub(super) fn save(&mut self, id: RequestId, doc_id: DocumentId) {
        if self.apply_save_cleanup(doc_id) {
            self.publish(doc_id, None);
        }
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
                self.publish(doc_id, None);
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

    fn apply_save_cleanup(&mut self, doc_id: DocumentId) -> bool {
        let tick = self.elapsed_ms();
        let highlight_tx = &self.highlight_tx;
        let settings = &self.config.settings;
        let lsp = &mut self.lsp;
        let Some(doc) = self.store.docs.get_mut(&doc_id) else {
            return false;
        };
        let current = doc.buffer.text();
        let normalized = normalize_text_for_save(&current, doc.settings);
        if normalized == current {
            return false;
        }
        let Some(change) = whole_document_change(doc, normalized) else {
            return false;
        };
        let ctx = edit_context(tick, EditCause::Replace, &change);
        let Ok(applied) = doc.buffer.apply(&change, ctx) else {
            return false;
        };
        update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
        doc.sync_dirty_since(tick);
        lsp.document_changed(doc.language, &doc.path, applied.version, || {
            doc.buffer.text()
        });
        true
    }

    pub(super) fn retarget(&mut self, id: RequestId, doc_id: DocumentId, path: PathBuf) {
        let Some(doc) = self.store.docs.get_mut(&doc_id) else {
            self.emit(Some(id), unknown_document(doc_id));
            return;
        };
        let old = doc.path.clone();
        let old_language = doc.language;
        self.store.by_path.remove(&old);
        doc.path = path.clone();
        doc.lang_id = language_id_from_path(&path);
        doc.language = language_name_from_path(&path);
        // The language may have changed with the extension; re-highlight from scratch.
        update_syntax(&self.config.settings, &self.highlight_tx, doc_id, doc, None);
        // The old URI is gone; the (possibly different) new language's server
        // adopts the new one.
        self.lsp.document_closed(old_language, &old);
        self.lsp
            .document_opened(doc.language, &doc.path, doc.buffer.version(), || {
                doc.buffer.text()
            });
        self.store.by_path.insert(path.clone(), doc_id);
        self.emit(Some(id), Event::Retargeted { doc: doc_id, path });
        self.refresh_document_settings(&[doc_id]);
        self.publish(doc_id, None);
    }

    pub(super) fn close(&mut self, id: RequestId, doc_id: DocumentId) {
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
                self.lsp.document_closed(doc.language, &doc.path);
                // Release the worker's retained trees for this document.
                self.highlight_tx.send(HighlightJob::Drop(doc_id)).ok();
                // The document is gone from the editor: skipping a save is an explicit
                // decision, so clean up its swap.
                if let Some(store) = self.swaps.as_ref() {
                    store.remove(&doc.path);
                }
            }
            self.emit(Some(id), Event::Closed { doc: doc_id });
        }
    }
}
