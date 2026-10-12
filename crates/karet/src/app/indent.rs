//! Application wiring for Tab / Shift+Tab in the editor: an inline macro first,
//! then the pure indent / dedent from `karet_editor::editing` over every
//! selection at once, with the document's resolved indentation.

use super::*;

impl App {
    /// Tab in the editor: expand a Tab-triggered inline macro when one applies at
    /// every caret, otherwise indent. (An open completion popup has already taken
    /// Tab before the keymap runs.) An editor showing no editable text (the
    /// welcome screen, an image, a preview) keeps Tab's focus toggle, so focus is
    /// never stranded where Tab does nothing.
    pub(super) fn indent_or_toggle_focus(&mut self) {
        if !self.active_tab_is_editable_code() {
            self.toggle_focus();
            return;
        }
        if !self.try_inline_macro(karet_syntax::InlineMacroTrigger::Tab) {
            self.reindent(false);
        }
    }

    /// The palette's "Indent Line": indent the active code tab, and do nothing
    /// when no editable code document is in front. Unlike the Tab key it never
    /// toggles focus and never expands an inline macro — the command names an
    /// indent, so that is all it does.
    pub(super) fn indent_lines(&mut self) {
        if self.active_tab_is_editable_code() {
            self.reindent(false);
        }
    }

    /// Whether the active tab is a code tab whose document has opened.
    fn active_tab_is_editable_code(&self) -> bool {
        matches!(
            self.tabs.get(self.active).map(|tab| &tab.kind),
            Some(TabKind::Code { doc: Some(_), .. })
        )
    }

    /// Indent (or with `outdent`, dedent) every selection and caret of the active
    /// code tab as one edit, keeping the selections, with the document's resolved
    /// indentation (EditorConfig, then language settings).
    pub(super) fn reindent(&mut self, outdent: bool) {
        let indentation = self.active_indentation();
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let tab_width = u32::from(self.tab_width_for(tab));
        let TabKind::Code {
            buffer,
            next_version,
            ..
        } = &tab.kind
        else {
            return;
        };
        let (cursors, base) = (tab.editor.cursors(), *next_version);
        let edit = if outdent {
            editing::dedent(buffer, cursors, base, &indentation, tab_width)
        } else {
            editing::indent(buffer, cursors, base, &indentation, tab_width)
        };
        if let Some(edit) = edit {
            self.submit_multi_edit(EditCause::Replace, edit);
        }
    }
}
