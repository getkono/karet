//! Document-domain backend-event handlers: opens, saves, diagnostics,
//! symbols, and the non-UTF-8 fallback. Called only from the
//! [`App::on_backend_event`] router.

use super::*;

impl App {
    /// Bind an answered open to the code tab that requested it, or release the
    /// document if the view has since closed.
    pub(super) fn on_opened(&mut self, id: Option<RequestId>, doc: DocumentId) {
        if id.is_some_and(|request| self.abandoned_open.remove(&request)) {
            self.send_command(SessionCommand::CloseDocument { doc });
            return;
        }
        let pending = id.and_then(|request| self.pending_open.remove(&request));
        if let Some(pending) = pending {
            let mut bound = false;
            for tab in self.all_tabs_mut() {
                if tab.view == pending.view
                    && let TabKind::Code {
                        path, doc: slot, ..
                    } = &mut tab.kind
                    && slot.is_none()
                    && *path == pending.path
                {
                    *slot = Some(doc);
                    bound = true;
                    break;
                }
            }
            // The session counts every open, but the app holds one reference
            // per document (`reconcile_open_docs` closes it once): a second
            // view's open of a document already held gives its extra one back.
            if !bound || !self.open_docs.insert(doc) {
                self.send_command(SessionCommand::CloseDocument { doc });
            }
        }
    }

    /// An open the session answered with an error rather than a document: the
    /// requesting tab's text will never arrive, so it stops waiting and shows
    /// why. No `Opened` follows, so the request is forgotten here.
    ///
    /// Returns the notification tag the failure reports under when the request
    /// was a pending open, so the caller's toast collapses with any other failed
    /// open of the same path.
    pub(super) fn on_open_failed(&mut self, request: RequestId, message: &str) -> Option<String> {
        self.abandoned_open.remove(&request);
        let pending = self.pending_open.remove(&request)?;
        for tab in self.all_tabs_mut() {
            if tab.view == pending.view
                && tab.fill_caret.is_some()
                && matches!(&tab.kind, TabKind::Code { path, doc: None, .. } if *path == pending.path)
            {
                tab.fill_since = None;
                tab.fill_error = Some(message.to_string());
            }
        }
        Some(Self::open_failure_tag(&pending.path))
    }

    /// The notification tag every failed open of `path` shares.
    ///
    /// A split made before its source's open is answered sends a second
    /// `OpenDocument` for the same path, so a file that cannot open fails twice;
    /// sharing one tag per path collapses the two into one card.
    fn open_failure_tag(path: &Path) -> String {
        format!("io.open-failed:{}", path.display())
    }

    /// Merge a published diagnostic layer over the retained LaTeX layer.
    pub(super) fn on_diagnostics_published(
        &mut self,
        doc: DocumentId,
        diagnostics: Vec<Diagnostic>,
    ) {
        // This event currently carries the complete non-LaTeX layer
        // (spell checking today, with room for other producers). Keep
        // compiler feedback alive when that layer refreshes.
        let mut combined = diagnostics;
        if let Some(existing) = self.docs.diagnostics.get(&doc) {
            combined.extend(
                existing
                    .iter()
                    .filter(|diagnostic| diagnostic.source.as_deref() == Some("latex"))
                    .cloned(),
            );
        }
        self.replace_document_diagnostics(doc, combined);
        self.maybe_auto_complete_spelling(doc);
    }

    /// Drop every per-document cache for a closed session document.
    pub(super) fn on_document_closed(&mut self, doc: DocumentId) {
        self.docs.settings.remove(&doc);
        if self.docs.diagnostics.remove(&doc).is_some() {
            self.refresh_problems();
        }
        self.docs.symbols.remove(&doc);
        self.forget_inlay_hints(doc);
        self.forget_code_lenses(doc);
        self.forget_line_changes(Some(doc));
        self.docs.outline_versions.remove(&doc);
        self.docs.outline_loading.remove(&doc);
    }

    /// Adopt a symbol tree, resolving which buffer version it represents.
    pub(super) fn on_symbols(&mut self, doc: DocumentId, symbols: Vec<Symbol>) {
        let version = self
            .docs
            .outline_loading
            .remove(&doc)
            .map(|(version, _)| version)
            .or_else(|| {
                self.all_tabs().find_map(|tab| match &tab.kind {
                    TabKind::Code {
                        doc: Some(candidate),
                        buffer,
                        ..
                    } if *candidate == doc => Some(buffer.version()),
                    _ => None,
                })
            });
        self.docs.symbols.insert(doc, symbols);
        if let Some(version) = version {
            self.docs.outline_versions.insert(doc, version);
        }
        self.sync_outline_selection();
        self.refresh_document_symbols(doc);
    }

    /// Clear the dirty markers for a successfully saved document.
    pub(super) fn on_saved(&mut self, doc: DocumentId) {
        for tab in self.all_tabs_mut() {
            if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                tab.dirty = false;
            }
        }
        // Tagged: a save-all writes one document per event, and auto-save fires
        // on a timer while the user types. Untagged, either would stack a column
        // of identical cards over the editor.
        self.notify_tagged(
            Report::Outcome,
            NotificationKind::Io,
            "saved",
            Some(Self::SAVED_TAG.to_string()),
        );
    }

    /// Full non-UTF-8 editing isn't supported: the tab requested a document
    /// that will never arrive (no `Opened` follows), so leaving it as a
    /// `doc: None` code tab would make every keystroke silently no-op. Fall
    /// back to the same read-only hex view a corrupt CBOR file already uses.
    pub(super) fn on_not_utf8(&mut self, id: Option<RequestId>, path: PathBuf) {
        if let Some(req) = id {
            self.pending_open.remove(&req);
            self.abandoned_open.remove(&req);
        }
        for tab in self.all_tabs_mut() {
            let is_pending_for_path =
                matches!(&tab.kind, TabKind::Code { path: p, doc: None, .. } if *p == path);
            if is_pending_for_path && let Ok(bytes) = std::fs::read(&path) {
                tab.kind = TabKind::Hex {
                    path: path.clone(),
                    bytes,
                    scroll: 0,
                };
                tab.markdown_preview = None;
            }
        }
        self.notify_tagged(
            Report::Alert,
            NotificationKind::Io,
            format!("opened {} read-only: not valid UTF-8", path.display()),
            Some(Self::open_failure_tag(&path)),
        );
    }
}
