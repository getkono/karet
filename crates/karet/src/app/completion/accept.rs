//! Accepting a completion -- its edit range, its additional edits, its commit
//! characters -- and resolving what the server deferred for the selected
//! item.

use karet_core::CompletionItem;

use super::*;
use crate::completion::AcceptFollowUp;
use crate::completion::CompletionMode;

/// The characters of `line` in columns `[from, to)`, or `None` past its end.
fn columns(line: &str, from: u32, to: u32) -> Option<String> {
    let len = to.checked_sub(from)? as usize;
    let text: String = line.chars().skip(from as usize).take(len).collect();
    (text.chars().count() == len).then_some(text)
}

/// What an accepted completion replaces at one caret, given what it replaces
/// around the primary one: `before` (from the edit's start to the primary
/// caret) when the same text sits before this caret -- always so at the
/// primary -- stretched over `after` when that follows it too; else the
/// caret's selection, else nothing (an insertion at the caret).
fn caret_range(
    (before, after): (&str, &str),
    caret: LineCol,
    selection: Option<Range>,
    buffer: &TextBuffer,
) -> Range {
    let line = buffer.line(caret.line as usize).unwrap_or_default();
    let back = u32::try_from(before.chars().count()).unwrap_or(u32::MAX);
    let ahead = u32::try_from(after.chars().count()).unwrap_or(u32::MAX);
    let start = caret.col.checked_sub(back);
    let typed = start.and_then(|start| columns(&line, start, caret.col));
    let (Some(start), Some(typed)) = (start, typed) else {
        return selection.unwrap_or(Range {
            start: caret,
            end: caret,
        });
    };
    if typed != before {
        return selection.unwrap_or(Range {
            start: caret,
            end: caret,
        });
    }
    let end = caret.col.saturating_add(ahead);
    let follows = columns(&line, caret.col, end).is_some_and(|text| text == after);
    Range {
        start: LineCol::new(caret.line, start),
        end: if follows {
            LineCol::new(caret.line, end)
        } else {
            caret
        },
    }
}

impl App {
    /// Accept the selected candidate over the word up to the caret (the
    /// item's insert range), through the ordinary session edit path.
    pub(crate) fn accept_completion(&mut self) {
        self.accept_selected(false);
    }

    /// Accept the selected candidate over the whole word around the caret
    /// (the item's replace range, where it has one).
    pub(crate) fn accept_completion_replacing(&mut self) {
        self.accept_selected(true);
    }

    /// The selected item, the popup's anchor, and the caret its request was
    /// made at. `None` when the popup is closed or nothing matches.
    fn selected_completion(&mut self) -> Option<(CompletionItem, LineCol, LineCol)> {
        let ranked = self.completion_ranked()?;
        let ui = self.completion.popup.as_ref()?;
        let selected = ui.list.selected().min(ranked.len().saturating_sub(1));
        let item = ranked.get(selected).and_then(|&i| ui.items.get(i))?;
        Some((item.clone(), ui.anchor, ui.requested_at))
    }

    fn accept_selected(&mut self, replace: bool) {
        let selected = self.selected_completion();
        self.dismiss_completion();
        if let Some((item, anchor, requested_at)) = selected {
            self.apply_completion(&item, (anchor, requested_at), replace);
        }
    }

    /// Apply `item` at every caret, with its additional edits, as one change.
    ///
    /// At the primary caret it replaces its edit range (see
    /// [`crate::completion::edit_range`]); every other caret takes the item
    /// too, replacing the same text where it sits around that caret (VS Code's
    /// behaviour). Additional edits apply once. When they may still be
    /// deferred -- the item was never resolved and listed none -- they are
    /// asked for now and follow as their own change. A snippet item selects
    /// its first stop and starts a snippet session.
    fn apply_completion(
        &mut self,
        item: &CompletionItem,
        (anchor, requested_at): (LineCol, LineCol),
        replace: bool,
    ) {
        let Some((doc, caret)) = self.completion_target() else {
            return;
        };
        let Some(range) = crate::completion::edit_range(item, anchor, requested_at, caret, replace)
        else {
            return;
        };
        let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            ..
        }) = self.tabs.get(self.active)
        else {
            return;
        };
        let line = buffer.line(caret.line as usize).unwrap_or_default();
        let before = columns(&line, range.start.col, caret.col).unwrap_or_default();
        let after = columns(&line, caret.col, range.end.col).unwrap_or_default();
        let text = item
            .edit
            .as_ref()
            .map_or_else(|| item.insert_text.clone(), |edit| edit.new_text.clone());
        let inserted = text.clone();
        let count = |s: &str| u32::try_from(s.chars().count()).unwrap_or(u32::MAX);
        let (back, ahead) = (count(&before), count(&after));
        let Some(multi) = self.compose_edit(move |caret, sel, buf, base| {
            let range = caret_range((&before, &after), caret, sel, buf);
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
        }) else {
            return;
        };
        let applied = multi.change.edits.clone();
        let multi = crate::completion::with_additional_edits(multi, &item.additional_edits);
        let snippet = self.snippet_for(item, &inserted, &applied, &multi);
        self.submit_with_snippet(multi, snippet);
        // Modal editing's `.` and macros replay the text, not the key that
        // accepted it.
        self.vi_note_text(back, ahead, &inserted);
        // Eager additional edits are final (a resolve keeps them, as
        // `CompletionItem::adopt_resolved` does): only an item that listed
        // none may still have some deferred.
        if item.payload.is_some() && item.additional_edits.is_empty() {
            self.follow_up_completion(doc, item, applied);
        }
    }

    /// Ask for the additional edits an accepted, never-resolved `item` may
    /// still bring, to apply once they arrive.
    fn follow_up_completion(
        &mut self,
        doc: DocumentId,
        item: &CompletionItem,
        applied: Vec<TextEdit>,
    ) {
        let (Some(backend), Some(version)) = (self.backend.clone(), self.active_next_version())
        else {
            return;
        };
        let id = backend.next_id();
        let command = LspCommand::ResolveCompletion {
            doc,
            item: item.clone(),
        };
        if backend.send(id, command.into()).is_ok() {
            self.completion.follow_up = Some(AcceptFollowUp {
                id,
                doc,
                version,
                applied,
            });
        }
    }

    /// The active code tab's next document version.
    pub(super) fn active_next_version(&self) -> Option<u64> {
        match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { next_version, .. },
                ..
            }) => Some(*next_version),
            _ => None,
        }
    }

    /// Ask the server to fill in the selected item, once, if it deferred
    /// anything: its documentation shows beside the popup, and its additional
    /// edits are in hand by the time it is accepted.
    pub(super) fn resolve_selected_completion(&mut self) {
        let Some(ranked) = self.completion_ranked() else {
            return;
        };
        let Some(backend) = self.backend.clone() else {
            return;
        };
        let Some(ui) = self.completion.popup.as_mut() else {
            return;
        };
        let selected = ui.list.selected().min(ranked.len().saturating_sub(1));
        let Some(&index) = ranked.get(selected) else {
            return;
        };
        let Some(item) = ui.items.get(index) else {
            return;
        };
        if item.payload.is_none() || ui.resolving.iter().any(|&(_, i)| i == index) {
            return;
        }
        let id = backend.next_id();
        let command = LspCommand::ResolveCompletion {
            doc: ui.doc,
            item: item.clone(),
        };
        if backend.send(id, command.into()).is_ok() {
            ui.resolving.push((id, index));
        }
    }

    /// Adopt an answering `Event::CompletionResolved`: fill in the popup item
    /// it was asked for, or apply the additional edits an accept was waiting
    /// on. Anything else is stale and dropped.
    pub(crate) fn on_completion_resolved(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        resolved: Option<CompletionItem>,
    ) {
        if let Some(follow) = self
            .completion
            .follow_up
            .take_if(|follow| Some(follow.id) == id)
        {
            if let Some(resolved) = resolved {
                self.apply_follow_up(&follow, &resolved.additional_edits);
            }
            return;
        }
        let Some(ui) = self.completion.popup.as_mut() else {
            return;
        };
        let Some(at) = ui.resolving.iter().position(|&(r, _)| Some(r) == id) else {
            return;
        };
        let (_, index) = ui.resolving.remove(at);
        if ui.doc != doc {
            return;
        }
        if let Some(item) = ui.items.get_mut(index) {
            match resolved {
                Some(resolved) => item.adopt_resolved(resolved),
                None => item.payload = None,
            }
        }
    }

    /// Apply additional edits that arrived after their accept: moved past the
    /// accept's own edits, as their own change, the carets moved past them --
    /// but only while the document is exactly as the accept left it.
    fn apply_follow_up(&mut self, follow: &AcceptFollowUp, extra: &[TextEdit]) {
        let edits = crate::completion::after_applied(&follow.applied, extra);
        if edits.is_empty() {
            return;
        }
        let unchanged = self
            .completion_target()
            .is_some_and(|(doc, _)| doc == follow.doc)
            && self.active_next_version() == Some(follow.version);
        let Some(tab) = self.tabs.get(self.active).filter(|_| unchanged) else {
            return;
        };
        let mut cursors = tab.editor.cursors().clone();
        for selection in &mut cursors.selections {
            *selection = editing::map_selection(*selection, &edits);
        }
        let change = Change::new(follow.version, edits);
        self.submit_multi_edit(EditCause::Replace, editing::MultiEdit { change, cursors });
    }

    /// Accept the selected candidate when `c`, about to be typed, is one of
    /// its commit characters. Returns whether it did; the caller types `c`
    /// either way.
    pub(crate) fn commit_completion_on(&mut self, c: char) -> bool {
        if !self.editor_owns_keyboard()
            || self
                .completion
                .popup
                .as_ref()
                .is_none_or(|ui| ui.mode != CompletionMode::Filtered)
        {
            return false;
        }
        let Some((item, ..)) = self.selected_completion() else {
            return false;
        };
        let defaults = self
            .completion
            .popup
            .as_ref()
            .map(|ui| self.completion_characters(ui.doc).commit_characters)
            .unwrap_or_default();
        if !crate::completion::commits(&item, c, &defaults) {
            return false;
        }
        self.vi.committing = true;
        self.accept_completion();
        self.vi.committing = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::from_text(text)
    }

    /// Every other caret replaces the same text the primary does, around it:
    /// before it, and after it only where that text follows too.
    #[test]
    fn other_carets_replace_what_matches_around_them() {
        let buf = buffer("pu_x\npush\npa\n");
        // Primary replaced `pu` before and `_x` after.
        let parts = ("pu", "_x");
        assert_eq!(
            caret_range(parts, LineCol::new(0, 2), None, &buf),
            Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 4),
            }
        );
        // `pu` precedes, `_x` does not follow: only the prefix.
        assert_eq!(
            caret_range(parts, LineCol::new(1, 2), None, &buf),
            Range {
                start: LineCol::new(1, 0),
                end: LineCol::new(1, 2),
            }
        );
        // Nothing matches: the selection, else an insertion.
        let selection = Range {
            start: LineCol::new(2, 0),
            end: LineCol::new(2, 2),
        };
        assert_eq!(
            caret_range(parts, LineCol::new(2, 2), Some(selection), &buf),
            selection
        );
        assert_eq!(
            caret_range(parts, LineCol::new(2, 1), None, &buf),
            Range {
                start: LineCol::new(2, 1),
                end: LineCol::new(2, 1),
            }
        );
    }

    #[test]
    fn columns_count_characters_and_refuse_past_the_end() {
        assert_eq!(columns("a😀bc", 1, 3).as_deref(), Some("😀b"));
        assert_eq!(columns("ab", 1, 1).as_deref(), Some(""));
        assert_eq!(columns("ab", 1, 4), None);
        assert_eq!(columns("ab", 2, 1), None);
    }
}
