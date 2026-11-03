//! Expand and Shrink Selection: every selection grows to the next enclosing
//! syntax range a caller supplies, and shrinks back through the exact sets it
//! grew from.
//!
//! The widget knows no syntax: the caller hands in the ranges that enclose a
//! selection (tree-sitter nodes, a language server's `selectionRange` chain),
//! and this module picks the next step for each selection and keeps the
//! history that Shrink walks back down.

use super::*;

/// One Expand Selection press: the cursor set before it and the set it left.
#[derive(Clone, Debug)]
pub(crate) struct SyntaxStep {
    /// The whole set before the press, restored by Shrink.
    before: CursorState,
    /// The first-placed caret's index before the press.
    before_original: usize,
    /// The set the press left. Shrink applies only while the set is still
    /// exactly this one; any other change of selection ends the history.
    after: CursorState,
}

impl EditorState {
    /// Expand Selection: grow every selection to the smallest range that
    /// strictly contains it, and remember the set it grew from so
    /// [`shrink_selection`](Self::shrink_selection) can restore it exactly.
    ///
    /// `enclosing` lists the ranges around a selection's range, in any order;
    /// those that do not contain it are ignored. A bare caret touching a word
    /// grows to that word first. A selection with nothing larger around it
    /// stays as it is. Each grown selection is anchored at its start with the
    /// head at its end; selections that grow into each other merge.
    ///
    /// Returns whether any selection grew. A press that grows nothing records
    /// no history and changes nothing.
    pub fn expand_selection(
        &mut self,
        buffer: &TextBuffer,
        mut enclosing: impl FnMut(Range) -> Vec<Range>,
    ) -> bool {
        let before = self.cursors.clone();
        let mut grew = false;
        for selection in &mut self.cursors.selections {
            let current = selection.range();
            let word = selection
                .is_empty()
                .then(|| word_at(buffer, selection.head))
                .flatten()
                .map(|(start, end)| Range { start, end });
            let next = match word {
                Some(word) => Some(word),
                None => smallest_enclosing(buffer, current, enclosing(current)),
            };
            if let Some(next) = next {
                *selection = Selection {
                    anchor: next.start,
                    head: next.end,
                };
                grew = true;
            }
        }
        if !grew {
            return false;
        }
        let before_original = self.original;
        self.normalize();
        // A press that follows anything but the previous press starts afresh.
        if self
            .syntax_steps
            .last()
            .is_some_and(|step| step.after != before)
        {
            self.syntax_steps.clear();
        }
        self.syntax_steps.push(SyntaxStep {
            before,
            before_original,
            after: self.cursors.clone(),
        });
        self.reveal(buffer, self.cursor());
        true
    }

    /// Shrink Selection: undo the last [`expand_selection`](Self::expand_selection),
    /// restoring the exact cursor set it grew from — every caret, its direction,
    /// and the primary.
    ///
    /// Applies only while the set is still exactly what that press left; after
    /// any other change of selection there is nothing to shrink back to.
    /// Returns whether anything was restored.
    pub fn shrink_selection(&mut self, buffer: &TextBuffer) -> bool {
        let Some(step) = self.syntax_steps.pop() else {
            return false;
        };
        if step.after != self.cursors {
            self.syntax_steps.clear();
            return false;
        }
        let mut restored = step.before;
        for selection in &mut restored.selections {
            selection.anchor = clamp_to_buffer(buffer, selection.anchor);
            selection.head = clamp_to_buffer(buffer, selection.head);
        }
        self.cursors = restored;
        self.original = step.before_original;
        self.normalize();
        self.reveal(buffer, self.cursor());
        true
    }
}

/// The smallest of `candidates` that strictly contains `current`, by the bytes
/// it spans. Ties keep the earliest-listed candidate.
fn smallest_enclosing(
    buffer: &TextBuffer,
    current: Range,
    candidates: Vec<Range>,
) -> Option<Range> {
    let bytes = |r: Range| -> Option<usize> {
        let start = buffer.line_col_to_byte(r.start).ok()?;
        let end = buffer.line_col_to_byte(r.end).ok()?;
        end.0.checked_sub(start.0)
    };
    candidates
        .into_iter()
        .filter(|r| r.start <= current.start && current.end <= r.end && *r != current)
        .filter_map(|r| bytes(r).map(|len| (len, r)))
        .min_by_key(|(len, _)| *len)
        .map(|(_, r)| r)
}
