use super::*;

/// Test-only read view of a document's buffer state. Production consumers render
/// from the [`DocSnapshot`](crate::local::DocSnapshot) stream instead.
#[cfg(test)]
pub(crate) struct DocumentView<'a> {
    pub(crate) buffer: &'a TextBuffer,
    version: u64,
}

#[cfg(test)]
impl DocumentView<'_> {
    pub(crate) fn buffer(&self) -> &TextBuffer {
        self.buffer
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
impl Session {
    /// Borrow a read-only view of a document (tests only).
    pub(crate) fn document(&self, doc: DocumentId) -> Option<DocumentView<'_>> {
        let d = self.store.docs.get(&doc)?;
        Some(DocumentView {
            buffer: &d.buffer,
            version: d.buffer.version(),
        })
    }
}

impl Session {
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
        let (mut buffer, format, must_create) = match load_document(&path) {
            Ok((buffer, format)) => (buffer, format, false),
            Err(DocumentLoadError::Missing) => (TextBuffer::new(), DocFormat::Text, true),
            Err(
                DocumentLoadError::Load(LoadError::NotUtf8 { .. }) | DocumentLoadError::Undecodable,
            ) => {
                // Full non-UTF-8 editing isn't supported (and a corrupt CBOR has no
                // text form); tell the client so it can fall back to a read-only
                // view instead of leaving this path's tab registered with no
                // document forever.
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
            .or_else(|| language_name_for_path(&path));
        let language_selector = language_selector_for_path(&path);
        let lsp_language_id = lsp_language_id_for_path(&path);
        let (document_settings, editorconfig_error) =
            resolve_document_settings(&path, language_selector, &self.config.settings);
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
            language_selector,
            lsp_language_id,
            lang_id,
            buffer,
            format,
            must_create,
            settings: document_settings,
            highlights: Arc::new(Highlights::default()),
            folds: Arc::new(FoldRegions::default()),
            semantic_blocks: Arc::new(SemanticBlocks::default()),
            syntax_symbols: Arc::default(),
            error_lines: Arc::default(),
            spell_diagnostics: Vec::new(),
            lint_diagnostics: Vec::new(),
            lsp_diagnostics: HashMap::new(),
            decorations: Vec::new(),
            refs: 1,
            dirty_since: None,
            backed_up_version: None,
        };
        let spell_without_syntax = update_syntax(
            &self.config.settings,
            &self.highlight_tx,
            doc_id,
            &mut doc,
            None,
        );
        let version = doc.buffer.version();
        // Lazily start (or address) this language's server and announce the open.
        self.lsp.document_opened(
            doc.language_selector,
            doc.lsp_language_id,
            &doc.path,
            version,
            || doc.buffer.text(),
        );
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
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
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
        let (version, spell_without_syntax) = {
            let highlight_tx = &self.highlight_tx;
            let settings = &self.config.settings;
            let lsp = &mut self.lsp;
            let Some(doc) = self.store.docs.get_mut(&doc_id) else {
                self.events.send((Some(id), unknown_document(doc_id))).ok();
                return;
            };
            let version = match doc.buffer.apply(change, ctx) {
                Ok(applied) => {
                    let _ =
                        update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
                    // Arm the backup clock on the clean→dirty transition (see
                    // `backup_tick`).
                    doc.sync_dirty_since(tick);
                    // The single LSP apply site: forward the new full text
                    // (debounced by the server task). A no-op while no server is
                    // attached for this language.
                    lsp.document_changed(doc.language_selector, &doc.path, applied.version, || {
                        doc.buffer.text()
                    });
                    Some(applied.version)
                },
                Err(_) => None,
            };
            (version, doc.lang_id.is_none())
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
        if version.is_some() && spell_without_syntax {
            self.schedule_spell(doc_id);
        }
    }

    pub(super) fn undo_redo(&mut self, id: RequestId, doc_id: DocumentId, undo: bool) {
        let tick = self.elapsed_ms();
        let (version, cursor, spell_without_syntax) = {
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
            let _ = update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
            // Undoing back to the save point clears dirtiness (and any pending backup).
            doc.sync_dirty_since(tick);
            // The buffer changed like any other edit: keep the server in sync.
            lsp.document_changed(doc.language_selector, &doc.path, applied.version, || {
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
            (applied.version, cursor, doc.lang_id.is_none())
        };
        self.emit(
            Some(id),
            Event::Applied {
                doc: doc_id,
                version,
            },
        );
        self.publish(doc_id, cursor);
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
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
                    self.lsp
                        .document_saved(doc.language_selector, &doc.path, || doc.buffer.text());
                }
                self.publish(doc_id, None);
                self.emit(Some(id), Event::Saved { doc: doc_id });
                self.wakatime_beat(doc_id, true);
                if self.config.settings.latex.build_on_save
                    && self.store.docs.get(&doc_id).is_some_and(|doc| {
                        doc.path
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
                    })
                    && let Some(source) = self.store.docs.get(&doc_id).map(|doc| doc.path.clone())
                    && self.enqueue_latex_build(None, id, doc_id, source).is_err()
                {
                    self.emit(
                        None,
                        Event::Notification {
                            severity: Severity::Warning,
                            kind: NotificationKind::System,
                            message: "LaTeX build worker is unavailable".to_owned(),
                        },
                    );
                }
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

    /// Persist one dirty TeX source before handing it to an external compiler.
    pub(super) fn save_for_external_build(
        &mut self,
        doc_id: DocumentId,
    ) -> Result<PathBuf, String> {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return Err("LaTeX build: unknown document".to_owned());
        };
        if !doc
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
        {
            return Err("LaTeX build requires an editable .tex document".to_owned());
        }
        let path = doc.path.clone();
        if !doc.buffer.is_dirty() {
            return Ok(path);
        }
        if self.apply_save_cleanup(doc_id) {
            self.publish(doc_id, None);
        }
        let result = self.store.docs.get_mut(&doc_id).map(save_document);
        match result {
            Some(Ok(())) => {
                if let Some(doc) = self.store.docs.get_mut(&doc_id) {
                    doc.dirty_since = None;
                    doc.backed_up_version = None;
                    if let Some(store) = self.swaps.as_ref() {
                        store.remove(&doc.path);
                    }
                }
                self.publish(doc_id, None);
                Ok(path)
            },
            Some(Err(TextError::Conflict)) => Err(
                "LaTeX build cancelled: the source changed on disk; resolve the save conflict first"
                    .to_owned(),
            ),
            Some(Err(error)) => Err(format!("LaTeX build cancelled: save failed: {error}")),
            None => Err("LaTeX build: unknown document".to_owned()),
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
        let spell_without_syntax =
            update_syntax(settings, highlight_tx, doc_id, doc, Some(&applied.edits));
        doc.sync_dirty_since(tick);
        lsp.document_changed(doc.language_selector, &doc.path, applied.version, || {
            doc.buffer.text()
        });
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
        true
    }

    pub(super) fn retarget(&mut self, id: RequestId, doc_id: DocumentId, path: PathBuf) {
        let Some(doc) = self.store.docs.get_mut(&doc_id) else {
            self.emit(Some(id), unknown_document(doc_id));
            return;
        };
        let old = doc.path.clone();
        let old_language = doc.language_selector;
        self.store.by_path.remove(&old);
        doc.path = path.clone();
        doc.lang_id = language_id_from_path(&path);
        doc.language = language_name_for_path(&path);
        doc.language_selector = language_selector_for_path(&path);
        doc.lsp_language_id = lsp_language_id_for_path(&path);
        // The language may have changed with the extension; re-highlight from scratch.
        let spell_without_syntax =
            update_syntax(&self.config.settings, &self.highlight_tx, doc_id, doc, None);
        // The old URI is gone; the (possibly different) new language's server
        // adopts the new one.
        self.lsp.document_closed(old_language, &old);
        self.lsp.document_opened(
            doc.language_selector,
            doc.lsp_language_id,
            &doc.path,
            doc.buffer.version(),
            || doc.buffer.text(),
        );
        self.store.by_path.insert(path.clone(), doc_id);
        self.emit(Some(id), Event::Retargeted { doc: doc_id, path });
        self.refresh_document_settings(&[doc_id]);
        self.publish(doc_id, None);
        if spell_without_syntax {
            self.schedule_spell(doc_id);
        }
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
                self.lsp.document_closed(doc.language_selector, &doc.path);
                // Release the worker's retained trees for this document.
                self.highlight_tx.send(HighlightJob::Drop(doc_id)).ok();
                self.spell_errors.remove(&doc_id);
                // The document is gone from the editor: skipping a save is an explicit
                // decision, so clean up its swap.
                if let Some(store) = self.swaps.as_ref() {
                    store.remove(&doc.path);
                }
            }
            self.emit(Some(id), Event::Closed { doc: doc_id });
        }
    }

    /// Convert a document (DOCX, or a Jupyter notebook) to markdown for a
    /// read-only preview. Parsing runs off the actor thread; the answer
    /// arrives as [`Event::DocumentConverted`].
    #[cfg(any(feature = "docx", feature = "notebook"))]
    pub(super) fn convert_document(&mut self, id: RequestId, path: PathBuf) {
        let events = self.events.clone();
        std::thread::spawn(move || {
            let markdown = convert_document_file(&path);
            let _ = events.send((Some(id), Event::DocumentConverted { path, markdown }));
        });
    }

    /// Without either conversion feature, report the build gap.
    #[cfg(not(any(feature = "docx", feature = "notebook")))]
    pub(super) fn convert_document(&mut self, id: RequestId, path: PathBuf) {
        self.emit(
            Some(id),
            Event::DocumentConverted {
                path,
                markdown: Err(
                    "this backend was built without document conversion (`docx`/`notebook` \
                     features)"
                        .to_string(),
                ),
            },
        );
    }
}

/// One document → markdown, routed by extension (each format behind its
/// feature so lean builds stay lean).
#[cfg(any(feature = "docx", feature = "notebook"))]
fn convert_document_file(path: &std::path::Path) -> Result<String, String> {
    let is_notebook = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"));
    if is_notebook {
        #[cfg(feature = "notebook")]
        {
            return std::fs::read_to_string(path)
                .map_err(|error| format!("could not read {}: {error}", path.display()))
                .and_then(|text| {
                    karet_notebook::parse(&text)
                        .map(|notebook| karet_notebook::to_markdown(&notebook))
                        .map_err(|error| format!("could not convert {}: {error}", path.display()))
                });
        }
        #[cfg(not(feature = "notebook"))]
        {
            return Err(
                "this backend was built without notebook support (`notebook` feature)".to_owned(),
            );
        }
    }
    #[cfg(feature = "docx")]
    {
        std::fs::read(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))
            .and_then(|bytes| {
                karet_docx::parse(&bytes)
                    .map(|document| karet_docx::to_markdown(&document))
                    .map_err(|error| format!("could not convert {}: {error}", path.display()))
            })
    }
    #[cfg(not(feature = "docx"))]
    {
        Err("this backend was built without DOCX support (`docx` feature)".to_owned())
    }
}

impl Session {
    /// Send one WakaTime heartbeat for `doc_id`, spawning the worker on first
    /// use. Inert unless `wakatime.enabled` is set.
    pub(super) fn wakatime_beat(&mut self, doc_id: DocumentId, is_write: bool) {
        if !self.config.settings.wakatime.enabled {
            return;
        }
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return;
        };
        let branch = self
            .vcs
            .as_ref()
            .and_then(|repo| repo.current_branch().ok().flatten());
        let project = self
            .config
            .roots
            .first()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let beat = crate::wakatime::Beat {
            path: doc.path.clone(),
            language: doc.language,
            lines: doc.buffer.text().lines().count(),
            is_write,
            branch,
            project,
        };
        let worker = self
            .wakatime_worker
            .get_or_insert_with(|| crate::wakatime::spawn(self.events.clone()));
        let _ = worker.send(beat);
    }
}
