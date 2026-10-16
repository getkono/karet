//! The multi-caret operations beyond adding one caret: Select All
//! Occurrences, Skip Occurrence, Add Cursors to Line Ends, and the history
//! Undo Last Cursor Operation walks back through.
//!
//! Every caret operation runs through [`EditorState::caret_op`], which
//! records the set it replaced. The history holds positions in the text as it
//! was, so it lasts only while the buffer is unedited: an entry recorded at
//! another buffer version is dropped rather than restored.

use super::*;

/// How many caret operations [`EditorState::undo_caret_operation`] can walk
/// back. The oldest is dropped past this.
const CARET_HISTORY: usize = 50;

/// The cursor set one caret operation replaced, restored by
/// [`EditorState::undo_caret_operation`].
#[derive(Clone, Debug)]
pub(crate) struct CaretSnapshot {
    /// The buffer version the set was recorded at; the positions mean nothing
    /// in any other version of the text.
    version: u64,
    /// The whole set before the operation.
    cursors: CursorState,
    /// The first-placed caret's index before the operation.
    original: usize,
    /// The whole-word occurrence run before the operation, so a `Ctrl+D`
    /// after an undo keeps matching the way it did.
    word_occurrences: Option<Vec<Selection>>,
}

impl EditorState {
    /// Run the caret operation `op` and, when it changed the cursor set,
    /// record the set it replaced for
    /// [`undo_caret_operation`](Self::undo_caret_operation).
    pub(super) fn caret_op(&mut self, buffer: &TextBuffer, op: impl FnOnce(&mut Self)) {
        let before = CaretSnapshot {
            version: buffer.version(),
            cursors: self.cursors.clone(),
            original: self.original,
            word_occurrences: self.word_occurrences.clone(),
        };
        op(self);
        if self.cursors == before.cursors {
            return;
        }
        if self.caret_history.len() >= CARET_HISTORY {
            self.caret_history.remove(0);
        }
        self.caret_history.push(before);
    }

    /// Undo Last Cursor Operation (`Ctrl+U`): restore the cursor set from
    /// before the most recent caret operation — a caret added above, below or
    /// by Alt+click, an occurrence added or skipped, all occurrences
    /// selected, or carets added to line ends. Repeating walks further back.
    ///
    /// Returns whether a set was restored. Nothing is restored once the text
    /// has changed since the operation: the history is dropped instead, as
    /// its positions name text that is no longer there.
    pub fn undo_caret_operation(&mut self, buffer: &TextBuffer) -> bool {
        let Some(last) = self.caret_history.pop() else {
            return false;
        };
        if last.version != buffer.version() {
            // Versions only grow, so every older entry is stale too.
            self.caret_history.clear();
            return false;
        }
        self.set_cursor_state(buffer, last.cursors);
        self.original = last
            .original
            .min(self.cursors.selections.len().saturating_sub(1));
        self.word_occurrences = last.word_occurrences;
        self.upstream.clear();
        true
    }

    /// Select All Occurrences (`Ctrl+Shift+L`): replace the cursor set with a
    /// selection over every occurrence of the primary selection's text, the
    /// primary staying on the occurrence it was on.
    ///
    /// A bare primary caret first takes the word touching it and then
    /// matches whole words only, as a `Ctrl+D` run from a caret does; nothing
    /// happens off a word. A selection matches its text anywhere, unless it
    /// is still the set a whole-word `Ctrl+D` run left.
    pub fn select_all_occurrences(&mut self, buffer: &TextBuffer) {
        self.caret_op(buffer, |state| {
            let bare = state.cursors.primary().is_empty();
            if bare {
                state.select_word_at_primary(buffer);
                if state.cursors.primary().is_empty() {
                    return;
                }
            }
            let whole_word = bare || state.whole_word_run();
            let primary = state.cursors.primary().range();
            let mut found = find_all(buffer, &range_text(buffer, primary), whole_word);
            // A scan left to right can step over the primary itself when an
            // overlapping occurrence starts before it (`aa` selected at the
            // end of `aaa`); the primary wins over any match it overlaps.
            if !found.contains(&primary) {
                found.retain(|r| !(r.start < primary.end && primary.start < r.end));
                found.push(primary);
                found.sort_by_key(|r| (r.start, r.end));
            }
            let at = found.iter().position(|r| *r == primary).unwrap_or(0);
            let selections = found
                .into_iter()
                .map(|r| Selection {
                    anchor: r.start,
                    head: r.end,
                })
                .collect();
            state.cursors = CursorState {
                selections,
                primary: at,
            };
            state.original = at;
            state.goals = None;
            state.upstream.clear();
            state.normalize();
            state.word_occurrences = whole_word.then(|| state.cursors.selections.clone());
            state.reveal(buffer, state.cursor());
        });
    }

    /// Skip Occurrence (`Ctrl+K Ctrl+D`): move the primary selection — the
    /// most recently added match — to the next occurrence of its text,
    /// wrapping and passing over occurrences already selected, so the match
    /// it was on is left unselected. The other selections stay.
    ///
    /// A bare primary caret takes the word touching it instead, as `Ctrl+D`
    /// does. With no further occurrence nothing changes.
    pub fn skip_occurrence(&mut self, buffer: &TextBuffer) {
        self.caret_op(buffer, |state| {
            if state.select_word_at_primary(buffer) {
                return;
            }
            let (found, whole_word) = state.next_occurrence(buffer);
            let Some(found) = found else {
                return;
            };
            if let Some(s) = state.cursors.selections.get_mut(state.cursors.primary) {
                *s = Selection {
                    anchor: found.start,
                    head: found.end,
                };
            }
            state.goals = None;
            state.upstream.clear();
            state.normalize();
            state.word_occurrences = whole_word.then(|| state.cursors.selections.clone());
            state.reveal(buffer, state.cursor());
        });
    }

    /// Add Cursors to Line Ends (`Shift+Alt+I`): replace each selection with a
    /// bare caret at the end of every line it spans. On its last line the
    /// caret goes where the selection ends, and a selection ending at the
    /// start of a line puts none there. A selection within one line becomes a
    /// caret at its end; a bare caret stays as it is.
    ///
    /// The primary becomes the last caret the primary selection produced.
    pub fn add_cursors_to_line_ends(&mut self, buffer: &TextBuffer) {
        self.caret_op(buffer, |state| {
            let mut carets = Vec::new();
            let mut primary = 0;
            for (i, s) in state.cursors.selections.iter().enumerate() {
                let r = s.range();
                carets.extend(
                    (r.start.line..r.end.line)
                        .map(|line| Selection::caret(LineCol::new(line, line_len(buffer, line)))),
                );
                if r.start.line == r.end.line || r.end.col > 0 {
                    carets.push(Selection::caret(r.end));
                }
                if i == state.cursors.primary {
                    primary = carets.len().saturating_sub(1);
                }
            }
            if carets.is_empty() {
                return;
            }
            state.cursors = CursorState {
                selections: carets,
                primary,
            };
            state.original = primary;
            state.goals = None;
            state.upstream.clear();
            state.normalize();
            state.reveal(buffer, state.cursor());
        });
    }
}
