//! Code lenses: listing a document's for a client, and running the one it
//! clicks.
//!
//! A listing is answered by [`Event::CodeLenses`] in buffer columns, or by
//! [`Event::CodeLensesFailed`] when nothing was learned; a run by
//! [`Event::WorkspaceEdit`], labelled with the lens's title, as a code
//! action's command is, since the edits a command makes arrive the same way.

use karet_core::ApplyWorkspaceEditResult;
use karet_core::CodeLens;

use super::super::columns::utf16_range_to_buffer;
use super::super::lsp_requests::NO_SERVER;
use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::lsp::CodeLensAnswer;
use crate::lsp::CodeLensCmd;

impl Session {
    /// Serve [`LspCommand::CodeLenses`]: ask the document's server, answering
    /// at once with no lenses when no server runs for it.
    pub(in crate::session) fn code_lenses(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        lines: (u32, u32),
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let cmd = CodeLensCmd::List {
            request: id,
            doc: doc_id,
            version: doc.buffer.version(),
            path: doc.path.clone(),
            lines,
        };
        if let Some(cmd) = self.lsp.code_lens(doc.language_selector, &doc.path, cmd) {
            self.adopt_code_lens(cmd.unserved(NO_SERVER));
        }
    }

    /// Serve [`LspCommand::RunCodeLens`]: run the lens's command on the
    /// document's server. A lens with no command has nothing to run, and is
    /// answered as done.
    pub(in crate::session) fn run_code_lens(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        lens: CodeLens,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        if lens.command.is_none() {
            let answer = CodeLensAnswer::Ran {
                request: id,
                title: lens.title.unwrap_or_default(),
                result: Ok(()),
            };
            self.adopt_code_lens(answer);
            return;
        }
        let cmd = CodeLensCmd::Run {
            request: id,
            lens: Box::new(lens),
        };
        if let Some(cmd) = self.lsp.code_lens(doc.language_selector, &doc.path, cmd) {
            self.adopt_code_lens(cmd.unserved(NO_SERVER));
        }
    }

    /// Adopt a server task's code-lens answer.
    ///
    /// A listing made against text that has since changed is reported as
    /// unanswered: its ranges describe text that is no longer there, and
    /// converting them against the new text would misplace them.
    pub(in crate::session) fn adopt_code_lens(&mut self, answer: CodeLensAnswer) {
        match answer {
            CodeLensAnswer::Listed {
                request,
                doc,
                version,
                lenses,
            } => {
                let current = self
                    .store
                    .docs
                    .get(&doc)
                    .filter(|document| document.buffer.version() == version);
                let event = match (lenses, current) {
                    (Some(mut lenses), Some(document)) => {
                        for lens in &mut lenses {
                            lens.range = utf16_range_to_buffer(&document.buffer, lens.range);
                        }
                        Event::CodeLenses {
                            doc,
                            version,
                            lenses,
                        }
                    },
                    _ => Event::CodeLensesFailed { doc, version },
                };
                self.emit(Some(request), event);
            },
            CodeLensAnswer::Ran {
                request,
                title,
                result,
            } => {
                let result = match result {
                    Ok(()) => ApplyWorkspaceEditResult::applied(),
                    Err(reason) => ApplyWorkspaceEditResult::failed(reason, None),
                };
                let applied = AppliedWorkspaceEdit {
                    label: Some(title),
                    result,
                    ..AppliedWorkspaceEdit::default()
                };
                self.emit(Some(request), Event::WorkspaceEdit { applied });
            },
        }
    }
}
