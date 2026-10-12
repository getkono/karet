//! Completion through the session seam: asking the document's language server
//! for candidates ([`Command::Completion`]) and for what it deferred about one
//! of them ([`Command::ResolveCompletion`]), and handing the answers back with
//! every range in buffer columns.

use karet_core::CompletionItem;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;

use super::updates::utf16_caret;
use super::updates::utf16_range_to_buffer;
use super::*;

impl Session {
    /// Serve a [`Command::Completion`] or [`Command::ResolveCompletion`];
    /// any other command is not this module's and is ignored.
    pub(super) fn completion_command(&mut self, id: RequestId, command: Command) {
        match command {
            Command::Completion {
                doc,
                position,
                trigger,
            } => self.completion(id, doc, (position, trigger)),
            Command::ResolveCompletion { doc, item } => self.resolve_completion(id, doc, item),
            _ => {},
        }
    }

    /// Serve [`Command::Completion`]: convert the caret to the server's UTF-16
    /// encoding and forward to the document's language server. Languages with no
    /// server answer immediately with an empty set, so the client never waits.
    fn completion(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        (position, trigger): (LineCol, CompletionTrigger),
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let version = doc.buffer.version();
        let at = (utf16_caret(doc, position), trigger);
        let forwarded =
            self.lsp
                .completion(doc.language_selector, id, doc_id, version, &doc.path, at);
        if !forwarded {
            self.emit(
                Some(id),
                Event::Completions {
                    doc: doc_id,
                    version,
                    items: Vec::new(),
                    incomplete: false,
                    options: CompletionOptions::default(),
                },
            );
        }
    }

    /// Serve [`Command::ResolveCompletion`] from the server that completes in
    /// the document. With no server to ask, the answer is that nothing was
    /// resolved, so the client stops waiting and does not ask again.
    fn resolve_completion(&mut self, id: RequestId, doc_id: DocumentId, item: CompletionItem) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let forwarded =
            self.lsp
                .resolve_completion(doc.language_selector, id, doc_id, &doc.path, item);
        if !forwarded {
            let resolved = None;
            self.emit(
                Some(id),
                Event::CompletionResolved {
                    doc: doc_id,
                    resolved,
                },
            );
        }
    }

    /// Hand a server's completion answer -- an [`LspUpdate::Completions`] or
    /// [`LspUpdate::CompletionResolved`] -- to the client with every range in
    /// buffer columns. A list for a document closed since is stale by
    /// definition and dropped.
    pub(super) fn deliver_completion(&mut self, update: LspUpdate) {
        match update {
            LspUpdate::Completions {
                request,
                doc,
                version,
                mut list,
                options,
                ..
            } => {
                if !self.store.docs.contains_key(&doc) {
                    return;
                }
                for item in &mut list.items {
                    self.completion_to_buffer_columns(doc, item);
                }
                let answer = Event::Completions {
                    doc,
                    version,
                    items: list.items,
                    incomplete: list.incomplete,
                    options,
                };
                self.emit(Some(request), answer);
            },
            LspUpdate::CompletionResolved {
                request,
                doc,
                mut resolved,
                ..
            } => {
                if let Some(item) = resolved.as_mut() {
                    self.completion_to_buffer_columns(doc, item);
                }
                self.emit(Some(request), Event::CompletionResolved { doc, resolved });
            },
            _ => {},
        }
    }

    /// Convert every range `item` carries -- its edit, its replace range, its
    /// additional edits -- from the server's UTF-16 columns to `doc`'s buffer
    /// columns. A closed document leaves them as they are.
    fn completion_to_buffer_columns(&self, doc: DocumentId, item: &mut CompletionItem) {
        let Some(open) = self.store.docs.get(&doc) else {
            return;
        };
        let ranges = item
            .edit
            .iter_mut()
            .chain(item.additional_edits.iter_mut())
            .map(|edit| &mut edit.range)
            .chain(item.replace_range.as_mut());
        for range in ranges {
            *range = utf16_range_to_buffer(&open.buffer, *range);
        }
    }
}
