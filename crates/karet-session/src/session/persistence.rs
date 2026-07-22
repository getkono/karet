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
    pub(super) fn discard_swaps(&mut self) {
        for record in std::mem::take(&mut self.pending_swaps) {
            discard(&record.swap_path);
        }
    }

    // --- visualizations ---------------------------------------------------

    /// Build the workspace package-dependency graph and emit it, or surface a failure
    /// (no lockfile / parse error) as a notification.
    pub(super) fn emit_dependency_graph(&mut self, id: RequestId) {
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
        let version = {
            let highlight_tx = &self.highlight_tx;
            let settings = &self.config.settings;
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
            update_syntax(settings, highlight_tx, doc_id, doc, None);
            // The on-disk content is the new truth; keep the server in sync.
            lsp.document_changed(doc.language, &doc.path, doc.buffer.version(), || {
                doc.buffer.text()
            });
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

    pub(super) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

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
                highlights: doc.highlights.clone(),
                folds: doc.folds.clone(),
                semantic_blocks: doc.semantic_blocks.clone(),
                decorations: Arc::new(doc.decorations.clone()),
                syntax_error_lines: doc.error_lines.clone(),
                language: doc.language,
                dirty: doc.buffer.is_dirty(),
                cursor,
            });
            self.snapshots.send((doc_id, snapshot)).ok();
        }
    }
}
