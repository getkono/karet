//! Forwarding one client request to the language server, and converting its
//! answer back into neutral models.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code line ceiling. Everything here has one shape: resolve the
//! document, hand the request to the LSP manager, emit the answer.

use karet_core::Location;
use karet_text::TextBuffer;

use super::updates::utf16_caret;
use super::updates::utf16_range_to_buffer;
use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::api::LocationKind;
use crate::api::RenameTarget;
use crate::lsp::RenameAnswer;

/// The event answering a navigation request: [`Event::Definitions`] for Go to
/// Definition (`kind` `None`), else [`Event::Locations`] naming what was asked.
pub(super) fn locations_event(kind: Option<LocationKind>, locations: Vec<Location>) -> Event {
    match kind {
        None => Event::Definitions { locations },
        Some(kind) => Event::Locations { kind, locations },
    }
}

/// Clamp `at` to a position `buffer` actually has.
pub(super) fn clamp_to_document(buffer: &TextBuffer, at: LineCol) -> LineCol {
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
    pub(super) fn doc_or_report<'a>(
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

    /// The text of `path` for column conversion: the live buffer when the file is
    /// open, else its contents from disk.
    ///
    /// A server answers in UTF-16 columns for whatever file it names, which is often
    /// not one the client has open — so converting those columns needs the target's
    /// text, not the requesting document's. `None` when the file cannot be read; the
    /// caller then leaves the range as the server sent it, since an approximate
    /// column beats a dropped result.
    pub(super) fn buffer_for_path(&self, path: &Path) -> Option<TextBuffer> {
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

    /// Convert every location from the server's UTF-16 columns to buffer columns,
    /// each against its own file.
    ///
    /// An open file's live buffer is borrowed, not cloned, and a file that is not
    /// open is read from disk at most once per answer, so a references list with
    /// many hits in one file costs one read rather than one per hit. A file that
    /// cannot be read keeps the server's columns, as [`Self::buffer_for_path`]
    /// explains.
    pub(super) fn locations_to_buffer_columns(&self, locations: &mut [Location]) {
        let mut read: HashMap<PathBuf, Option<TextBuffer>> = HashMap::new();
        for location in locations {
            let open = self
                .store
                .by_path
                .get(&location.path)
                .and_then(|doc| self.store.docs.get(doc))
                .map(|document| &document.buffer);
            let buffer = match open {
                Some(buffer) => Some(buffer),
                None => read
                    .entry(location.path.clone())
                    .or_insert_with(|| {
                        std::fs::read_to_string(&location.path)
                            .ok()
                            .map(|text| TextBuffer::from_text(&text))
                    })
                    .as_ref(),
            };
            if let Some(buffer) = buffer {
                location.range = utf16_range_to_buffer(buffer, location.range);
            }
        }
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

    /// Answer a [`Command::Definition`] (`kind` `None`) or a
    /// [`Command::FindLocations`]: forward it, or answer it empty when no
    /// server serves the document.
    pub(super) fn definition(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        position: LineCol,
        kind: Option<LocationKind>,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = utf16_caret(doc, position);
        let (language, path) = (doc.language_selector, doc.path.clone());
        if !self
            .lsp
            .definition(language, id, doc_id, version, &path, utf16, kind)
        {
            self.emit(Some(id), locations_event(kind, Vec::new()));
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
        let version = doc.buffer.version();
        if !self.lsp.rename(
            doc.language_selector,
            id,
            doc_id,
            version,
            &doc.path,
            utf16,
            new_name,
        ) {
            self.emit(
                Some(id),
                Event::WorkspaceEdit {
                    applied: refused(NO_SERVER),
                },
            );
        }
    }

    /// Serve [`Command::PrepareRename`]: ask the document's server what can be
    /// renamed at `position`, answering at once when no server runs for it.
    pub(super) fn prepare_rename(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = utf16_caret(doc, position);
        let forwarded = self.lsp.prepare_rename(
            doc.language_selector,
            id,
            doc_id,
            version,
            &doc.path,
            utf16,
            position,
        );
        if !forwarded {
            self.adopt_prepare_rename(id, doc_id, version, position, RenameAnswer::NoServer);
        }
    }

    /// Turn a server's prepare-rename answer into [`Event::RenamePrepared`].
    ///
    /// `position` is the caret the request was made at, in buffer columns,
    /// for picking the identifier when the server leaves that to the client.
    /// The server's own range arrives in UTF-16 columns.
    pub(super) fn adopt_prepare_rename(
        &mut self,
        request: RequestId,
        doc_id: DocumentId,
        version: u64,
        position: LineCol,
        answer: RenameAnswer,
    ) {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return; // closed since the request: nothing left to rename in
        };
        let buffer = &doc.buffer;
        let (target, reason) = match answer {
            RenameAnswer::NoServer => (None, Some(NO_SERVER.to_owned())),
            RenameAnswer::Unsupported => (
                None,
                Some("the language server does not support rename".to_owned()),
            ),
            RenameAnswer::Refused(reason) => (
                None,
                Some(reason.unwrap_or_else(|| "nothing to rename here".to_owned())),
            ),
            RenameAnswer::Span(karet_core::PrepareRename::Range { range, placeholder }) => {
                let range = utf16_range_to_buffer(buffer, range);
                let placeholder = placeholder.unwrap_or_else(|| text_in(buffer, range));
                (Some(RenameTarget { range, placeholder }), None)
            },
            RenameAnswer::Span(_) | RenameAnswer::Identifier => {
                match identifier_at(buffer, position) {
                    Some(target) => (Some(target), None),
                    None => (None, Some("no symbol at the cursor".to_owned())),
                }
            },
        };
        self.emit(
            Some(request),
            Event::RenamePrepared {
                doc: doc_id,
                version,
                target,
                reason,
            },
        );
    }

    /// Apply a rename's edit, unless the document moved on since the request
    /// or the request produced none, whose reason is then the answer.
    pub(super) fn adopt_rename_edit(
        &mut self,
        request: RequestId,
        doc_id: DocumentId,
        version: u64,
        edit: Result<karet_core::WorkspaceEdit, String>,
    ) {
        let current = self.store.docs.get(&doc_id).map(|doc| doc.buffer.version());
        let applied = match edit {
            // The server computed the edit against text that is gone: applying
            // it would put the new name wherever the old offsets now land.
            _ if current != Some(version) => {
                refused("the document changed while renaming; try again")
            },
            Err(reason) => refused(&reason),
            Ok(edit) if edit.is_empty() => refused("the language server found nothing to rename"),
            Ok(edit) => self.apply_workspace_edit(edit, Columns::Utf16, None),
        };
        self.emit(Some(request), Event::WorkspaceEdit { applied });
    }

    /// Apply a language server's `workspace/applyEdit` and answer it. The
    /// client hears about it like any other edit, without a request id.
    pub(super) fn adopt_server_edit(&mut self, request: karet_lsp::ApplyEditRequest) {
        let edit = request.edit.clone();
        let applied = self.apply_workspace_edit(edit, Columns::Utf16, request.label.clone());
        request.respond(applied.result.clone());
        self.emit(None, Event::WorkspaceEdit { applied });
    }
}

/// The refusal when no language server serves the document.
pub(super) const NO_SERVER: &str = "no language server is running for this file";

/// An edit refused before anything was applied.
pub(super) fn refused(reason: &str) -> AppliedWorkspaceEdit {
    AppliedWorkspaceEdit {
        result: karet_core::ApplyWorkspaceEditResult::failed(reason, None),
        ..AppliedWorkspaceEdit::default()
    }
}

/// The text `range` covers in `buffer`.
fn text_in(buffer: &TextBuffer, range: Range) -> String {
    let (Ok(start), Ok(end)) = (
        buffer.line_col_to_byte(range.start),
        buffer.line_col_to_byte(range.end),
    ) else {
        return String::new();
    };
    buffer
        .text()
        .get(start.0..end.0)
        .unwrap_or_default()
        .to_owned()
}

/// The identifier touching `position`, by the editor's own word rules: the one
/// the caret is in, or the one it sits just after.
fn identifier_at(buffer: &TextBuffer, position: LineCol) -> Option<RenameTarget> {
    let line: Vec<char> = buffer.line(position.line as usize)?.chars().collect();
    let at = (position.col as usize).min(line.len());
    let is_word = |index: usize| {
        line.get(index)
            .copied()
            .is_some_and(karet_core::is_word_char)
    };
    let anchor = if is_word(at) {
        at
    } else if at > 0 && is_word(at - 1) {
        at - 1
    } else {
        return None;
    };
    let start = (0..=anchor)
        .rev()
        .take_while(|&index| is_word(index))
        .last()?;
    let end = (anchor..line.len())
        .take_while(|&index| is_word(index))
        .last()?
        + 1;
    let column = |index: usize| u32::try_from(index).unwrap_or(u32::MAX);
    Some(RenameTarget {
        range: Range {
            start: LineCol::new(position.line, column(start)),
            end: LineCol::new(position.line, column(end)),
        },
        placeholder: line.get(start..end)?.iter().collect(),
    })
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
