//! Application wiring for parser-backed inline editing macros.

use super::*;

impl App {
    /// Resolve the active tab's configured inline macro for every caret and submit
    /// all expansions as one atomic edit. Mixed matching/non-matching cursor sets
    /// fall back to the ordinary key behavior instead of editing only some carets.
    pub(super) fn try_inline_macro(&mut self, trigger: karet_syntax::InlineMacroTrigger) -> bool {
        if !matches!(
            trigger,
            karet_syntax::InlineMacroTrigger::Character('[' | '<')
                | karet_syntax::InlineMacroTrigger::Tab
        ) {
            return false;
        }
        let indentation = self.active_indentation();
        let (language, text, selections) = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { path, text, .. },
                editor,
                ..
            }) => {
                let Some(language) = karet_treesitter::language_id_from_path(path) else {
                    return false;
                };
                if matches!(trigger, karet_syntax::InlineMacroTrigger::Character(_))
                    && editor
                        .cursors()
                        .selections
                        .iter()
                        .all(|selection| selection.is_empty())
                {
                    return false;
                }
                (language, text.clone(), editor.cursors().selections.clone())
            },
            _ => return false,
        };
        let expansions = selections
            .iter()
            .map(|selection| {
                self.inline_macro_engine
                    .expand(language, &text, selection.range(), trigger, &indentation)
                    .map(|expansion| (selection.range(), expansion))
            })
            .collect::<Option<Vec<_>>>();
        let Some(expansions) = expansions.filter(|expansions| !expansions.is_empty()) else {
            return false;
        };

        self.submit_edit_with_cause(
            EditCause::Replace,
            move |caret, selection, _buffer, base| {
                let range = selection.unwrap_or(Range {
                    start: caret,
                    end: caret,
                });
                let (_, expansion) = expansions.iter().find(|(input, _)| *input == range)?;
                Some(editing::Edit {
                    change: Change::new(
                        base,
                        vec![TextEdit {
                            range: expansion.range,
                            new_text: expansion.new_text.clone(),
                        }],
                    ),
                    caret: expansion.caret,
                })
            },
        );
        true
    }
}
