//! Application wiring for indentation: Tab / Shift+Tab in the editor (an inline
//! macro first, then the pure indent / dedent from `karet_editor::editing` over
//! every selection at once, with the document's resolved indentation), and the
//! syntax-aware indentation a line break opens with, which Enter and a
//! multi-line paste consult.

use std::path::Path;
use std::sync::Arc;

use karet_core::EditCause;
use karet_core::Range;
use karet_editor::editing;
use karet_text::TextBuffer;

use super::syntax_select::ParsedText;
use crate::app::App;
use crate::tab::Tab;
use crate::tab::TabKind;

/// One parse of a buffer, with the indent queries of its languages, answering
/// what indentation a line break over a range of it opens with.
pub(super) struct SyntaxIndent {
    parsed: Arc<ParsedText>,
    indenter: karet_syntax::Indenter,
    unit: String,
}

impl SyntaxIndent {
    /// Whether `path`'s language has an indent query to consult, or injects a
    /// language that might.
    fn applies(path: &Path) -> bool {
        karet_treesitter::language_id_from_path(path).is_some_and(|lang| {
            karet_treesitter::indents_query(lang).is_some()
                || karet_treesitter::injections_query(lang).is_some()
        })
    }

    /// Answer line breaks over `parsed`, with `unit` as one level.
    fn new(parsed: Arc<ParsedText>, unit: String) -> Self {
        let indenter = karet_syntax::Indenter::for_tree(&parsed.tree);
        Self {
            parsed,
            indenter,
            unit,
        }
    }

    /// The new line's indentation for a break replacing `range` of `buffer`
    /// (the buffer this was parsed from), or `None` to copy the current line.
    pub(super) fn for_range(&self, buffer: &TextBuffer, range: Range) -> Option<String> {
        let before = buffer.line_col_to_byte(range.start).ok()?;
        let after = buffer.line_col_to_byte(range.end).ok()?;
        self.indenter.newline_indent(
            &self.parsed.tree,
            &self.parsed.text,
            (before, after),
            &self.unit,
        )
    }
}

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

    /// The syntax-aware indentation for line breaks in the active code tab, or
    /// `None` when its language has no indent query to consult (nor injects
    /// one that might), so Enter copies the current line's indentation.
    pub(super) fn syntax_indent(&self) -> Option<SyntaxIndent> {
        let unit = self.active_indentation();
        let Some(Tab {
            kind: TabKind::Code {
                path, doc: Some(_), ..
            },
            ..
        }) = self.tabs.get(self.active)
        else {
            return None;
        };
        if !SyntaxIndent::applies(path) {
            return None;
        }
        Some(SyntaxIndent::new(self.layered_tree()?, unit))
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
