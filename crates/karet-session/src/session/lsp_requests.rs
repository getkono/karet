//! Forwarding one client request to the language server, and converting its
//! answer back into neutral models.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code line ceiling. Everything here has one shape: resolve the
//! document, hand the request to the LSP manager, emit the answer.

use super::updates::utf16_caret;
use super::*;

impl Session {
    /// The single document-lookup used by request handlers: answers `id` with the
    /// standard unknown-document notification when `doc_id` is not open.
    ///
    /// An associated function over the disjoint fields (not `&self`) so the
    /// returned borrow leaves `self.lsp` free for the forwarding call.
    fn doc_or_report<'a>(
        store: &'a DocumentStore,
        events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
        id: RequestId,
        doc_id: DocumentId,
    ) -> Option<&'a Document> {
        let doc = store.docs.get(&doc_id);
        if doc.is_none() {
            events.send((Some(id), unknown_document(doc_id))).ok();
        }
        doc
    }

    /// Serve [`Command::Completion`]: convert the caret to the server's UTF-16
    /// encoding and forward to the document's language server. Languages with no
    /// server answer immediately with an empty set, so the client never waits.
    pub(super) fn completion(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = utf16_caret(doc, position);
        let forwarded =
            self.lsp
                .completion(doc.language_selector, id, doc_id, version, &doc.path, utf16);
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
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let forwarded =
            self.lsp
                .document_symbols(doc.language_selector, id, doc_id, version, &doc.path);
        if !forwarded {
            self.emit(
                Some(id),
                Event::Symbols {
                    doc: doc_id,
                    symbols: doc.syntax_symbols.as_ref().clone(),
                },
            );
        }
    }

    pub(super) fn hover(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = utf16_caret(doc, position);
        if !self
            .lsp
            .hover(doc.language_selector, id, doc_id, version, &doc.path, utf16)
        {
            self.emit(Some(id), Event::HoverResult { hover: None });
        }
    }

    pub(super) fn definition(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = utf16_caret(doc, position);
        if !self
            .lsp
            .definition(doc.language_selector, id, doc_id, version, &doc.path, utf16)
        {
            self.emit(
                Some(id),
                Event::Definitions {
                    locations: Vec::new(),
                },
            );
        }
    }

    pub(super) fn workspace_symbols(&mut self, id: RequestId, query: String) {
        if !self.lsp.workspace_symbols(id, query) {
            self.emit(
                Some(id),
                Event::WorkspaceSymbols {
                    symbols: Vec::new(),
                },
            );
        }
    }

    pub(super) fn rename(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        position: LineCol,
        new_name: String,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let utf16 = utf16_caret(doc, position);
        if !self
            .lsp
            .rename(doc.language_selector, id, &doc.path, utf16, new_name)
        {
            self.emit(
                Some(id),
                Event::WorkspaceEdit {
                    edit: karet_core::WorkspaceEdit::default(),
                },
            );
        }
    }

    pub(super) fn format_document(&mut self, id: RequestId, doc_id: DocumentId) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        if !self.lsp.formatting(
            doc.language_selector,
            id,
            doc_id,
            doc.buffer.version(),
            &doc.path,
        ) {
            // No server offered formatting; TOML falls back to the built-in
            // taplo formatter (same engine the taplo LSP would use).
            #[cfg(feature = "toml-format")]
            if doc.language_selector == Some("toml")
                && self.config.settings.toml.format
                && let Some(formatted) =
                    crate::toml_format::format_toml(&doc.buffer.text(), &self.config.roots)
            {
                let version = doc.buffer.version();
                let end_line = u32::try_from(doc.buffer.text().lines().count()).unwrap_or(u32::MAX);
                self.emit(
                    Some(id),
                    Event::FormattingEdits {
                        doc: doc_id,
                        version,
                        edits: vec![karet_core::TextEdit {
                            range: karet_core::Range {
                                start: karet_core::LineCol::new(0, 0),
                                end: karet_core::LineCol::new(end_line, 0),
                            },
                            new_text: formatted,
                        }],
                    },
                );
                return;
            }
            self.emit(
                Some(id),
                Event::FormattingEdits {
                    doc: doc_id,
                    version: doc.buffer.version(),
                    edits: Vec::new(),
                },
            );
        }
    }
}
