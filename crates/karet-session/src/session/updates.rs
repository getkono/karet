use karet_core::Symbol;

use super::*;

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

    /// Take the spell worker's result stream, to be driven by the actor.
    pub(crate) fn take_spell_results(&mut self) -> Option<mpsc::UnboundedReceiver<SpellResult>> {
        self.spell_rx.take()
    }

    /// Take the LSP tasks' result stream, to be driven by the actor.
    pub(crate) fn take_lsp_updates(&mut self) -> Option<mpsc::UnboundedReceiver<LspUpdate>> {
        self.lsp_rx.take()
    }

    /// Take shared-registry results, to be driven by the actor.
    pub(crate) fn take_lsp_registry_updates(
        &mut self,
    ) -> Option<mpsc::UnboundedReceiver<crate::lsp_registry::RegistryUpdate>> {
        self.lsp_registry_rx.take()
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
                if symbols.is_empty() {
                    symbols = document.syntax_symbols.as_ref().clone();
                }
                self.emit(Some(request), Event::Symbols { doc, symbols });
            },
            LspUpdate::Hover {
                request,
                doc,
                version,
                mut hover,
                ..
            } => {
                let Some(document) = self.store.docs.get(&doc) else {
                    return;
                };
                if document.buffer.version() != version {
                    return;
                }
                if let Some(range) = hover.as_mut().and_then(|hover| hover.range.as_mut()) {
                    *range = utf16_range_to_buffer(&document.buffer, *range);
                }
                self.emit(Some(request), Event::HoverResult { hover });
            },
            LspUpdate::Definitions {
                request,
                doc,
                version,
                mut locations,
                ..
            } => {
                let Some(document) = self.store.docs.get(&doc) else {
                    return;
                };
                if document.buffer.version() != version {
                    return;
                }
                // A definition usually lands in a *different* file from the one the
                // request came from, and the server answers in UTF-16 columns for
                // every one of them — so each location is converted against its own
                // file, not just the requesting document's buffer.
                for location in &mut locations {
                    if let Some(buffer) = self.buffer_for_path(&location.path) {
                        location.range = utf16_range_to_buffer(&buffer, location.range);
                    }
                }
                self.emit(Some(request), Event::Definitions { locations });
            },
            LspUpdate::WorkspaceSymbols {
                request, symbols, ..
            } => {
                self.emit(Some(request), Event::WorkspaceSymbols { symbols });
            },
            LspUpdate::WorkspaceEdit { request, edit, .. } => {
                let mut edit = edit;
                for (path, edits) in &mut edit.changes {
                    if let Some(buffer) = self.buffer_for_path(path) {
                        for edit in edits {
                            edit.range = utf16_range_to_buffer(&buffer, edit.range);
                        }
                    }
                }
                self.emit(Some(request), Event::WorkspaceEdit { edit });
            },
            LspUpdate::Formatting {
                request,
                doc,
                version,
                mut edits,
                ..
            } => {
                let Some(document) = self.store.docs.get(&doc) else {
                    return;
                };
                if document.buffer.version() != version {
                    return;
                }
                for edit in &mut edits {
                    edit.range = utf16_range_to_buffer(&document.buffer, edit.range);
                }
                self.emit(
                    Some(request),
                    Event::FormattingEdits {
                        doc,
                        version,
                        edits,
                    },
                );
            },
            LspUpdate::ServerStatus {
                server, message, ..
            } => {
                // The transient status line is the right surface: it clears on
                // the next keystroke and never queues like a notification.
                self.emit(
                    None,
                    Event::Progress {
                        message: format!("{server}: {message}"),
                        percent: None,
                    },
                );
            },
            LspUpdate::Diagnostics {
                server,
                path,
                version,
                mut diagnostics,
                ..
            } => {
                let Some(&doc_id) = self.store.by_path.get(&path) else {
                    return;
                };
                let Some(document) = self.store.docs.get_mut(&doc_id) else {
                    return;
                };
                if version.is_some_and(|published| {
                    published != crate::lsp::version_i32(document.buffer.version())
                }) {
                    return;
                }
                for diagnostic in &mut diagnostics {
                    diagnostic.range = utf16_range_to_buffer(&document.buffer, diagnostic.range);
                    for related in &mut diagnostic.related {
                        if related.location.path == path {
                            related.location.range =
                                utf16_range_to_buffer(&document.buffer, related.location.range);
                        }
                    }
                }
                if document.lsp_diagnostics.get(&server) == Some(&diagnostics) {
                    return;
                }
                document.lsp_diagnostics.insert(server, diagnostics);
                self.publish_document_diagnostics(doc_id);
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
                        "the {language} language server stopped; reconnecting with bounded backoff"
                    ),
                },
            ),
            LspUpdate::RuntimeState {
                server,
                root,
                state,
                error,
                ..
            } => {
                self.lsp
                    .note_runtime(server.clone(), root.clone(), state, error.clone());
                self.emit(
                    None,
                    Event::LanguageServerRuntimeChanged {
                        server,
                        root,
                        state,
                        error,
                    },
                );
            },
            LspUpdate::PreflightFailed { message, .. } => self.emit(
                None,
                Event::Notification {
                    severity: Severity::Warning,
                    kind: NotificationKind::Lsp,
                    message,
                },
            ),
            LspUpdate::InstallRequired {
                server, language, ..
            } => {
                match self.config.settings.lsp.managed_downloads {
                    crate::config::schema::ManagedDownloads::Prompt => {
                        // Ask at most once. A provider Karet has installed before
                        // has had this question answered already, and a refusal the
                        // user recorded is an answer too — re-asking either would
                        // spend their bandwidth on a decision they have made.
                        let root = self.config.lsp_registry_dir.as_deref();
                        if crate::lsp_registry::ever_installed(root, &server) {
                            return;
                        }
                        if crate::lsp_registry::read_declined(root, &server).is_some() {
                            return;
                        }
                        let enabled = self
                            .config
                            .settings
                            .lsp
                            .servers
                            .get(&language)
                            .is_none_or(|setting| setting.enabled);
                        self.emit(
                            None,
                            Event::LanguageServerInstallRequired {
                                server,
                                language,
                                enabled,
                            },
                        );
                    },
                    crate::config::schema::ManagedDownloads::Auto => {
                        let request = RequestId(0);
                        self.queue_lsp_registry(
                            request,
                            crate::lsp_registry::RegistryJob::Install { request, server },
                        );
                    },
                    crate::config::schema::ManagedDownloads::Off => {},
                }
            },
        }
    }

    pub(super) fn reopen_lsp_documents(&mut self, only: Option<crate::api::LanguageServerId>) {
        let documents: Vec<_> = self
            .store
            .docs
            .values()
            .filter(|document| {
                only.as_ref().is_none_or(|server| {
                    document
                        .language_selector
                        .and_then(crate::lsp::builtin_server)
                        .as_ref()
                        == Some(server)
                })
            })
            .map(|document| {
                (
                    document.language_selector,
                    document.lsp_language_id,
                    document.path.clone(),
                    document.buffer.version(),
                    document.buffer.text(),
                )
            })
            .collect();
        for (selector, lsp_language_id, path, version, text) in documents {
            self.lsp
                .document_opened(selector, lsp_language_id, &path, version, || text);
        }
    }

    pub(super) fn restart_lsp(&mut self, server: crate::api::LanguageServerId) {
        if self.lsp.restart(server) {
            // Restart advances a global generation and retires every slot so no
            // late answer from the old provider can be adopted.
            self.reopen_lsp_documents(None);
        }
    }

    pub(super) fn queue_lsp_registry(
        &self,
        request: RequestId,
        job: crate::lsp_registry::RegistryJob,
    ) {
        if self.lsp_registry.send(job).is_err() {
            self.emit(
                Some(request),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Lsp,
                    message: "language-server registry worker stopped".into(),
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
        doc.syntax_symbols = result.symbols;
        doc.error_lines = result.error_lines;
        self.publish(result.doc, None);
        self.schedule_spell(result.doc);
    }

    /// Queue the current token model after highlighting has settled. Disabled or
    /// unsupported settings clear only spell diagnostics, leaving other producers intact.
    pub(crate) fn schedule_spell(&mut self, doc_id: DocumentId) {
        // Every text-change path funnels through here, so the (synchronous,
        // line-based, sub-millisecond) markdown lint and the WakaTime typing
        // heartbeat ride along rather than adding hooks to each call site.
        self.refresh_markdown_lint(doc_id);
        self.wakatime_beat(doc_id, false);
        self.refresh_manifest_hints(doc_id);
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return;
        };
        let Some(spelling_language) = doc.settings.spelling_language else {
            self.clear_spell_diagnostics(doc_id);
            return;
        };
        let job = SpellJob {
            doc: doc_id,
            version: doc.buffer.version(),
            language: doc.language,
            language_selector: doc.language_selector,
            spelling_language,
            text: doc.buffer.text(),
            highlights: doc.highlights.clone(),
            syntax_error_lines: doc.error_lines.clone(),
            settings: self.config.settings.spellcheck.clone(),
        };
        if self.spell_tx.send(job).is_err() {
            self.clear_spell_diagnostics(doc_id);
        }
    }

    /// Adopt one versioned spell result and publish the complete spell layer.
    pub(crate) fn apply_spell_result(&mut self, result: SpellResult) {
        let Some(doc) = self.store.docs.get_mut(&result.doc) else {
            return;
        };
        if doc.buffer.version() != result.version {
            return;
        }
        if let Some(error) = result.error {
            if self.spell_errors.get(&result.doc) != Some(&error) {
                self.spell_errors.insert(result.doc, error.clone());
                self.emit(
                    None,
                    Event::Notification {
                        severity: Severity::Warning,
                        kind: NotificationKind::System,
                        message: error,
                    },
                );
            }
        } else {
            self.spell_errors.remove(&result.doc);
        }
        let Some(doc) = self.store.docs.get_mut(&result.doc) else {
            return;
        };
        if doc.spell_diagnostics == result.diagnostics {
            return;
        }
        doc.spell_diagnostics = result.diagnostics.clone();
        self.publish_document_diagnostics(result.doc);
        self.publish_spelling(result.doc);
    }

    fn clear_spell_diagnostics(&mut self, doc_id: DocumentId) {
        self.spell_errors.remove(&doc_id);
        let changed = self.store.docs.get_mut(&doc_id).is_some_and(|doc| {
            if doc.spell_diagnostics.is_empty() {
                false
            } else {
                doc.spell_diagnostics.clear();
                true
            }
        });
        if changed {
            self.publish_document_diagnostics(doc_id);
            self.publish_spelling(doc_id);
        }
    }

    pub(crate) fn publish_document_diagnostics(&self, doc_id: DocumentId) {
        let Some(document) = self.store.docs.get(&doc_id) else {
            return;
        };
        let lsp_count = document
            .lsp_diagnostics
            .values()
            .map(Vec::len)
            .sum::<usize>();
        let mut diagnostics = Vec::with_capacity(lsp_count + document.spell_diagnostics.len());
        diagnostics.extend(
            document
                .lsp_diagnostics
                .values()
                .flat_map(|layer| layer.iter().cloned()),
        );
        diagnostics.extend(document.spell_diagnostics.iter().cloned());
        diagnostics.extend(document.lint_diagnostics.iter().cloned());
        diagnostics.sort_by(|left, right| {
            left.range
                .start
                .cmp(&right.range.start)
                .then_with(|| left.severity.cmp(&right.severity))
                .then_with(|| left.source.cmp(&right.source))
                .then_with(|| left.message.cmp(&right.message))
        });
        diagnostics.dedup();
        self.emit(
            None,
            Event::DiagnosticsPublished {
                doc: doc_id,
                diagnostics,
            },
        );
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
        self.debug.reconfigure(report.settings.debug.clone());
        let lsp_changed = self.lsp.reconfigure(report.settings.lsp.clone());
        self.config.settings = report.settings.clone();
        self.config.loaded_config = report.clone();
        let docs: Vec<DocumentId> = self.store.docs.keys().copied().collect();
        self.refresh_document_settings(&docs);

        // Semantic-comment settings can vary by language. Requeue every open
        // document from scratch so both global and selector changes take effect.
        let settings = &self.config.settings;
        let highlight_tx = &self.highlight_tx;
        let mut spell_without_syntax = Vec::new();
        for (&doc_id, doc) in &mut self.store.docs {
            if update_syntax(settings, highlight_tx, doc_id, doc, None) {
                spell_without_syntax.push(doc_id);
            }
        }
        for doc_id in spell_without_syntax {
            self.schedule_spell(doc_id);
        }

        if lsp_changed {
            let lsp = &mut self.lsp;
            for doc in self.store.docs.values() {
                lsp.document_opened(
                    doc.language_selector,
                    doc.lsp_language_id,
                    &doc.path,
                    doc.buffer.version(),
                    || doc.buffer.text(),
                );
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
                    .map(|doc| (*doc_id, doc.path.clone(), doc.language_selector))
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
                self.schedule_spell(doc_id);
            }
        }
    }

    /// The text of `path` for column conversion: the live buffer when the file is
    /// open, else its contents from disk.
    ///
    /// A server answers in UTF-16 columns for whatever file it names, which is often
    /// not one the client has open — so converting those columns needs the target's
    /// text, not the requesting document's. `None` when the file cannot be read; the
    /// caller then leaves the range as the server sent it, since an approximate
    /// column beats a dropped result.
    fn buffer_for_path(&self, path: &Path) -> Option<TextBuffer> {
        self.store
            .by_path
            .get(path)
            .and_then(|doc| self.store.docs.get(doc))
            .map(|document| document.buffer.clone())
            .or_else(|| {
                std::fs::read_to_string(path)
                    .ok()
                    .map(|text| TextBuffer::from_text(&text))
            })
    }
}

pub(super) fn convert_symbol_columns(buffer: &TextBuffer, symbols: &mut [Symbol]) {
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

pub(super) fn utf16_range_to_buffer(buffer: &TextBuffer, range: Range) -> Range {
    Range {
        start: buffer.utf16_to_line_col(range.start.line, range.start.col),
        end: buffer.utf16_to_line_col(range.end.line, range.end.col),
    }
}

/// A caret converted to the server's UTF-16 column encoding — the conversion
/// every positional LSP forwarder applies (see the karet-lsp crate docs).
pub(super) fn utf16_caret(doc: &Document, position: LineCol) -> LineCol {
    LineCol::new(position.line, doc.buffer.line_col_to_utf16(position))
}

#[cfg(not(feature = "mdlint"))]
impl Session {
    /// Without the `mdlint` feature there is no markdown lint layer.
    pub(crate) fn refresh_markdown_lint(&mut self, _doc: crate::api::DocumentId) {}
}
