use super::*;

impl App {
    /// The active code tab's completion target: `(document, caret)`.
    pub(super) fn completion_target(&self) -> Option<(DocumentId, LineCol)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { doc: Some(doc), .. } = &tab.kind else {
            return None;
        };
        Some((*doc, tab.editor.cursor()))
    }

    /// Request completions at the caret. `manual` (Ctrl+Space) bypasses the
    /// syntax-error gate; automatic triggers hold off while the caret's line
    /// has an outright parse error (per issue #57).
    pub(crate) fn trigger_completion(&mut self, manual: bool) {
        let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .completion()
                .enabled()
        });
        if !completion_enabled {
            return;
        }
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let Some((doc, caret)) = self.completion_target() else {
            return;
        };
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code {
            buffer,
            syntax_errors,
            ..
        } = &tab.kind
        else {
            return;
        };
        if !manual && crate::completion::line_has_syntax_error(syntax_errors, caret.line) {
            return; // the line doesn't parse yet: suggesting now is noise
        }
        let (anchor, _) = karet_editor::word_bounds(buffer, caret);
        let id = backend.next_id();
        if backend
            .send(
                id,
                SessionCommand::Completion {
                    doc,
                    position: caret,
                },
            )
            .is_ok()
        {
            self.pending_completion =
                Some(crate::completion::PendingCompletion { id, doc, anchor });
        }
    }

    /// Auto-trigger after typing `c`: identifier characters open the popup (a
    /// one-character prefix suffices), `.` and the second `:` of `::`
    /// re-request at the new completion boundary, anything else does nothing.
    pub(super) fn maybe_auto_complete(&mut self, c: char) {
        let completion = self.tabs.get(self.active).map(|tab| {
            self.settings
                .editor
                .for_language(tab_language(tab))
                .completion()
        });
        if !completion.is_some_and(|completion| completion.enabled() && completion.auto_trigger()) {
            return;
        }
        let boundary = c == crate::completion::TRIGGER_DOT
            || (c == crate::completion::TRIGGER_COLON && self.typed_second_colon());
        if boundary {
            self.trigger_completion(false);
            return;
        }
        if self.completion.is_some() {
            return; // already open: typing narrows the filter client-side
        }
        if crate::completion::is_word_char(c) {
            self.trigger_completion(false);
        }
    }

    /// After typing `:`, whether it completed a `::` path separator.
    pub(super) fn typed_second_colon(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else {
            return false;
        };
        let TabKind::Code { buffer, .. } = &tab.kind else {
            return false;
        };
        let caret = tab.editor.cursor();
        buffer.line(caret.line as usize).is_some_and(|line| {
            let chars: Vec<char> = line.chars().collect();
            let i = caret.col as usize;
            i >= 2 && chars.get(i - 1) == Some(&':') && chars.get(i - 2) == Some(&':')
        })
    }

    /// The live filter: the text typed between the popup's anchor and the
    /// caret. `None` when the popup no longer applies to the active view.
    pub(crate) fn completion_filter(&self) -> Option<String> {
        let ui = self.completion.as_ref()?;
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        if *doc != ui.doc {
            return None;
        }
        let caret = tab.editor.cursor();
        if !crate::completion::caret_still_anchored(ui.anchor, caret) {
            return None;
        }
        let line = buffer.line(ui.anchor.line as usize)?;
        let chars: Vec<char> = line.chars().collect();
        let start = ui.anchor.col as usize;
        let end = (caret.col as usize).min(chars.len());
        (start <= end).then(|| chars[start..end].iter().collect())
    }

    /// The popup's current candidate order (indices into its items), resetting
    /// the selection when the filter changed since the last look.
    pub(crate) fn completion_ranked(&mut self) -> Option<Vec<usize>> {
        let filter = self.completion_filter()?;
        let ui = self.completion.as_mut()?;
        if filter != ui.last_filter {
            ui.list.reset();
            ui.last_filter.clone_from(&filter);
        }
        let mut popup = karet_widgets::CompletionPopup::new(
            &ui.items,
            &mut self.completion_matcher,
            &filter,
            &self.theme,
        );
        Some(popup.ranked())
    }

    /// Handle a key while the popup is open; returns whether it was consumed.
    /// Up/Down navigate, Enter/Tab accept, Esc dismisses; everything else
    /// falls through to normal editing (which refilters).
    pub(super) fn completion_key(&mut self, key: KeyEvent) -> bool {
        if self.completion.is_none() || !key.modifiers.is_empty() {
            return false;
        }
        let len = self.completion_ranked().map_or(0, |ranked| ranked.len());
        if len == 0 {
            // Nothing matches the typed prefix any more: the popup is over.
            self.dismiss_completion();
            return false;
        }
        match key.code {
            KeyCode::Up => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.list.select_prev(len);
                }
                true
            },
            KeyCode::Down => {
                if let Some(ui) = self.completion.as_mut() {
                    ui.list.select_next(len);
                }
                true
            },
            KeyCode::Esc => {
                self.dismiss_completion();
                true
            },
            KeyCode::Enter | KeyCode::Tab => {
                self.accept_completion();
                true
            },
            _ => false,
        }
    }

    /// Accept the selected candidate: replace the typed prefix (anchor to
    /// caret) with the item's resolved insert text, through the ordinary
    /// session edit path. (The item's `insert_text` already carries its
    /// `textEdit.newText` per the LSP precedence applied in karet-lsp.)
    pub(super) fn accept_completion(&mut self) {
        let Some(ranked) = self.completion_ranked() else {
            self.dismiss_completion();
            return;
        };
        let text = {
            let Some(ui) = self.completion.as_ref() else {
                return;
            };
            let selected = ui.list.selected.min(ranked.len().saturating_sub(1));
            let Some(item) = ranked.get(selected).and_then(|&i| ui.items.get(i)) else {
                self.dismiss_completion();
                return;
            };
            item.insert_text.clone()
        };
        let Some(anchor) = self.completion.as_ref().map(|ui| ui.anchor) else {
            return;
        };
        self.dismiss_completion();
        self.submit_edit_with_cause(EditCause::Replace, move |caret, _sel, _buf, base| {
            // Only carets still on the anchored span complete; others no-op.
            let range = crate::completion::accept_range(anchor, caret)?;
            Some(editing::Edit {
                change: Change::new(
                    base,
                    vec![TextEdit {
                        range,
                        new_text: text.clone(),
                    }],
                ),
                caret: crate::completion::caret_after_insert(range.start, &text),
            })
        });
    }

    /// Close the popup and forget any in-flight request.
    pub(crate) fn dismiss_completion(&mut self) {
        self.completion = None;
        self.pending_completion = None;
    }

    /// Drop the popup / pending request when the caret left the anchored span,
    /// the document changed, or the active tab is no longer a code tab.
    pub(crate) fn reconcile_completion(&mut self) {
        let target = self.completion_target();
        let anchored = |doc: DocumentId, anchor: LineCol| {
            matches!(target, Some((d, caret))
                if d == doc && crate::completion::caret_still_anchored(anchor, caret))
        };
        if let Some(pending) = &self.pending_completion
            && !anchored(pending.doc, pending.anchor)
        {
            self.pending_completion = None;
        }
        if let Some(ui) = &self.completion
            && !anchored(ui.doc, ui.anchor)
        {
            self.completion = None;
        }
    }

    /// Adopt (or drop as stale) an answering `Event::Completions`.
    pub(super) fn on_completions(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        items: Vec<karet_core::CompletionItem>,
    ) {
        // The request id is the primary staleness key (a newer request
        // supersedes); the anchor check below covers caret movement, which
        // also subsumes the version tag for typed-ahead edits.
        let _ = version;
        let Some(pending) = self.pending_completion else {
            return;
        };
        if id != Some(pending.id) {
            return; // an answer to a superseded request
        }
        self.pending_completion = None;
        if pending.doc != doc {
            return;
        }
        let still_valid = matches!(self.completion_target(), Some((d, caret))
            if d == doc && crate::completion::caret_still_anchored(pending.anchor, caret));
        if !still_valid {
            return;
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(crate::completion::CompletionUi {
            items,
            list: karet_widgets::CompletionState::default(),
            doc,
            anchor: pending.anchor,
            last_filter: String::new(),
        });
        // Seed the filter so the first render doesn't spuriously reset it.
        if let Some(filter) = self.completion_filter()
            && let Some(ui) = self.completion.as_mut()
        {
            ui.last_filter = filter;
        }
    }
}
