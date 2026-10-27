//! Rename Symbol (F2): the inline prompt over the symbol, and adopting the
//! workspace edits the backend applies.
//!
//! The backend owns the whole edit -- `PrepareRename` says what can be
//! renamed, `Rename` asks the language server and applies its answer through
//! the one workspace-edit path, which also serves the server's own
//! `workspace/applyEdit`. This module is the client half: the prompt, and
//! keeping the tabs in step with what an edit opened or moved.
//!
//! The prompt opens at once, prefilled with the identifier under the caret, so
//! F2 never waits on a round trip to show something; the server's answer then
//! corrects the span and the suggested name (unless the user has started
//! typing), or closes the prompt with its reason when nothing there can be
//! renamed.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use karet_core::LineCol;
use karet_core::NotificationKind;
use karet_core::Range;
use karet_session::AppliedWorkspaceEdit;
use karet_session::DocumentCommand;
use karet_session::DocumentId;
use karet_session::LspCommand;
use karet_session::RenameTarget;
use karet_session::RequestId;
use karet_session::ViewId;
use karet_text::TextBuffer;
use karet_widgets::textfield::TextFieldState;

use crate::app::App;
use crate::app::notifications::Report;
use crate::app::util::canonical;
use crate::app::util::rebase_path;
use crate::tab::Tab;
use crate::tab::TabKind;

/// The open rename prompt.
#[derive(Clone, Debug)]
pub(crate) struct RenamePrompt {
    /// The document the symbol is in.
    pub(crate) doc: DocumentId,
    /// The view the prompt belongs to; switching away closes it.
    pub(crate) view: ViewId,
    /// The symbol's span, in buffer columns. The prompt is anchored at it.
    pub(crate) range: Range,
    /// The name being replaced, so submitting it unchanged renames nothing.
    pub(crate) original: String,
    /// The new name as typed.
    pub(crate) text: String,
    /// Cursor and selection in [`text`](Self::text).
    pub(crate) edit: TextFieldState,
    /// The `PrepareRename` request still awaiting an answer, if any.
    pub(crate) preparing: Option<RequestId>,
    /// Whether the user has edited the name; a late answer then keeps it.
    pub(crate) touched: bool,
    /// Whether the prompt is on screen. It is not while the caret is on
    /// nothing the editor recognizes as a name, until the server says what is.
    pub(crate) shown: bool,
}

/// A submitted rename awaiting the backend's answer.
#[derive(Clone, Debug)]
pub(crate) struct PendingRename {
    /// The `Rename` request.
    pub(crate) id: RequestId,
    /// The name it renames to, for the notice.
    pub(crate) name: String,
}

/// The identifier the editor sees at `pos`, if the caret is on (or just past)
/// one: its span and text.
fn local_identifier(buffer: &TextBuffer, pos: LineCol) -> Option<(Range, String)> {
    let (start, end) = karet_text::words::word_at(buffer, pos)?;
    let range = Range { start, end };
    let name = buffer.slice_range(range)?;
    Some((range, name))
}

impl App {
    /// F2: open the rename prompt for the symbol at the caret, and ask the
    /// backend what exactly can be renamed there.
    pub(crate) fn start_rename(&mut self) {
        let Some((doc, caret)) = self.completion_target() else {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "rename: open a code file first",
            );
            return;
        };
        let local = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { buffer, .. },
                ..
            }) => local_identifier(buffer, caret),
            _ => None,
        };
        let Some(view) = self.tabs.get(self.active).map(|tab| tab.view) else {
            return;
        };
        let preparing = self.send(LspCommand::PrepareRename {
            doc,
            position: caret,
        });
        let (range, original, shown) = match local {
            Some((range, name)) => (range, name, true),
            None => (
                Range {
                    start: caret,
                    end: caret,
                },
                String::new(),
                false,
            ),
        };
        let mut edit = TextFieldState::default();
        edit.select_all(&original);
        self.rename = Some(RenamePrompt {
            doc,
            view,
            range,
            text: original.clone(),
            original,
            edit,
            preparing,
            touched: false,
            shown,
        });
    }

    /// Adopt the answer to the prompt's `PrepareRename`.
    pub(crate) fn on_rename_prepared(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        target: Option<RenameTarget>,
        reason: Option<String>,
    ) {
        let Some(prompt) = self.rename.as_mut() else {
            return;
        };
        if id.is_none() || prompt.preparing != id || prompt.doc != doc {
            return; // a superseded request; the live prompt stands
        }
        prompt.preparing = None;
        match target {
            Some(target) => {
                prompt.range = target.range;
                if !prompt.touched {
                    prompt.text.clone_from(&target.placeholder);
                    prompt.edit = TextFieldState::default();
                    prompt.edit.select_all(&prompt.text);
                }
                prompt.original = target.placeholder;
                prompt.shown = true;
            },
            None => {
                self.rename = None;
                let reason = reason.unwrap_or_else(|| "nothing to rename here".to_owned());
                self.notify(
                    Report::Refusal,
                    NotificationKind::Lsp,
                    format!("rename: {reason}"),
                );
            },
        }
    }

    /// Enter in the prompt: rename to what was typed.
    pub(crate) fn rename_submit(&mut self) {
        let Some(prompt) = self.rename.take() else {
            return;
        };
        let name = prompt.text.trim().to_owned();
        if name.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "rename: the new name is empty",
            );
            return;
        }
        if name == prompt.original {
            return; // nothing to rename
        }
        let id = self.send(LspCommand::Rename {
            doc: prompt.doc,
            position: prompt.range.start,
            new_name: name.clone(),
        });
        self.pending_rename = id.map(|id| PendingRename { id, name });
    }

    /// Esc in the prompt, or anything that moves the user away from it.
    pub(crate) fn rename_cancel(&mut self) {
        self.rename = None;
    }

    /// Close the prompt once the view or document it was opened over is no
    /// longer the one in front.
    pub(crate) fn reconcile_rename(&mut self) {
        let Some(prompt) = &self.rename else {
            return;
        };
        let current = self.tabs.get(self.active);
        let same_view = current.is_some_and(|tab| tab.view == prompt.view);
        let same_doc = current.is_some_and(
            |tab| matches!(tab.kind, TabKind::Code { doc: Some(doc), .. } if doc == prompt.doc),
        );
        if !same_view || !same_doc {
            self.rename = None;
        }
    }

    /// A key the prompt's bindings left to its text field.
    pub(crate) fn rename_input(&mut self, key: KeyEvent) {
        let Some(prompt) = self.rename.as_mut() else {
            return;
        };
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let word = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let RenamePrompt {
            text,
            edit,
            touched,
            ..
        } = prompt;
        match key.code {
            KeyCode::Backspace => {
                edit.backspace(text, word);
                *touched = true;
            },
            KeyCode::Delete => {
                edit.delete(text, word);
                *touched = true;
            },
            KeyCode::Left if word => edit.move_word_left(text, shift),
            KeyCode::Right if word => edit.move_word_right(text, shift),
            KeyCode::Left => edit.move_left(text, shift),
            KeyCode::Right => edit.move_right(text, shift),
            KeyCode::Home => edit.move_start(text, false, shift),
            KeyCode::End => edit.move_end(text, false, shift),
            KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                edit.select_all(text);
            },
            KeyCode::Char(c) if !word => {
                edit.insert(text, c.encode_utf8(&mut [0; 4]));
                *touched = true;
            },
            _ => {},
        }
    }

    /// Text pasted while the prompt has focus. A name is one line, so only
    /// the first line of a multi-line paste is taken.
    pub(crate) fn rename_paste(&mut self, pasted: &str) {
        if let Some(prompt) = self.rename.as_mut() {
            let line = pasted.lines().next().unwrap_or_default();
            prompt.edit.insert(&mut prompt.text, line);
            prompt.touched = true;
        }
    }

    /// Adopt a workspace edit the backend applied (or refused): keep the tabs
    /// in step with what it moved and opened, and report a rename's outcome.
    pub(crate) fn on_workspace_edit(
        &mut self,
        id: Option<RequestId>,
        applied: AppliedWorkspaceEdit,
    ) {
        self.adopt_workspace_edit_tabs(&applied);
        if self.settle_code_action(id, &applied) || self.settle_code_lens_run(id, &applied) {
            return;
        }
        let ours = match (&self.pending_rename, id) {
            (Some(pending), Some(id)) if pending.id == id => self.pending_rename.take(),
            _ => None,
        };
        let reason = applied
            .result
            .failure_reason
            .clone()
            .unwrap_or_else(|| "the edit was not applied".to_owned());
        match ours {
            Some(pending) if applied.result.applied => {
                let files = applied.edited.len();
                let noun = if files == 1 { "file" } else { "files" };
                self.notify(
                    Report::Outcome,
                    NotificationKind::Lsp,
                    format!("renamed to {} in {files} {noun}", pending.name),
                );
            },
            Some(_) => self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!("rename: {reason}"),
            ),
            // A language server's own edit says nothing when it works -- the
            // change is on screen -- but must not fail silently.
            None if id.is_none() && !applied.result.applied => {
                let label = applied.label.as_deref().unwrap_or("language server edit");
                self.notify(
                    Report::Failure,
                    NotificationKind::Lsp,
                    format!("{label}: {reason}"),
                );
            },
            None => {},
        }
    }

    /// Retarget tabs over moved files, and give every document the backend
    /// opened to edit it a tab of its own, in the background.
    ///
    /// The backend holds one reference to each opened document on our behalf.
    /// A new tab takes its own through the ordinary open, and that reference
    /// is then released, so the tab is left owning exactly one -- the same
    /// bookkeeping as any other tab.
    ///
    /// Only a code tab holds a document reference: a preview, placeholder, or
    /// byte view over the same path does not, and neither does the tab a path
    /// would ordinarily open as when it is too large, a notebook, or binary.
    /// Releasing the backend's reference then would close the edited document
    /// and discard its change, so such a document is given a code tab of its
    /// own.
    fn adopt_workspace_edit_tabs(&mut self, applied: &AppliedWorkspaceEdit) {
        for (from, to) in &applied.renamed {
            for tab in self.all_tabs_mut() {
                let moved = tab.path().and_then(|path| rebase_path(path, from, to));
                if let Some(path) = moved {
                    tab.retarget(&path);
                }
            }
        }
        for (doc, path) in &applied.opened {
            let target = canonical(path);
            let shown = self.all_tabs().any(|tab| {
                matches!(tab.kind, TabKind::Code { .. })
                    && tab.path().is_some_and(|p| canonical(p) == target)
            });
            if !shown {
                let mut tab = Tab::open(path);
                if !matches!(tab.kind, TabKind::Code { .. }) {
                    tab = Tab::open_as_text(path);
                }
                tab.view = self.alloc_view();
                self.tabs.push(tab);
                self.register_doc(self.tabs.len() - 1);
            }
            self.send_command(DocumentCommand::Close { doc: *doc });
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn local_identifier_takes_the_word_at_or_just_before_the_caret() {
        let buffer = TextBuffer::from_text("let some_value = 1;\n");
        let span = |start, end| Range {
            start: LineCol::new(0, start),
            end: LineCol::new(0, end),
        };
        for col in [4, 8, 14] {
            assert_eq!(
                local_identifier(&buffer, LineCol::new(0, col)),
                Some((span(4, 14), "some_value".to_owned())),
                "caret at {col}"
            );
        }
        // Between `=` and a space: no name to rename.
        assert_eq!(local_identifier(&buffer, LineCol::new(0, 16)), None);
        assert_eq!(local_identifier(&buffer, LineCol::new(9, 0)), None);
    }
}
