use super::*;

const SPELLCHECK_SOURCE: &str = "karet-spell";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SpellWarning {
    pub(super) doc: DocumentId,
    pub(super) range: Range,
    pub(super) word: String,
    pub(super) suggestions: Vec<String>,
}

impl App {
    /// Resolve the spell warning covering `position` in the active editor.
    pub(super) fn spell_warning_at(&self, position: LineCol) -> Option<SpellWarning> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            text,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let diagnostic = self
            .document_diagnostics
            .get(doc)?
            .iter()
            .find(|diagnostic| {
                diagnostic.source.as_deref() == Some(SPELLCHECK_SOURCE)
                    && range_contains(diagnostic.range, position)
            })?;
        Some(SpellWarning {
            doc: *doc,
            range: diagnostic.range,
            word: selection_text(buffer, text, diagnostic.range)?,
            suggestions: suggestions_from_message(&diagnostic.message),
        })
    }

    /// Open the correction menu for a spell warning under `position`.
    pub(super) fn open_spelling_menu(&mut self, x: u16, y: u16, position: LineCol) -> bool {
        let Some(warning) = self.spell_warning_at(position) else {
            return false;
        };
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.set_selection(buffer, warning.range.start, warning.range.end);
        }

        let mut entries: Vec<ContextMenuEntry> = warning
            .suggestions
            .iter()
            .map(|suggestion| {
                ContextMenuEntry::custom(
                    format!("Replace with “{suggestion}”"),
                    ContextMenuAction::ReplaceSpelling {
                        doc: warning.doc,
                        range: warning.range,
                        replacement: suggestion.clone(),
                    },
                )
            })
            .collect();
        if entries.is_empty() {
            entries.push(ContextMenuEntry::disabled_custom(
                "No similar words found",
                ContextMenuAction::AddSpellingToDictionary {
                    word: warning.word.clone(),
                },
                "The dictionary has no close matches",
            ));
        }
        entries.push(ContextMenuEntry::custom(
            format!("Add “{}” to Project Dictionary", warning.word),
            ContextMenuAction::AddSpellingToDictionary { word: warning.word },
        ));
        self.context_menu = Some(ContextMenu::new(x, y, entries));
        true
    }

    /// Apply a selected spelling correction through the ordinary atomic edit path.
    pub(super) fn replace_spelling(&mut self, doc: DocumentId, range: Range, replacement: String) {
        if self.active_code_doc() != Some(doc) {
            return;
        }
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            editor.set_selection(buffer, range.start, range.end);
        }
        self.submit_edit_with_cause(
            EditCause::Replace,
            move |_caret, selection, _buffer, base| {
                (selection == Some(range)).then(|| editing::Edit {
                    change: Change::new(
                        base,
                        vec![TextEdit {
                            range,
                            new_text: replacement.clone(),
                        }],
                    ),
                    caret: crate::completion::caret_after_insert(range.start, &replacement),
                })
            },
        );
    }

    /// Add a spelling word directly to an existing project file, or require typed
    /// confirmation before creating the missing `.karet/setting.jsonc` tree.
    pub(super) fn add_spelling_to_dictionary(&mut self, word: String) {
        match karet_session::config::add_project_dictionary_word(
            std::slice::from_ref(&self.root),
            &word,
            false,
        ) {
            Ok(path) => self.dictionary_word_added(&word, &path),
            Err(karet_session::config::ConfigWriteError::ProjectCreationRequiresConfirmation(
                path,
            )) => {
                self.overlay = Some(Overlay::text(
                    format!("Type create to add “{word}” and create {}", path.display()),
                    TextPurpose::ConfirmCreateProjectSettings { word, path },
                ));
            },
            Err(error) => self.notify(
                Severity::Error,
                NotificationKind::System,
                format!("dictionary: {error}"),
            ),
        }
    }

    /// Finish the explicitly confirmed missing-project-settings path.
    pub(super) fn create_project_dictionary(&mut self, word: &str, expected_path: &Path) {
        match karet_session::config::add_project_dictionary_word(
            std::slice::from_ref(&self.root),
            word,
            true,
        ) {
            Ok(path) if path == expected_path => self.dictionary_word_added(word, &path),
            Ok(path) => self.notify(
                Severity::Error,
                NotificationKind::System,
                format!(
                    "dictionary: project settings target changed from {} to {}",
                    expected_path.display(),
                    path.display()
                ),
            ),
            Err(error) => self.notify(
                Severity::Error,
                NotificationKind::System,
                format!("dictionary: {error}"),
            ),
        }
    }

    fn dictionary_word_added(&mut self, word: &str, path: &Path) {
        if !self
            .settings
            .spellcheck
            .words
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(word))
        {
            self.settings.spellcheck.words.push(word.to_string());
        }
        self.loaded_config.settings.spellcheck.words = self.settings.spellcheck.words.clone();
        self.status = Some(format!("Added “{word}” to {}", path.display()));
    }
}

fn range_contains(range: Range, position: LineCol) -> bool {
    range.start <= position && position < range.end
}

fn suggestions_from_message(message: &str) -> Vec<String> {
    message
        .split_once("; try ")
        .map(|(_, suggestions)| {
            suggestions
                .split(", ")
                .map(str::trim)
                .filter(|suggestion| !suggestion.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_messages_decode_without_treating_an_unknown_word_as_a_match() {
        assert_eq!(
            suggestions_from_message("Unknown word “recieve”; try receive, receiver"),
            vec!["receive", "receiver"]
        );
        assert!(suggestions_from_message("Unknown word “Karet”").is_empty());
    }

    #[test]
    fn warning_ranges_are_end_exclusive() {
        let range = Range {
            start: LineCol::new(2, 4),
            end: LineCol::new(2, 8),
        };
        assert!(range_contains(range, LineCol::new(2, 4)));
        assert!(range_contains(range, LineCol::new(2, 7)));
        assert!(!range_contains(range, LineCol::new(2, 8)));
    }
}
