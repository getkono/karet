//! Code actions: listing them for a client, and running the one it picks.
//!
//! Running one is a sequence the session drives, because only the session
//! holds the documents an action edits: the server fills in an edit it
//! deferred (`codeAction/resolve`), the session applies the edit through the
//! one workspace-edit path, and only then does the server run the action's
//! command. The client hears one answer, [`Event::WorkspaceEdit`], when the
//! sequence ends; edits the server applies while running the command arrive
//! before it, unrequested, as any server edit does.

use karet_core::ApplyWorkspaceEditResult;
use karet_core::CodeAction;
use karet_core::DocumentChange;
use karet_text::TextBuffer;

use super::lsp_requests::NO_SERVER;
use super::lsp_requests::clamp_to_document;
use super::lsp_requests::refused;
use super::updates::utf16_caret;
use super::updates::utf16_range_to_buffer;
use super::workspace_edit::Columns;
use super::*;
use crate::api::AppliedWorkspaceEdit;
use crate::api::CodeActionTrigger;
use crate::lsp::CodeActionAnswer;
use crate::lsp::CodeActionCmd;

/// The refusal for an action listed against text that has since changed:
/// its edit's ranges describe text that is gone.
const STALE: &str = "the document changed since the actions were listed; try again";

impl Session {
    /// Serve [`Command::CodeActions`]: ask the document's server, answering at
    /// once when no server runs for it.
    pub(super) fn code_actions(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        range: Range,
        kinds: Vec<String>,
        trigger: CodeActionTrigger,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let range = Range {
            start: utf16_caret(doc, clamp_to_document(&doc.buffer, range.start)),
            end: utf16_caret(doc, clamp_to_document(&doc.buffer, range.end)),
        };
        let cmd = CodeActionCmd::List {
            request: id,
            doc: doc_id,
            version: doc.buffer.version(),
            path: doc.path.clone(),
            range,
            kinds,
            trigger,
        };
        if let Some(cmd) = self.lsp.code_action(doc.language_selector, &doc.path, cmd) {
            self.adopt_code_action(cmd.unanswered(NO_SERVER));
        }
    }

    /// Serve [`Command::ApplyCodeAction`]: refuse an action that cannot run
    /// as listed, resolve a deferred edit, or go straight to applying.
    pub(super) fn apply_code_action(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        version: u64,
        action: CodeAction,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        if doc.buffer.version() != version {
            self.emit(
                Some(id),
                Event::WorkspaceEdit {
                    applied: refused(STALE),
                },
            );
            return;
        }
        if let Some(reason) = &action.disabled {
            let applied = refused(reason);
            self.emit(Some(id), Event::WorkspaceEdit { applied });
            return;
        }
        if action.edit.is_some() || action.payload.is_none() {
            self.finish_code_action(id, doc_id, version, action, Columns::Buffer);
            return;
        }
        // Possibly deferred: the server decides whether there is anything to
        // resolve, and hands the action back either way.
        let cmd = CodeActionCmd::Resolve {
            request: id,
            doc: doc_id,
            version,
            action: Box::new(action),
        };
        if let Some(cmd) = self.lsp.code_action(doc.language_selector, &doc.path, cmd) {
            self.adopt_code_action(cmd.unanswered(NO_SERVER));
        }
    }

    /// Adopt a server task's code-action answer.
    pub(super) fn adopt_code_action(&mut self, answer: CodeActionAnswer) {
        match answer {
            CodeActionAnswer::Listed {
                request,
                doc,
                version,
                actions,
            } => {
                let (actions, reason) = match actions {
                    Ok(mut actions) => {
                        self.actions_to_buffer_columns(&mut actions);
                        (actions, None)
                    },
                    Err(reason) => (Vec::new(), Some(reason)),
                };
                let event = Event::CodeActions {
                    doc,
                    version,
                    actions,
                    reason,
                };
                self.emit(Some(request), event);
            },
            CodeActionAnswer::Resolved {
                request,
                doc,
                version,
                action,
            } => match action {
                Ok(action) => {
                    self.finish_code_action(request, doc, version, action, Columns::Utf16)
                },
                Err(reason) => self.emit(
                    Some(request),
                    Event::WorkspaceEdit {
                        applied: refused(&reason),
                    },
                ),
            },
            CodeActionAnswer::Executed {
                request,
                applied,
                result,
            } => {
                let mut applied = *applied;
                if let Err(reason) = result {
                    applied.result = ApplyWorkspaceEditResult::failed(reason, None);
                }
                self.emit(Some(request), Event::WorkspaceEdit { applied });
            },
        }
    }

    /// Apply the action's edit (ranges counted in `columns`), then run its
    /// command, or answer as soon as there is nothing more to do.
    fn finish_code_action(
        &mut self,
        request: RequestId,
        doc_id: DocumentId,
        version: u64,
        mut action: CodeAction,
        columns: Columns,
    ) {
        // Checked again after a resolve: the user may have typed meanwhile.
        let current = self.store.docs.get(&doc_id).map(|doc| doc.buffer.version());
        if current != Some(version) {
            self.emit(
                Some(request),
                Event::WorkspaceEdit {
                    applied: refused(STALE),
                },
            );
            return;
        }
        let label = Some(action.title.clone());
        let applied = match action.edit.take() {
            Some(edit) => self.apply_workspace_edit(edit, columns, label),
            None => AppliedWorkspaceEdit {
                label,
                result: ApplyWorkspaceEditResult::applied(),
                ..AppliedWorkspaceEdit::default()
            },
        };
        // The document as it is now: the edit may have moved it.
        let target = self
            .store
            .docs
            .get(&doc_id)
            .map(|doc| (doc.language_selector, doc.path.clone()));
        let (Some((selector, path)), Some(_), true) =
            (target, &action.command, applied.result.applied)
        else {
            self.emit(Some(request), Event::WorkspaceEdit { applied });
            return;
        };
        let cmd = CodeActionCmd::Execute {
            request,
            applied: Box::new(applied),
            action: Box::new(action),
        };
        if let Some(cmd) = self.lsp.code_action(selector, &path, cmd) {
            self.adopt_code_action(cmd.unanswered(NO_SERVER));
        }
    }

    /// Convert every listed action's edit from the server's UTF-16 columns to
    /// buffer columns, reading each file at most once.
    fn actions_to_buffer_columns(&self, actions: &mut [CodeAction]) {
        let mut buffers: HashMap<PathBuf, Option<TextBuffer>> = HashMap::new();
        let mut convert = |path: &Path, edits: &mut [TextEdit]| {
            let buffer = buffers
                .entry(path.to_path_buf())
                .or_insert_with(|| self.buffer_for_path(path));
            // A file that cannot be read keeps the server's columns: its edit
            // cannot apply anyway, and that is reported when it is run.
            if let Some(buffer) = buffer {
                for edit in edits {
                    edit.range = utf16_range_to_buffer(buffer, edit.range);
                }
            }
        };
        for edit in actions.iter_mut().filter_map(|action| action.edit.as_mut()) {
            for (path, edits) in &mut edit.changes {
                convert(path, edits);
            }
            for change in &mut edit.document_changes {
                if let DocumentChange::Edit(document) = change {
                    convert(&document.path, &mut document.edits);
                }
            }
        }
    }
}
