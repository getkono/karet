//! Forwarding one client request to the language server, and converting its
//! answer back into neutral models.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code line ceiling. Everything here has one shape: resolve the
//! document, hand the request to the LSP manager, emit the answer.

use karet_text::TextBuffer;

use super::updates::utf16_caret;
use super::*;

/// Clamp `at` to a position `buffer` actually has.
fn clamp_to_document(buffer: &TextBuffer, at: LineCol) -> LineCol {
    let last_line = (buffer.line_count() as u32).saturating_sub(1);
    let line = at.line.min(last_line);
    let width = buffer.line(line as usize).map_or(0, |text| {
        u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
    });
    LineCol::new(line, at.col.min(width))
}

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

    /// Serve [`Command::InlayHints`]: convert the range to the server's UTF-16
    /// encoding and forward it. A language with no server, or a server that
    /// does not offer hints, answers immediately with an empty set so the
    /// editor never holds a stale annotation waiting for a reply that is not
    /// coming.
    pub(super) fn inlay_hints(&mut self, id: RequestId, doc_id: DocumentId, range: Range) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        // Clamped against the buffer before converting. A caller asking for
        // "all of the last line" has no way to know its length, so it says
        // `u32::MAX`; converting that verbatim puts a column no document has
        // on the wire, and a server is entitled to reject the whole request.
        let utf16 = Range {
            start: utf16_caret(doc, clamp_to_document(&doc.buffer, range.start)),
            end: utf16_caret(doc, clamp_to_document(&doc.buffer, range.end)),
        };
        let forwarded =
            self.lsp
                .inlay_hints(doc.language_selector, id, doc_id, version, &doc.path, utf16);
        if !forwarded {
            self.emit(
                Some(id),
                Event::InlayHints {
                    doc: doc_id,
                    version,
                    hints: Vec::new(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_to_document_brings_a_whole_line_request_inside_the_text() {
        // `u32::MAX` is how a caller says "to the end of the line" without
        // knowing the line's length. It has to become the real end, counted
        // in characters -- the emoji is one column, not four bytes or two
        // UTF-16 units.
        let buffer = TextBuffer::from_text("ab\n😀c\n");
        assert_eq!(
            clamp_to_document(&buffer, LineCol::new(1, u32::MAX)),
            LineCol::new(1, 2)
        );
        // Past the last line lands on the last line, at its own end.
        let last = (buffer.line_count() as u32).saturating_sub(1);
        let last_width = buffer
            .line(last as usize)
            .map_or(0, |text| text.chars().count() as u32);
        assert_eq!(
            clamp_to_document(&buffer, LineCol::new(u32::MAX, u32::MAX)),
            LineCol::new(last, last_width)
        );
        // A position already inside the document is left alone.
        assert_eq!(
            clamp_to_document(&buffer, LineCol::new(0, 1)),
            LineCol::new(0, 1)
        );
    }

    #[test]
    fn clamp_to_document_handles_an_empty_buffer() {
        let buffer = TextBuffer::from_text("");
        assert_eq!(
            clamp_to_document(&buffer, LineCol::new(u32::MAX, u32::MAX)),
            LineCol::new(0, 0)
        );
    }
}
