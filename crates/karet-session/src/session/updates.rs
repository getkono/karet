use super::*;
use karet_core::Symbol;

impl Session {
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

    /// Take the highlight worker's result stream, to be driven by the actor.
    pub(crate) fn take_highlights(&mut self) -> Option<mpsc::UnboundedReceiver<HighlightResult>> {
        self.highlight_rx.take()
    }

    /// Take the LSP tasks' result stream, to be driven by the actor.
    pub(crate) fn take_lsp_updates(&mut self) -> Option<mpsc::UnboundedReceiver<LspUpdate>> {
        self.lsp_rx.take()
    }

    /// Replace how language servers are connected (tests inject an in-memory
    /// server instead of spawning a process).
    #[cfg(test)]
    pub(crate) fn set_lsp_connector(&mut self, connector: crate::lsp::Connector) {
        self.lsp.set_connector(connector);
    }

    /// Adopt one LSP task result: convert positions against the live buffer
    /// (LSP's UTF-16 → the buffer's UTF-32 columns) and emit the answering event.
    /// A result for a document that has since closed is dropped as stale.
    pub(crate) fn apply_lsp_update(&mut self, update: LspUpdate) {
        if !self.lsp.accepts(&update) {
            return;
        }
        match update {
            LspUpdate::Completions {
                request,
                doc,
                version,
                mut items,
                ..
            } => {
                let Some(d) = self.store.docs.get(&doc) else {
                    return; // closed since the request: stale by definition
                };
                for item in &mut items {
                    if let Some(edit) = item.edit.as_mut() {
                        let start = edit.range.start;
                        let end = edit.range.end;
                        edit.range = Range {
                            start: d.buffer.utf16_to_line_col(start.line, start.col),
                            end: d.buffer.utf16_to_line_col(end.line, end.col),
                        };
                    }
                }
                self.emit(
                    Some(request),
                    Event::Completions {
                        doc,
                        version,
                        items,
                    },
                );
            },
            LspUpdate::Symbols {
                request,
                doc,
                version,
                mut symbols,
                ..
            } => {
                let Some(document) = self.store.docs.get(&doc) else {
                    return;
                };
                if document.buffer.version() != version {
                    return;
                }
                convert_symbol_columns(&document.buffer, &mut symbols);
                self.emit(Some(request), Event::Symbols { doc, symbols });
            },
            LspUpdate::SpawnFailed {
                language, command, ..
            } => self.emit(
                None,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Lsp,
                    message: format!(
                        "no language server for {language}: '{command}' could not be started \
                         (language features disabled for {language})"
                    ),
                },
            ),
            LspUpdate::ServerDied { language, .. } => self.emit(
                None,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Lsp,
                    message: format!(
                        "the {language} language server stopped; restart karet to relaunch it"
                    ),
                },
            ),
        }
    }

    /// Serve [`Command::Completion`]: convert the caret to the server's UTF-16
    /// encoding and forward to the document's language server. Languages with no
    /// server answer immediately with an empty set, so the client never waits.
    pub(super) fn completion(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            self.emit(Some(id), unknown_document(doc_id));
            return;
        };
        let version = doc.buffer.version();
        let utf16 = LineCol::new(position.line, doc.buffer.line_col_to_utf16(position));
        let forwarded = self
            .lsp
            .completion(doc.language, id, doc_id, version, &doc.path, utf16);
        if !forwarded {
            self.emit(
                Some(id),
                Event::Completions {
                    doc: doc_id,
                    version,
                    items: Vec::new(),
                },
            );
        }
    }

    /// Serve [`Command::DocumentSymbols`] from the document's language server.
    pub(super) fn document_symbols(&mut self, id: RequestId, doc_id: DocumentId) {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            self.emit(Some(id), unknown_document(doc_id));
            return;
        };
        let version = doc.buffer.version();
        let forwarded = self
            .lsp
            .document_symbols(doc.language, id, doc_id, version, &doc.path);
        if !forwarded {
            self.emit(
                Some(id),
                Event::Symbols {
                    doc: doc_id,
                    symbols: Vec::new(),
                },
            );
        }
    }

    /// Adopt a completed highlight, then publish the refreshed snapshot.
    ///
    /// A result for a version the buffer has already moved past is dropped: a newer
    /// request is by construction already queued (every edit sends one), so waiting for
    /// it beats painting spans that no longer describe the text.
    pub(crate) fn apply_highlights(&mut self, result: HighlightResult) {
        let Some(doc) = self.store.docs.get_mut(&result.doc) else {
            return; // the document closed while the worker was busy
        };
        if doc.buffer.version() != result.version {
            return;
        }
        doc.highlights = result.highlights;
        doc.folds = result.folds;
        doc.semantic_blocks = result.semantic_blocks;
        doc.error_lines = result.error_lines;
        self.publish(result.doc, None);
    }

    /// React to a debounced filesystem event by reloading or flagging any open
    /// document whose file changed underneath it.
    pub(crate) fn handle_fs_event(&mut self, event: FsEvent) {
        if event.kind == karet_watch::FsEventKind::WatchDegraded {
            self.emit(
                None,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Io,
                    message: "filesystem watch limit reached; some paths are polled".to_string(),
                },
            );
            return;
        }
        let config_paths: Vec<PathBuf> = self
            .config_manager
            .as_ref()
            .map(|manager| {
                event
                    .paths
                    .iter()
                    .filter(|path| manager.is_config_path(path))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let changed_config = self
            .config_manager
            .as_mut()
            .and_then(|manager| manager.reload(&config_paths));
        if let Some(report) = changed_config {
            self.apply_config_report(report);
        }

        if event
            .paths
            .iter()
            .any(|path| path.file_name().is_some_and(|name| name == ".editorconfig"))
        {
            let docs: Vec<DocumentId> = self.store.docs.keys().copied().collect();
            self.refresh_document_settings(&docs);
        }

        let workspace_paths: Vec<PathBuf> = event
            .paths
            .into_iter()
            .filter(|path| !config_paths.contains(path))
            .collect();
        for path in &workspace_paths {
            if let Some(&doc_id) = self.store.by_path.get(path) {
                self.on_external_change(doc_id, path);
            }
        }
        if workspace_paths.is_empty() {
            return;
        }
        // A generic "something changed" signal for anything else the client
        // derives from the workspace (e.g. a live-updating search) — distinct
        // from the specific reactions below, which only cover open documents and
        // VCS state.
        self.emit(
            None,
            Event::FsChanged {
                paths: workspace_paths,
            },
        );
        // Any worktree edit or watched git-metadata change can alter status. The
        // event is already debounced and the emit is change-gated, so a burst (and
        // the session's own index writes) collapse to at most one update.
        self.emit_vcs_status(None);
        // A watched `refs/**` / `HEAD` change may mean new commits; reconcile the log
        // incrementally. The head read is cheap and this early-returns when unchanged.
        self.reconcile_vcs_log();
    }

    /// Adopt one validated live configuration snapshot and refresh producers whose
    /// behavior is derived from it. Existing LSP tasks are retired on an LSP change;
    /// their generation-tagged late answers are ignored by [`Self::apply_lsp_update`].
    pub(super) fn apply_config_report(&mut self, report: crate::config::LoadedConfig) {
        let lsp_changed = self.lsp.reconfigure(report.settings.lsp.clone());
        self.config.settings = report.settings.clone();
        self.config.loaded_config = report.clone();
        let docs: Vec<DocumentId> = self.store.docs.keys().copied().collect();
        self.refresh_document_settings(&docs);

        // Semantic-comment settings can vary by language. Requeue every open
        // document from scratch so both global and selector changes take effect.
        let settings = &self.config.settings;
        let highlight_tx = &self.highlight_tx;
        for (&doc_id, doc) in &mut self.store.docs {
            update_syntax(settings, highlight_tx, doc_id, doc, None);
        }

        if lsp_changed {
            let lsp = &mut self.lsp;
            for doc in self.store.docs.values() {
                lsp.document_opened(doc.language, &doc.path, doc.buffer.version(), || {
                    doc.buffer.text()
                });
            }
        }

        self.emit(
            None,
            Event::ConfigChanged {
                report: Box::new(report),
            },
        );
    }

    /// Re-resolve per-path behavior after an application or EditorConfig change.
    pub(super) fn refresh_document_settings(&mut self, docs: &[DocumentId]) {
        let settings = self.config.settings.clone();
        let inputs: Vec<(DocumentId, PathBuf, Option<&'static str>)> = docs
            .iter()
            .filter_map(|doc_id| {
                self.store
                    .docs
                    .get(doc_id)
                    .map(|doc| (*doc_id, doc.path.clone(), doc.language))
            })
            .collect();
        for (doc_id, path, language) in inputs {
            let (resolved, error) = resolve_document_settings(&path, language, &settings);
            let changed = self.store.docs.get_mut(&doc_id).is_some_and(|doc| {
                if doc.settings == resolved {
                    return false;
                }
                doc.settings = resolved;
                apply_serialization_settings(&mut doc.buffer, resolved);
                true
            });
            if let Some(message) = error {
                self.emit(
                    None,
                    Event::Notification {
                        severity: Severity::Warning,
                        kind: NotificationKind::Io,
                        message,
                    },
                );
            }
            if changed {
                self.emit(
                    None,
                    Event::DocumentSettingsChanged {
                        doc: doc_id,
                        settings: resolved,
                    },
                );
                self.publish(doc_id, None);
            }
        }
    }
}

fn convert_symbol_columns(buffer: &TextBuffer, symbols: &mut [Symbol]) {
    for symbol in symbols {
        let range = symbol.range;
        symbol.range = Range {
            start: buffer.utf16_to_line_col(range.start.line, range.start.col),
            end: buffer.utf16_to_line_col(range.end.line, range.end.col),
        };
        let selection = symbol.selection_range;
        symbol.selection_range = Range {
            start: buffer.utf16_to_line_col(selection.start.line, selection.start.col),
            end: buffer.utf16_to_line_col(selection.end.line, selection.end.col),
        };
        convert_symbol_columns(buffer, &mut symbol.children);
    }
}
