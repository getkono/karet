//! Running the snippet session an accepted snippet completion starts: placing
//! its first stop, following every edit, and Tab / Shift+Tab / Esc (see
//! [`crate::completion::SnippetSession`]).

use karet_core::CompletionItem;

use super::*;
use crate::completion::SnippetSession;

impl App {
    /// The session `item`'s snippet starts, inserted as `text` by the
    /// `applied` edits of `multi` (its final change, which may hold
    /// additional edits too), at version 0 until it lands. `None` for an item
    /// with no stops.
    pub(super) fn snippet_for(
        &self,
        item: &CompletionItem,
        text: &str,
        applied: &[TextEdit],
        multi: &editing::MultiEdit,
    ) -> Option<SnippetSession> {
        if item.tabstops.is_empty() {
            return None;
        }
        let view = self.tabs.get(self.active)?.view;
        let edits = &multi.change.edits;
        let starts: Vec<LineCol> = applied
            .iter()
            .filter(|edit| edit.new_text == text)
            .map(|edit| {
                let others: Vec<TextEdit> = edits
                    .iter()
                    .filter(|other| *other != edit)
                    .cloned()
                    .collect();
                editing::map_position(edit.range.start, &others)
            })
            .collect();
        SnippetSession::new(view, 0, &starts, text, &item.tabstops)
    }

    /// Submit an accept's `multi`, starting `session` (from
    /// [`Self::snippet_for`]) over it: the carets go to its first stop, and
    /// it stays live -- unless that stop is already the final one -- once the
    /// change lands, replacing any session before it. Without one, `multi`
    /// is an ordinary edit, which a live session follows: a plain item
    /// accepted inside a placeholder fills it in.
    pub(super) fn submit_with_snippet(
        &mut self,
        mut multi: editing::MultiEdit,
        session: Option<SnippetSession>,
    ) {
        let Some(mut session) = session else {
            self.submit_multi_edit(EditCause::Replace, multi);
            return;
        };
        self.completion.snippet = None;
        let primary = multi
            .cursors
            .selections
            .get(multi.cursors.primary)
            .map_or(LineCol::new(0, 0), |selection| selection.head);
        multi.cursors = session.cursors(primary);
        let before = self.active_next_version();
        self.submit_multi_edit(EditCause::Replace, multi);
        let after = self.active_next_version();
        if let Some(version) = after.filter(|_| after != before && !session.finished()) {
            session.version = version;
            self.completion.snippet = Some(session);
        }
    }

    /// Follow a change view `view` applied from version `base` to `version`;
    /// a change the session cannot follow ends it.
    pub(crate) fn carry_snippet(&mut self, view: ViewId, versions: (u64, u64), edits: &[TextEdit]) {
        let Some(session) = self.completion.snippet.as_mut() else {
            return;
        };
        if session.view == view && !session.carry(versions, edits) {
            self.completion.snippet = None;
        }
    }

    /// Whether the session is still about the active view as it stands: the
    /// same view, at the version the session last followed.
    fn snippet_live(&self) -> bool {
        let Some(session) = &self.completion.snippet else {
            return false;
        };
        self.tabs.get(self.active).is_some_and(|tab| {
            tab.view == session.view
                && matches!(
                    &tab.kind,
                    TabKind::Code { next_version, .. } if *next_version == session.version
                )
        })
    }

    /// The snippet session's keys: Tab to the next stop, Shift+Tab to the
    /// previous, Esc to leave it. Returns whether the key was consumed.
    ///
    /// A session that no longer matches the view (it changed in a way the
    /// session did not follow) ends without taking the key, and so does
    /// Tab or Shift+Tab from a caret outside the snippet, which then
    /// indents or dedents as usual.
    pub(crate) fn snippet_key(&mut self, key: KeyEvent) -> bool {
        if self.completion.snippet.is_none() || !self.editor_owns_keyboard() {
            return false;
        }
        let forward = match (key.code, key.modifiers) {
            (KeyCode::Tab, KeyModifiers::NONE) => true,
            (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
            | (KeyCode::Tab, KeyModifiers::SHIFT) => false,
            (KeyCode::Esc, KeyModifiers::NONE) => {
                let live = self.snippet_live();
                self.completion.snippet = None;
                return live;
            },
            _ => return false,
        };
        if !self.snippet_live() {
            self.completion.snippet = None;
            return false;
        }
        let Some(mut session) = self.completion.snippet.take() else {
            return false;
        };
        let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        else {
            return false;
        };
        let caret = editor.cursor();
        if !session.holds(caret) {
            return false;
        }
        session.step(forward);
        editor.set_cursor_state(buffer, session.cursors(caret));
        if !session.finished() {
            self.completion.snippet = Some(session);
        }
        true
    }
}
