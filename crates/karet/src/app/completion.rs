use std::collections::HashMap;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;
use karet_core::LineCol;
use karet_session::DocumentId;
use karet_session::LspCommand;
use karet_session::RequestId;

use crate::app::App;
use crate::tab::TabKind;

mod accept;
mod snippet;

/// Completion's app-side state, from the request to the last snippet stop.
#[derive(Default)]
pub(crate) struct Completion {
    /// The in-flight completion request, if any (see [`crate::completion`]).
    pub(crate) pending: Option<crate::completion::PendingCompletion>,
    /// The open completion popup, if any.
    pub(crate) popup: Option<crate::completion::CompletionUi>,
    /// The trigger and commit characters the server completing in each
    /// document last answered with there; dropped when the document closes.
    pub(crate) chars: HashMap<DocumentId, CompletionOptions>,
    /// An accepted item's additional edits, still on their way.
    pub(crate) follow_up: Option<crate::completion::AcceptFollowUp>,
    /// The snippet an accepted completion is still being filled in through,
    /// if any (see [`crate::completion::SnippetSession`]).
    pub(crate) snippet: Option<crate::completion::SnippetSession>,
    /// The reusable fuzzy matcher backing the popup's filtering.
    pub(crate) matcher: karet_fuzzy::Matcher,
}

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
        self.request_completion(manual, CompletionTrigger::Invoked);
    }

    /// [`Self::trigger_completion`], saying why through `trigger`.
    fn request_completion(&mut self, manual: bool, trigger: CompletionTrigger) {
        let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
            self.settings
                .editor
                .for_language(tab.language_id())
                .completion()
                .enabled()
        });
        if !completion_enabled {
            return;
        }
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
        if let Some(warning) = self.spell_warning_ending_at(doc, caret)
            && !warning.suggestions.is_empty()
        {
            self.open_spelling_completion(warning);
            return;
        }
        let Some(backend) = self.backend.clone() else {
            return;
        };
        // Narrowing an incomplete list keeps the popup's own anchor: the word
        // being completed has not changed, only how much of it is typed.
        let anchor = match (trigger, &self.completion.popup) {
            (CompletionTrigger::Incomplete, Some(ui)) => ui.anchor,
            _ => karet_text::words::word_bounds(buffer, caret).0,
        };
        let id = backend.next_id();
        let command = LspCommand::Completion {
            doc,
            position: caret,
            trigger,
        };
        if backend.send(id, command.into()).is_ok() {
            self.completion.pending = Some(crate::completion::PendingCompletion {
                id,
                doc,
                anchor,
                position: caret,
            });
        }
    }

    /// The trigger and commit characters of the server completing in `doc`,
    /// as its last answer there said; empty before any did.
    pub(crate) fn completion_characters(&self, doc: DocumentId) -> CompletionOptions {
        self.completion.chars.get(&doc).cloned().unwrap_or_default()
    }

    /// Auto-trigger after typing `c`: identifier characters open the popup (a
    /// one-character prefix suffices), the server's trigger characters (or,
    /// before it named any, `.` and the second `:` of `::`) re-request at the
    /// new completion boundary, anything else does nothing.
    pub(super) fn maybe_auto_complete(&mut self, c: char) {
        let completion = self.tabs.get(self.active).map(|tab| {
            self.settings
                .editor
                .for_language(tab.language_id())
                .completion()
        });
        if !completion.is_some_and(|completion| completion.enabled() && completion.auto_trigger()) {
            return;
        }
        let known = self
            .completion_target()
            .map(|(doc, _)| self.completion_characters(doc).trigger_characters)
            .unwrap_or_default();
        let second_colon = c == crate::completion::TRIGGER_COLON && self.typed_second_colon();
        if let Some(trigger) = crate::completion::trigger_for(c, &known, second_colon) {
            self.request_completion(false, trigger);
            return;
        }
        if self.completion.popup.is_some() {
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
        let ui = self.completion.popup.as_ref()?;
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
        if let crate::completion::CompletionMode::Spelling { caret: expected } = ui.mode {
            return (caret == expected).then(String::new);
        }
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
        let ui = self.completion.popup.as_mut()?;
        if filter != ui.last_filter {
            ui.list.reset();
            ui.last_filter.clone_from(&filter);
        }
        let mut popup = karet_widgets::CompletionPopup::new(
            &ui.items,
            &mut self.completion.matcher,
            &filter,
            &self.theme,
        );
        Some(popup.ranked())
    }

    /// Handle a key while the popup is open; returns whether it was consumed.
    /// Up/Down navigate, Enter/Tab accept (inserting over the word up to the
    /// caret), Shift+Enter/Shift+Tab accept replacing the rest of the word
    /// too, Esc dismisses; everything else falls through to normal editing
    /// (which refilters).
    ///
    /// Only while the owning editor has the keyboard: with the Explorer focused,
    /// Enter belongs to the Explorer, never to a popup over a document.
    pub(super) fn completion_key(&mut self, key: KeyEvent) -> bool {
        if self.completion.popup.is_none() || !self.editor_owns_keyboard() {
            return false;
        }
        let replace = matches!(
            (key.code, key.modifiers),
            (KeyCode::Enter, KeyModifiers::SHIFT)
                | (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
        );
        if !key.modifiers.is_empty() && !replace {
            return false;
        }
        if replace {
            if self
                .completion_ranked()
                .is_some_and(|ranked| !ranked.is_empty())
            {
                self.accept_completion_replacing();
                return true;
            }
            self.dismiss_completion();
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
                if let Some(ui) = self.completion.popup.as_mut() {
                    ui.list.select_prev(len);
                }
                true
            },
            KeyCode::Down => {
                if let Some(ui) = self.completion.popup.as_mut() {
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

    /// Close the popup and forget any in-flight request.
    pub(crate) fn dismiss_completion(&mut self) {
        self.completion.popup = None;
        self.completion.pending = None;
    }

    /// Drop the popup / pending request when the caret left the anchored span,
    /// the document changed, the active tab is no longer a code tab, or the
    /// editor gave up the keyboard (VS Code closes its suggest widget on blur).
    ///
    /// The signature popup is kept with the caret at the same moments, so
    /// every path that reconciles one reconciles both.
    pub(crate) fn reconcile_completion(&mut self) {
        self.reconcile_signature();
        if !self.editor_owns_keyboard() {
            self.dismiss_completion();
            return;
        }
        let target = self.completion_target();
        let anchored =
            |doc: DocumentId, anchor: LineCol, mode: crate::completion::CompletionMode| {
                matches!(target, Some((d, caret))
                if d == doc && match mode {
                    crate::completion::CompletionMode::Filtered =>
                        crate::completion::caret_still_anchored(anchor, caret),
                    crate::completion::CompletionMode::Spelling { caret: expected } =>
                        caret == expected,
                })
            };
        if let Some(pending) = &self.completion.pending
            && !anchored(
                pending.doc,
                pending.anchor,
                crate::completion::CompletionMode::Filtered,
            )
        {
            self.completion.pending = None;
        }
        if let Some(ui) = &self.completion.popup
            && !anchored(ui.doc, ui.anchor, ui.mode)
        {
            self.completion.popup = None;
        }
        self.requery_incomplete_completion();
        self.resolve_selected_completion();
    }

    /// Ask again over an incomplete list whose filter changed since it was
    /// asked for -- typed further or deleted back -- unless a request is
    /// already on its way.
    fn requery_incomplete_completion(&mut self) {
        if self.completion.pending.is_some() {
            return;
        }
        let Some(ui) = self.completion.popup.as_ref() else {
            return;
        };
        if !ui.incomplete || ui.mode != crate::completion::CompletionMode::Filtered {
            return;
        }
        let queried = ui.queried_filter.clone();
        if self
            .completion_filter()
            .is_some_and(|filter| filter != queried)
        {
            self.request_completion(false, CompletionTrigger::Incomplete);
        }
    }

    /// Adopt (or drop as stale) an answering `Event::Completions`: open the
    /// popup, or replace an open one's candidates when it asked again over an
    /// incomplete list, keeping what the server said about its characters.
    pub(super) fn on_completions(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        (items, incomplete, options): (Vec<karet_core::CompletionItem>, bool, CompletionOptions),
    ) {
        // The request id is the primary staleness key (a newer request
        // supersedes); the anchor check below covers caret movement, which
        // also subsumes the version tag for typed-ahead edits.
        let _ = version;
        let Some(pending) = self.completion.pending else {
            return;
        };
        if id != Some(pending.id) {
            return; // an answer to a superseded request
        }
        self.completion.pending = None;
        if pending.doc != doc {
            return;
        }
        if !options.trigger_characters.is_empty() || !options.commit_characters.is_empty() {
            self.completion.chars.insert(doc, options);
        }
        let still_valid = matches!(self.completion_target(), Some((d, caret))
            if d == doc && crate::completion::caret_still_anchored(pending.anchor, caret));
        if !still_valid {
            return;
        }
        if items.is_empty() && !incomplete {
            self.completion.popup = None;
            return;
        }
        let mut ui = crate::completion::CompletionUi::new(
            doc,
            pending.anchor,
            pending.position,
            items,
            crate::completion::CompletionMode::Filtered,
        );
        ui.incomplete = incomplete;
        self.completion.popup = Some(ui);
        // Seed the filter so the first render doesn't spuriously reset it, and
        // remember what the items were asked for.
        if let Some(filter) = self.completion_filter()
            && let Some(ui) = self.completion.popup.as_mut()
        {
            ui.queried_filter.clone_from(&filter);
            ui.last_filter = filter;
        }
        self.resolve_selected_completion();
    }
}
