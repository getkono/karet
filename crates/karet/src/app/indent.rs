//! Application wiring for indentation: Tab / Shift+Tab in the editor (an inline
//! macro first, then the pure indent / dedent from `karet_editor::editing` over
//! every selection at once, with the document's resolved indentation), and the
//! syntax-aware indentation a line break opens with, which Enter and a
//! multi-line paste consult.

use super::*;

/// One parse of a buffer, with the indent queries of its languages, answering
/// what indentation a line break over a range of it opens with.
pub(super) struct SyntaxIndent {
    tree: karet_treesitter::LayeredTree,
    indenter: karet_syntax::Indenter,
    text: String,
    unit: String,
}

impl SyntaxIndent {
    /// Parse `buffer` as the language `path` names, with `unit` as one level.
    fn new(path: &Path, buffer: &TextBuffer, unit: String) -> Option<Self> {
        let lang = karet_treesitter::language_id_from_path(path)?;
        if karet_treesitter::indents_query(lang).is_none()
            && karet_treesitter::injections_query(lang).is_none()
        {
            return None;
        }
        let text = buffer.text();
        let tree = karet_treesitter::LayeredParser::new()
            .parse(lang, &text)
            .ok()?;
        let indenter = karet_syntax::Indenter::for_tree(&tree);
        Some(Self {
            tree,
            indenter,
            text,
            unit,
        })
    }

    /// The new line's indentation for a break replacing `range` of `buffer`
    /// (the buffer this was parsed from), or `None` to copy the current line.
    pub(super) fn for_range(&self, buffer: &TextBuffer, range: Range) -> Option<String> {
        let before = buffer.line_col_to_byte(range.start).ok()?;
        let after = buffer.line_col_to_byte(range.end).ok()?;
        self.indenter
            .newline_indent(&self.tree, &self.text, (before, after), &self.unit)
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
            kind:
                TabKind::Code {
                    path,
                    doc: Some(_),
                    buffer,
                    ..
                },
            ..
        }) = self.tabs.get(self.active)
        else {
            return None;
        };
        SyntaxIndent::new(path, buffer, unit)
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
