//! The language-server requests an edit or a save makes: the ranges Start
//! Linked Editing links, the edits a typed character triggers, and the edits
//! a server makes before a save writes the file.
//!
//! Split from [`super::lsp_requests`] and [`super::documents`] to keep both
//! under the workspace's per-file code line ceiling.

use super::columns::edits_to_buffer_columns;
use super::columns::utf16_caret;
use super::columns::utf16_range_to_buffer;
use super::*;
use crate::api::SaveCause;
use crate::lsp::EditingAnswer;
use crate::lsp::EditingAsk;
use crate::lsp::EditingReply;
use crate::lsp::FormattingScope;

/// What `textDocument/willSaveWaitUntil` tells a server about a save from
/// `cause`. An unrecognised cause is a manual save, as [`SaveCause::Unknown`]
/// is read everywhere else.
pub(super) fn save_reason(cause: SaveCause) -> karet_lsp::SaveReason {
    match cause {
        SaveCause::FocusChange => karet_lsp::SaveReason::FocusOut,
        SaveCause::AutoDelay => karet_lsp::SaveReason::AfterDelay,
        SaveCause::Manual | SaveCause::Unknown => karet_lsp::SaveReason::Manual,
    }
}

impl Session {
    /// Request the server's will-save edits and LSP formatting (or apply the
    /// built-in TOML formatter) before the disk write; only the will-save
    /// edits when the document does not format on save. `true` means the save
    /// is waiting on an async reply.
    pub(super) fn begin_format_on_save(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        cause: SaveCause,
    ) -> bool {
        let Some(doc) = self.store.docs.get(&doc_id) else {
            return false;
        };
        let editor = self
            .config
            .settings
            .editor
            .for_language(doc.language_selector);
        let format = editor.format_on_save();
        let (version, selector, path) = (
            doc.buffer.version(),
            doc.language_selector,
            doc.path.clone(),
        );
        let scope = if format {
            FormattingScope::Save
        } else {
            FormattingScope::SaveUnformatted
        };
        let mut ask = self.formatting_ask(doc, scope);
        ask.will_save = Some(save_reason(cause));
        if self
            .lsp
            .formatting(selector, id, doc_id, version, &path, ask)
        {
            let issued_ms = self.elapsed_ms();
            self.pending_format_saves.insert(
                id,
                PendingFormatSave {
                    doc: doc_id,
                    issued_ms,
                },
            );
            // The user asked for this buffer to be on disk and it is not going
            // to be for a while: the write waits on a formatter that has up to
            // `editor.formatOnSaveTimeout` to answer. Until then the swap is the
            // only other copy there is, and the backup interval alone would not
            // have written one — a buffer edited in the last thirty seconds, or
            // edited since its last swap, has none. Write it now, so what the
            // force-quit path promises the user is recoverable actually is.
            self.back_up_document(doc_id);
            return true;
        }
        if format && let Some(edits) = self.builtin_format_edits(doc_id) {
            let _ = self.apply_format_edits(doc_id, edits, true);
        }
        false
    }

    /// Forward a
    /// [`LspCommand::LinkedEditingRanges`](crate::api::LspCommand::LinkedEditingRanges),
    /// or answer it with none when no server serves the document.
    pub(super) fn linked_editing_ranges(&mut self, id: RequestId, doc_id: DocumentId, at: LineCol) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let ask = EditingAsk::Linked {
            position: utf16_caret(doc, at),
        };
        let request = (id, doc_id, version);
        if !self
            .lsp
            .editing(doc.language_selector, request, &doc.path, ask)
        {
            let answer = Event::LinkedEditingRanges {
                doc: doc_id,
                version,
                ranges: None,
            };
            self.emit(Some(id), answer);
        }
    }

    /// Forward a [`LspCommand::FormatOnType`](crate::api::LspCommand::FormatOnType),
    /// indented as the document is, or
    /// answer it with no edits when no server serves the document.
    pub(super) fn format_on_type(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        at: LineCol,
        ch: char,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let ask = EditingAsk::OnType {
            position: utf16_caret(doc, at),
            ch,
            indentation: self
                .formatting_ask(doc, FormattingScope::Document)
                .indentation,
        };
        let request = (id, doc_id, version);
        if !self
            .lsp
            .editing(doc.language_selector, request, &doc.path, ask)
        {
            let answer = Event::OnTypeFormatting {
                doc: doc_id,
                version,
                edits: Vec::new(),
                triggers: None,
            };
            self.emit(Some(id), answer);
        }
    }

    /// Turn a server's [`EditingAnswer`] into its event, in buffer columns,
    /// or carry on the save it belongs to. A document that has changed since
    /// the request gets no ranges and no edits: they describe text that is no
    /// longer there.
    pub(super) fn adopt_editing(&mut self, answer: EditingAnswer) {
        let EditingAnswer {
            request,
            doc,
            version,
            reply,
        } = answer;
        let current = self
            .store
            .docs
            .get(&doc)
            .filter(|document| document.buffer.version() == version);
        let event = match reply {
            EditingReply::Linked(ranges) => Event::LinkedEditingRanges {
                doc,
                version,
                ranges: current.and_then(|document| {
                    ranges.map(|found| {
                        found
                            .into_iter()
                            .map(|range| utf16_range_to_buffer(&document.buffer, range))
                            .collect()
                    })
                }),
            },
            EditingReply::OnType { edits, triggers } => Event::OnTypeFormatting {
                doc,
                version,
                edits: current.map_or_else(Vec::new, |document| {
                    edits_to_buffer_columns(&document.buffer, edits, Columns::Utf16)
                }),
                triggers,
            },
            EditingReply::WillSave { edits, then_format } => {
                self.adopt_will_save(request, (doc, version), edits, then_format);
                return;
            },
        };
        self.emit(Some(request), event);
    }

    /// Settle an [`EditingAnswer`] the generation fence turned away: a save
    /// still completes, without the edits, and every other request is still
    /// answered, with nothing, because the client waits on it.
    pub(super) fn settle_retired_editing(&mut self, mut answer: EditingAnswer) {
        answer.reply = match answer.reply {
            EditingReply::Linked(_) => EditingReply::Linked(None),
            EditingReply::OnType { .. } => EditingReply::OnType {
                edits: Vec::new(),
                triggers: None,
            },
            EditingReply::WillSave { .. } => {
                let _ = self.finish_format_on_save(
                    answer.request,
                    answer.doc,
                    answer.version,
                    Vec::new(),
                );
                return;
            },
        };
        self.adopt_editing(answer);
    }

    /// Apply a parked save's will-save edits, then format it, when it
    /// formats, or write it. A save that has since been written, cancelled or
    /// closed is not waiting any more and is left alone; one whose document
    /// changed meanwhile is written without the edits, as a stale formatting
    /// answer is.
    fn adopt_will_save(
        &mut self,
        request: RequestId,
        (doc_id, version): (DocumentId, u64),
        edits: Vec<TextEdit>,
        then_format: bool,
    ) {
        if !self.pending_format_saves.contains_key(&request) {
            return;
        }
        let Some(document) = self
            .store
            .docs
            .get(&doc_id)
            .filter(|document| document.buffer.version() == version)
        else {
            let _ = self.finish_format_on_save(request, doc_id, version, Vec::new());
            return;
        };
        let edits = edits_to_buffer_columns(&document.buffer, edits, Columns::Utf16);
        if !edits.is_empty() {
            let _ = self.apply_format_edits(doc_id, edits, true);
        }
        let Some(document) = self.store.docs.get(&doc_id) else {
            return;
        };
        let version = document.buffer.version();
        if !then_format {
            let _ = self.finish_format_on_save(request, doc_id, version, Vec::new());
            return;
        }
        // The same request, still parked under the same deadline, asks again
        // against the text the edits produced -- with only the time that
        // deadline has left, so the server task is not held for a second
        // full `formatOnSaveTimeout` past the point the save goes to disk.
        let mut ask = self.formatting_ask(document, FormattingScope::Save);
        let spent = self
            .pending_format_saves
            .get(&request)
            .map_or(0, |save| self.elapsed_ms().saturating_sub(save.issued_ms));
        ask.save_timeout = ask
            .save_timeout
            .saturating_sub(std::time::Duration::from_millis(spent));
        let (selector, path) = (document.language_selector, document.path.clone());
        if self
            .lsp
            .formatting(selector, request, doc_id, version, &path, ask)
        {
            return;
        }
        let edits = self.builtin_format_edits(doc_id).unwrap_or_default();
        let _ = self.finish_format_on_save(request, doc_id, version, edits);
    }

    /// Adopt a formatting answer that belongs to a save: its edits in buffer
    /// columns, or where no language server formatted the file the built-in
    /// formatter's -- unless the save does not format at all. A document that
    /// changed meanwhile is written unformatted.
    pub(super) fn adopt_save_format(
        &mut self,
        request: RequestId,
        (doc, version): (DocumentId, u64),
        (formatted, edits): (bool, Vec<TextEdit>),
        scope: &FormattingScope,
    ) {
        let current = self
            .store
            .docs
            .get(&doc)
            .filter(|document| document.buffer.version() == version);
        let edits = match current {
            None => Vec::new(),
            Some(document) if formatted => {
                edits_to_buffer_columns(&document.buffer, edits, Columns::Utf16)
            },
            // No language server formatted this file -- it never offered the
            // method, was not reachable, or failed the request. That is
            // exactly the case the built-in formatter exists for, and it
            // produces buffer coordinates already.
            Some(_) if *scope == FormattingScope::Save => {
                self.builtin_format_edits(doc).unwrap_or_default()
            },
            Some(_) => Vec::new(),
        };
        let _ = self.finish_format_on_save(request, doc, version, edits);
    }
}
