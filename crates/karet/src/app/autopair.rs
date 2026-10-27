//! Application wiring for auto-closing brackets and quotes: the active code
//! tab's language pairs (`karet_filetype`) and `editor.autoClosing*` settings
//! handed to `karet_editor::editing::autopair`, and the record of which closers
//! the editor inserted, so only those are typed over.

use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_editor::editing;
use karet_editor::editing::autopair;
use karet_editor::editing::autopair::AutoClose;
use karet_editor::editing::autopair::AutoPairs;
use karet_filetype::file_type_for_path;
use karet_session::ViewId;
use karet_session::config::schema::AutoClosing;
use karet_text::EditCause;
use karet_text::TextBuffer;

use super::indent::SyntaxIndent;
use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

/// The closers the last keystrokes auto-inserted in one view.
///
/// They stay open to type-over only while nothing else has happened there: the
/// record names the document version and cursor set the keystroke left, so a
/// caret moved away, an undo, or an edit from anywhere else retires it.
#[derive(Clone, Debug)]
pub(super) struct AutoClosers {
    view: ViewId,
    version: u64,
    cursors: CursorState,
    positions: Vec<LineCol>,
}

/// The engine's reading of a setting.
fn auto_close(setting: AutoClosing) -> AutoClose {
    match setting {
        AutoClosing::Always => AutoClose::Always,
        AutoClosing::LanguageDefined => AutoClose::LanguageDefined,
        AutoClosing::Never => AutoClose::Never,
    }
}

impl App {
    /// The active code tab's pairs, settings and highlights, with its buffer,
    /// cursors, base version and view. `None` without an editable code tab.
    fn auto_pair_context(&self) -> Option<(AutoPairs<'_>, &TextBuffer, &CursorState, u64, ViewId)> {
        self.backend.as_ref()?;
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            path,
            doc: Some(_),
            buffer,
            highlights,
            next_version,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let settings = self.settings.editor.for_language(tab.language_id());
        let rules = AutoPairs {
            pairs: file_type_for_path(path).auto_pairs(),
            brackets: auto_close(settings.auto_closing_brackets()),
            quotes: auto_close(settings.auto_closing_quotes()),
            highlights: Some(highlights),
        };
        Some((rules, buffer, tab.editor.cursors(), *next_version, tab.view))
    }

    /// The closers still open to type-over in the active view at `version`
    /// with `cursors`.
    fn live_closers(&self, view: ViewId, version: u64, cursors: &CursorState) -> Vec<LineCol> {
        match &self.auto_closers {
            Some(c) if c.view == view && c.version == version && c.cursors == *cursors => {
                c.positions.clone()
            },
            _ => Vec::new(),
        }
    }

    /// Type `c` with auto-closing: insert a pair, wrap a selection, step over
    /// an auto-inserted closer, or snap a closing bracket to its opener's
    /// indentation. `false` when none applies at any caret, leaving `c` to the
    /// ordinary insert.
    pub(super) fn type_auto_paired(&mut self, c: char) -> bool {
        let Some((rules, buffer, cursors, base, view)) = self.auto_pair_context() else {
            return false;
        };
        let closers = self.live_closers(view, base, cursors);
        let Some(typed) = autopair::type_char(buffer, cursors, base, c, &rules, &closers) else {
            return false;
        };
        match typed.edit {
            Some(edit) => {
                self.submit_multi_edit(EditCause::Type, edit);
                let landed = matches!(
                    self.tabs.get(self.active).map(|tab| &tab.kind),
                    Some(TabKind::Code { next_version, .. }) if *next_version != base
                );
                if !landed {
                    // The buffer refused the batch: no closer was inserted.
                    self.auto_closers = None;
                    return true;
                }
            },
            None => {
                if let Some(Tab {
                    kind: TabKind::Code { buffer, .. },
                    editor,
                    ..
                }) = self.tabs.get_mut(self.active)
                {
                    editor.set_cursor_state(buffer, typed.cursors);
                    let head = editor.cursor();
                    editor.reveal(buffer, head);
                }
            },
        }
        self.remember_closers(view, typed.closers);
        true
    }

    /// Backspace that deletes both halves of an empty pair a caret sits in.
    /// `false` when no caret does, leaving the ordinary Backspace.
    pub(super) fn backspace_auto_paired(&mut self) -> bool {
        let Some((rules, buffer, cursors, base, _)) = self.auto_pair_context() else {
            return false;
        };
        let Some(edit) = autopair::backspace(buffer, cursors, base, &rules) else {
            return false;
        };
        self.submit_multi_edit(EditCause::Delete, edit);
        true
    }

    /// Enter that opens an indented line between the halves of a bracket pair,
    /// with every other caret's newline indented by `syntax`. `false` when no
    /// caret sits between one, leaving the ordinary Enter.
    pub(super) fn newline_auto_paired(&mut self, syntax: Option<&SyntaxIndent>) -> bool {
        let indentation = self.active_indentation();
        let Some((rules, buffer, cursors, base, _)) = self.auto_pair_context() else {
            return false;
        };
        let indent = |range: Range| syntax.and_then(|s| s.for_range(buffer, range));
        let Some(edit) =
            autopair::newline_indented(buffer, cursors, base, &rules, &indentation, &indent)
        else {
            return false;
        };
        self.submit_multi_edit(EditCause::Newline, edit);
        true
    }
}

impl App {
    /// Record `positions` as the active view's open closers, at its current
    /// version and cursor set (or forget them all when there are none).
    fn remember_closers(&mut self, view: ViewId, positions: Vec<LineCol>) {
        self.auto_closers = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { next_version, .. },
                editor,
                ..
            }) if !positions.is_empty() => Some(AutoClosers {
                view,
                version: *next_version,
                cursors: editor.cursors().clone(),
                positions,
            }),
            _ => None,
        };
    }

    /// Carry the open closers through an edit applied to `view`, which took
    /// its document from `base` to `version` and its cursors from `before` to
    /// `after`: they move with the text when the record described the state
    /// the edit started from, and are forgotten otherwise. A closer inside an
    /// edit's range is gone from the text, so it is dropped rather than moved
    /// to where the deletion starts, onto a character the editor never typed.
    pub(super) fn carry_auto_closers(
        &mut self,
        view: ViewId,
        (base, version): (u64, u64),
        (before, after): (&CursorState, &CursorState),
        edits: &[TextEdit],
    ) {
        let Some(closers) = self.auto_closers.as_mut() else {
            return;
        };
        if closers.view != view {
            return;
        }
        if closers.version != base || closers.cursors != *before {
            self.auto_closers = None;
            return;
        }
        closers.positions.retain(|position| {
            !edits
                .iter()
                .any(|e| e.range.start <= *position && *position < e.range.end)
        });
        if closers.positions.is_empty() {
            self.auto_closers = None;
            return;
        }
        for position in &mut closers.positions {
            *position = editing::map_position(*position, edits);
        }
        closers.version = version;
        closers.cursors = after.clone();
    }
}
