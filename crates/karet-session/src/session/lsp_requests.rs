//! Forwarding one client request to the language server, and converting its
//! answer back into neutral models.
//!
//! Split out of [`super::updates`] to keep that module under the workspace's
//! per-file code line ceiling. Everything here has one shape: resolve the
//! document, hand the request to the LSP manager, emit the answer.

use karet_core::InlayHint;
use karet_core::Location;
use karet_core::WorkspaceSymbol;
use karet_text::TextBuffer;

use super::updates::utf16_caret;
use super::updates::utf16_range_to_buffer;
use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::api::FormatRequest;
use crate::api::FormatScope;
use crate::api::LocationKind;
use crate::api::RenameTarget;
use crate::lsp::FormattingScope;
use crate::lsp::LspUpdate;
use crate::lsp::RenameAnswer;
use crate::lsp::SelectionRangesAnswer;

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
    pub(super) fn locations_to_buffer_columns<'a>(
        &self,
        locations: impl IntoIterator<Item = &'a mut Location>,
    ) {
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

    /// Move workspace symbols from the server's UTF-16 columns to buffer
    /// columns, each against its own file when that file is open here.
    ///
    /// A symbol in a file that is not open keeps the server's columns. A
    /// workspace search answers every typing pause with hits across any number
    /// of files, so reading each from disk here would put that much blocking
    /// I/O on the session for rows nobody may pick -- the trade inlay-hint
    /// part locations make (see [`Self::hints_to_buffer_columns`]). The line is
    /// right either way; the column differs only where an astral-plane
    /// character precedes the symbol on its line.
    pub(super) fn workspace_symbols_to_buffer_columns(&self, symbols: &mut [WorkspaceSymbol]) {
        for symbol in symbols {
            let Some(range) = symbol.range.as_mut() else {
                continue;
            };
            let open = self
                .store
                .by_path
                .get(&symbol.path)
                .and_then(|doc| self.store.docs.get(doc));
            if let Some(document) = open {
                *range = utf16_range_to_buffer(&document.buffer, *range);
            }
        }
    }

    /// Move `hints` from the server's UTF-16 columns to buffer columns: each
    /// position against `doc`'s buffer, and each label part's location
    /// against the file it names when that file is open. Hints for a document
    /// closed since they were asked for are left as they are; nobody will
    /// adopt them.
    ///
    /// A part location into a file that is not open keeps the server's
    /// columns. Hint lists arrive on every edit pause and scroll, so reading
    /// each named file from disk here would put blocking I/O on that path for
    /// locations nobody may follow; UTF-16 and buffer columns differ only
    /// where an astral-plane character precedes the target on its line. A
    /// resolve, which is asked for one hint on demand, converts against the
    /// file on disk (see [`Self::part_locations_to_buffer_columns`]).
    pub(super) fn hints_to_buffer_columns(&self, doc: DocumentId, hints: &mut [InlayHint]) {
        let Some(document) = self.store.docs.get(&doc) else {
            return;
        };
        for hint in hints.iter_mut() {
            hint.position = document
                .buffer
                .utf16_to_line_col(hint.position.line, hint.position.col);
        }
        let locations = hints.iter_mut().flat_map(|hint| {
            hint.parts
                .iter_mut()
                .filter_map(|part| part.location.as_mut())
        });
        for location in locations {
            let open = self
                .store
                .by_path
                .get(&location.path)
                .and_then(|doc| self.store.docs.get(doc));
            if let Some(open) = open {
                location.range = utf16_range_to_buffer(&open.buffer, location.range);
            }
        }
    }

    /// Convert every label part's location in `hints` to buffer columns, each
    /// against the file it names, as a definition's is. A resolved hint needs
    /// only this: it keeps the position the client asked about.
    pub(super) fn part_locations_to_buffer_columns(&self, hints: &mut [InlayHint]) {
        self.locations_to_buffer_columns(hints.iter_mut().flat_map(|hint| {
            hint.parts
                .iter_mut()
                .filter_map(|part| part.location.as_mut())
        }));
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

    /// Serve [`Command::ResolveInlayHint`] from the server that answered the
    /// hint: the document's primary, which is the only one asked for hints.
    /// With no server to ask the hint comes straight back, so the client
    /// stops waiting on it and does not ask again.
    pub(super) fn resolve_inlay_hint(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        hint: InlayHint,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let unanswered =
            self.lsp
                .resolve_inlay_hint(doc.language_selector, id, doc_id, &doc.path, hint);
        if let Some(hint) = unanswered {
            self.emit(Some(id), Event::InlayHintResolved { doc: doc_id, hint });
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

    /// Serve a [`Command::Hover`] or a [`Command::SignatureHelp`]: the
    /// requests about the caret whose answers the client shows in a popup.
    pub(super) fn at_caret(&mut self, id: RequestId, cmd: Command) {
        match cmd {
            Command::Hover { doc, position } => self.hover(id, doc, position),
            Command::SignatureHelp {
                doc,
                position,
                manual,
            } => self.signature_help(id, doc, position, manual),
            _ => {},
        }
    }

    /// Forward a [`Command::SelectionRanges`], or answer it with an empty
    /// chain per position when no server serves the document.
    pub(super) fn selection_ranges(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        positions: &[LineCol],
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let utf16 = positions.iter().map(|&at| utf16_caret(doc, at)).collect();
        let request = (id, doc_id, version);
        if !self
            .lsp
            .selection_ranges(doc.language_selector, request, &doc.path, utf16)
        {
            let chains = vec![Vec::new(); positions.len()];
            let answer = Event::SelectionRanges {
                doc: doc_id,
                version,
                chains,
            };
            self.emit(Some(id), answer);
        }
    }

    /// Settle an answer the generation fence turned away: a save's formatting
    /// still completes the save, unformatted, and selection ranges still
    /// answer the request with empty chains, because `Command::SelectionRanges`
    /// is always answered and Expand Selection waits on it. A code-lens
    /// listing is answered as unanswered, so the client asks again, and a
    /// lens run reports how it went, since its command did run. Every other
    /// fenced answer is advisory and simply dropped.
    pub(super) fn settle_retired_answer(&mut self, update: LspUpdate) {
        match update {
            LspUpdate::Formatting {
                request,
                doc,
                version,
                ..
            } => {
                let _ = self.finish_format_on_save(request, doc, version, Vec::new());
            },
            LspUpdate::SelectionRanges { mut answer, .. } => {
                answer.chains.iter_mut().for_each(Vec::clear);
                self.adopt_selection_ranges(answer);
            },
            LspUpdate::CodeLens { answer, .. } => self.adopt_code_lens(answer.fenced()),
            _ => {},
        }
    }

    /// Turn a server's selection ranges into [`Event::SelectionRanges`], in
    /// buffer columns. A document that has changed since the request gets
    /// empty chains: the ranges describe text that is no longer there.
    pub(super) fn adopt_selection_ranges(&mut self, answer: SelectionRangesAnswer) {
        let SelectionRangesAnswer {
            request,
            doc,
            version,
            mut chains,
        } = answer;
        match self.store.docs.get(&doc) {
            Some(document) if document.buffer.version() == version => {
                for range in chains.iter_mut().flatten() {
                    *range = utf16_range_to_buffer(&document.buffer, *range);
                }
            },
            _ => chains.iter_mut().for_each(Vec::clear),
        }
        let event = Event::SelectionRanges {
            doc,
            version,
            chains,
        };
        self.emit(Some(request), event);
    }

    /// Forward a [`Command::SignatureHelp`], or answer it empty when no
    /// server serves the document.
    fn signature_help(&mut self, id: RequestId, doc_id: DocumentId, at: LineCol, manual: bool) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let utf16 = utf16_caret(doc, at);
        if !self.lsp.signature_help(
            doc.language_selector,
            (id, doc_id),
            &doc.path,
            utf16,
            manual,
        ) {
            let options = karet_core::SignatureHelpOptions::default();
            let empty = Event::SignatureHelp {
                doc: doc_id,
                help: None,
                options,
            };
            self.emit(Some(id), empty);
        }
    }

    fn hover(&mut self, id: RequestId, doc_id: DocumentId, position: LineCol) {
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

/// The refusal when nothing can format a document asked to be formatted.
pub(super) const NO_FORMATTER: &str = "format: nothing formats this file";

/// The refusal when nothing can format a selection asked to be formatted: the
/// built-in formatter only ever rewrites a whole document.
pub(super) const NO_RANGE_FORMATTER: &str =
    "format selection: nothing formats selections in this file";

/// The notice when the document moved on before its formatter answered.
pub(super) const FORMAT_STALE: &str =
    "format: the document changed before the formatter answered; nothing was applied";

/// The notice when a formatter's edits cannot be applied together.
pub(super) const FORMAT_OVERLAP: &str =
    "format: the formatter's edits overlap; nothing was applied";

impl Session {
    /// Serve [`Command::Format`]: hand the document, or its selected ranges, to
    /// the formatter its language names -- the same choice a save makes --
    /// and fall back on the built-in formatter for a whole document when no
    /// server runs for it.
    pub(super) fn format(&mut self, id: RequestId, request: FormatRequest) {
        let doc_id = request.doc;
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let scope = match request.scope {
            FormatScope::Document => FormattingScope::Document,
            FormatScope::Ranges(ranges) => FormattingScope::Ranges(
                format_ranges(&doc.buffer, ranges)
                    .into_iter()
                    .map(|range| Range {
                        start: utf16_caret(doc, range.start),
                        end: utf16_caret(doc, range.end),
                    })
                    .collect(),
            ),
        };
        if matches!(&scope, FormattingScope::Ranges(ranges) if ranges.is_empty()) {
            return;
        }
        let ask = self.formatting_ask(doc, scope.clone());
        let version = doc.buffer.version();
        let path = doc.path.clone();
        if !self
            .lsp
            .formatting(doc.language_selector, id, doc_id, version, &path, ask)
        {
            self.format_without_server(id, doc_id, &scope, true);
        }
    }

    /// Adopt a server's answer to a [`Command::Format`]: its edits, converted
    /// to buffer columns, as one undoable change -- or nothing, when the
    /// document moved on while the server worked, since the edits describe
    /// text that is no longer there.
    pub(super) fn adopt_format(
        &mut self,
        request: RequestId,
        doc_id: DocumentId,
        version: u64,
        (formatted, mut edits): (bool, Vec<TextEdit>),
        scope: &FormattingScope,
    ) {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return; // closed since the request: stale by definition
        };
        if doc.buffer.version() != version {
            self.emit(Some(request), format_notice(FORMAT_STALE));
            return;
        }
        if !formatted {
            // The server refused (and said so), failed, or was unreachable:
            // the built-in formatter is the only one left to ask.
            self.format_without_server(request, doc_id, scope, false);
            return;
        }
        for edit in &mut edits {
            edit.range = utf16_range_to_buffer(&doc.buffer, edit.range);
        }
        self.apply_requested_format(request, doc_id, edits);
    }

    /// Format with the session's own formatter where no server did: a whole
    /// document only, and only a language it formats. With `report`, say so
    /// when it cannot; a server's refusal has already said why.
    fn format_without_server(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        scope: &FormattingScope,
        report: bool,
    ) {
        let builtin = *scope == FormattingScope::Document
            && self
                .store
                .docs
                .get(&doc_id)
                .is_some_and(|doc| self.has_builtin_formatter(doc));
        if builtin {
            let edits = self.builtin_format_edits(doc_id).unwrap_or_default();
            self.apply_requested_format(id, doc_id, edits);
        } else if report {
            let message = match scope {
                FormattingScope::Ranges(_) => NO_RANGE_FORMATTER,
                FormattingScope::Save | FormattingScope::Document => NO_FORMATTER,
            };
            self.emit(Some(id), format_notice(message));
        }
    }

    /// Apply the edits a [`Command::Format`] produced, reporting edits that
    /// cannot be applied together. Nothing to change is not a failure.
    fn apply_requested_format(&mut self, id: RequestId, doc_id: DocumentId, edits: Vec<TextEdit>) {
        if !edits.is_empty() && !self.apply_format_edits(doc_id, edits, false) {
            self.emit(Some(id), format_notice(FORMAT_OVERLAP));
        }
    }
}

/// A warning answering a [`Command::Format`] that changed nothing.
fn format_notice(message: &str) -> Event {
    Event::Notification {
        severity: Severity::Warning,
        kind: NotificationKind::Lsp,
        message: message.to_owned(),
    }
}

/// The ranges a Format Selection formats, in buffer columns: each clamped to
/// the document and put in order, an empty one (a bare caret) widened to its
/// whole line, and those that touch or overlap merged -- one request per
/// stretch of text, so no two answers rewrite the same characters.
pub(super) fn format_ranges(buffer: &TextBuffer, ranges: Vec<Range>) -> Vec<Range> {
    let mut ranges: Vec<Range> = ranges
        .into_iter()
        .map(|range| {
            let (a, b) = (
                clamp_to_document(buffer, range.start),
                clamp_to_document(buffer, range.end),
            );
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            if start == end {
                let line = LineCol::new(start.line, 0);
                Range {
                    start: line,
                    end: clamp_to_document(buffer, LineCol::new(start.line, u32::MAX)),
                }
            } else {
                Range { start, end }
            }
        })
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(from: (u32, u32), to: (u32, u32)) -> Range {
        Range {
            start: LineCol::new(from.0, from.1),
            end: LineCol::new(to.0, to.1),
        }
    }

    /// Carets widen to their line; reversed, overlapping and touching
    /// selections become one stretch each; a range past the end is pulled in.
    #[test]
    fn format_ranges_widen_carets_and_merge_what_overlaps() {
        let buffer = TextBuffer::from_text("alpha\nbeta\ngamma\ndelta\n");
        assert_eq!(
            format_ranges(
                &buffer,
                vec![
                    range((3, 2), (3, 2)),
                    range((1, 3), (0, 1)),
                    range((1, 2), (1, 4)),
                    range((1, 4), (2, 1)),
                    range((3, 0), (99, 99)),
                ],
            ),
            vec![range((0, 1), (2, 1)), range((3, 0), (4, 0))]
        );
        // Two carets on one line format it once.
        assert_eq!(
            format_ranges(&buffer, vec![range((2, 1), (2, 1)), range((2, 4), (2, 4))]),
            vec![range((2, 0), (2, 5))]
        );
        assert!(format_ranges(&buffer, Vec::new()).is_empty());
    }

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
