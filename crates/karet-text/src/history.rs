//! In-memory undo/redo history for a [`TextBuffer`](crate::TextBuffer).
//!
//! Each [`Revision`] stores the *inverse* change (to undo) and the *forward*
//! change (to redo), tagged with a coalescing `group`. Consecutive single-`char`
//! typing within a short time window and contiguous in position share one group,
//! so it undoes as a unit. History records the last saved group for undo/redo
//! bookkeeping, while [`TextBuffer`](crate::TextBuffer) derives dirty state from
//! a content fingerprint so equivalent manual edits become clean again.
//!
//! History is purely in-memory and lives for the buffer's lifetime; it is dropped
//! only by [`TextBuffer::reset_history`](crate::TextBuffer::reset_history), which
//! the session calls after an accepted external reload (the recorded inverse edits
//! no longer match the new on-disk content).

use karet_core::Change;
use karet_core::CursorState;
use karet_core::LineCol;

/// Coalesce consecutive typing within this many milliseconds into one undo step.
const COALESCE_WINDOW_MS: u64 = 200;

/// Why an edit happened. Drives undo coalescing: only [`EditCause::Type`] is
/// eligible to merge with the preceding edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EditCause {
    /// A single typed character.
    Type,
    /// A newline (always starts a fresh undo group).
    Newline,
    /// A backward/forward deletion.
    Delete,
    /// A clipboard paste (one group, however large).
    Paste,
    /// A cut (delete of a selection).
    Cut,
    /// A programmatic or multi-edit replacement.
    Replace,
    /// An edit originating from outside the editor (e.g. a reload).
    External,
}

/// Context supplied alongside a [`Change`] when applying it: the monotonic
/// coalescing clock (millis), why the edit happened, and the cursor *before* it
/// (restored on undo).
#[derive(Clone, Debug)]
pub struct EditContext {
    /// Monotonic millisecond tick, supplied by the caller (the engine reads no
    /// wall-clock, so it stays pure and deterministically testable).
    pub tick_ms: u64,
    /// Why the edit happened.
    pub cause: EditCause,
    /// The cursor/selection state immediately before the edit.
    pub cursor_before: CursorState,
}

impl Default for EditContext {
    fn default() -> Self {
        Self {
            tick_ms: 0,
            // `Replace` never coalesces, so each `apply_simple` is its own undo step.
            cause: EditCause::Replace,
            cursor_before: CursorState::default(),
        }
    }
}

/// One reversible step.
#[derive(Clone)]
struct Revision {
    /// Applying this restores the pre-edit content (post-edit coordinate frame).
    inverse: Change,
    /// The original edit, replayed on redo (pre-edit coordinate frame).
    forward: Change,
    /// Coalescing group id (monotonic; equal ids undo/redo together).
    group: u64,
    /// The cursor before this edit, restored when the group is undone.
    cursor_before: CursorState,
}

/// The undo/redo stacks plus coalescing and save-point bookkeeping.
#[derive(Clone, Default)]
pub(crate) struct History {
    undo: Vec<Revision>,
    redo: Vec<Revision>,
    next_group: u64,
    saved_group: u64,
    last_tick_ms: u64,
    last_caret_after: Option<LineCol>,
}

impl History {
    fn alloc_group(&mut self) -> u64 {
        self.next_group += 1;
        self.next_group
    }

    /// The group id of the current head state (0 == the initial, never-edited state).
    fn current_group(&self) -> u64 {
        self.undo.last().map_or(0, |r| r.group)
    }

    /// Whether the head differs from the last save point.
    #[cfg(test)]
    pub(crate) fn is_dirty(&self) -> bool {
        self.current_group() != self.saved_group
    }

    /// Mark the current head as the saved state (clears dirty).
    pub(crate) fn mark_saved(&mut self) {
        self.saved_group = self.current_group();
    }

    /// Record an applied edit, coalescing with the previous one when eligible.
    /// `edit_end` is the caret position just after the edit (for adjacency).
    pub(crate) fn record(
        &mut self,
        inverse: Change,
        forward: Change,
        ctx: &EditContext,
        edit_end: Option<LineCol>,
    ) {
        self.redo.clear();
        let coalesce = ctx.cause == EditCause::Type
            && is_single_char_insert(&forward)
            && !self.undo.is_empty()
            && self.last_caret_after == Some(ctx.cursor_before.primary().head)
            && ctx.tick_ms.saturating_sub(self.last_tick_ms) <= COALESCE_WINDOW_MS;
        let group = if coalesce {
            self.current_group()
        } else {
            self.alloc_group()
        };
        self.undo.push(Revision {
            inverse,
            forward,
            group,
            cursor_before: ctx.cursor_before.clone(),
        });
        self.last_tick_ms = ctx.tick_ms;
        self.last_caret_after = edit_end;
    }

    /// Pop the top undo group, moving it to the redo stack. Returns the inverse
    /// changes to apply (in order) and the cursor to restore.
    pub(crate) fn take_undo(&mut self) -> Option<(Vec<Change>, CursorState)> {
        let group = self.undo.last()?.group;
        let mut changes = Vec::new();
        let mut cursor = CursorState::default();
        while self.undo.last().is_some_and(|r| r.group == group) {
            if let Some(rev) = self.undo.pop() {
                changes.push(rev.inverse.clone());
                cursor = rev.cursor_before.clone();
                self.redo.push(rev);
            }
        }
        self.last_caret_after = None;
        Some((changes, cursor))
    }

    /// Pop the top redo group, moving it back to the undo stack. Returns the
    /// forward changes to re-apply (in order).
    pub(crate) fn take_redo(&mut self) -> Option<Vec<Change>> {
        let group = self.redo.last()?.group;
        let mut changes = Vec::new();
        while self.redo.last().is_some_and(|r| r.group == group) {
            if let Some(rev) = self.redo.pop() {
                changes.push(rev.forward.clone());
                self.undo.push(rev);
            }
        }
        self.last_caret_after = None;
        Some(changes)
    }
}

/// Whether `change` is a single insertion of exactly one non-newline `char`.
fn is_single_char_insert(change: &Change) -> bool {
    let [edit] = change.edits.as_slice() else {
        return false;
    };
    if edit.range.start != edit.range.end {
        return false;
    }
    let mut chars = edit.new_text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c != '\n')
}

#[cfg(test)]
mod tests {
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::Selection;
    use karet_core::TextEdit;

    use super::*;

    fn insert(line: u32, col: u32, text: &str) -> Change {
        Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(line, col),
                    end: LineCol::new(line, col),
                },
                new_text: text.to_string(),
            }],
        )
    }

    fn typed(tick: u64, line: u32, col: u32) -> EditContext {
        EditContext {
            tick_ms: tick,
            cause: EditCause::Type,
            cursor_before: CursorState::single(Selection::caret(LineCol::new(line, col))),
        }
    }

    fn group_of(h: &History, idx: usize) -> u64 {
        h.undo.get(idx).map_or(0, |r| r.group)
    }

    #[test]
    fn adjacent_typing_coalesces() {
        let mut h = History::default();
        // Type "ab": both single chars, adjacent, within the window.
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        h.record(
            insert(0, 1, "b"),
            insert(0, 1, "b"),
            &typed(100, 0, 1),
            Some(LineCol::new(0, 2)),
        );
        assert_eq!(h.undo.len(), 2);
        assert_eq!(
            group_of(&h, 0),
            group_of(&h, 1),
            "adjacent typing shares a group"
        );
        let (changes, _) = h.take_undo().unwrap_or_default();
        assert_eq!(changes.len(), 2, "whole word undoes as one step");
    }

    #[test]
    fn caret_jump_breaks_coalescing() {
        let mut h = History::default();
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        // Caret jumped to col 5 (not adjacent to the last edit end at col 1).
        h.record(
            insert(0, 5, "b"),
            insert(0, 5, "b"),
            &typed(50, 0, 5),
            Some(LineCol::new(0, 6)),
        );
        assert_ne!(group_of(&h, 0), group_of(&h, 1));
    }

    #[test]
    fn time_gap_breaks_coalescing() {
        let mut h = History::default();
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        h.record(
            insert(0, 1, "b"),
            insert(0, 1, "b"),
            &typed(1_000, 0, 1),
            Some(LineCol::new(0, 2)),
        );
        assert_ne!(group_of(&h, 0), group_of(&h, 1));
    }

    #[test]
    fn dirty_tracks_save_point() {
        let mut h = History::default();
        assert!(!h.is_dirty());
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        // A new (non-coalescing) edit re-dirties.
        h.record(
            insert(0, 1, "X"),
            insert(0, 1, "X"),
            &EditContext {
                tick_ms: 9_999,
                cause: EditCause::Replace,
                cursor_before: CursorState::default(),
            },
            None,
        );
        assert!(h.is_dirty());
        // Undoing back to the saved group clears dirty again.
        h.take_undo();
        assert!(!h.is_dirty());
    }

    #[test]
    fn redo_round_trips() {
        let mut h = History::default();
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        let group = group_of(&h, 0);
        h.take_undo();
        assert!(h.undo.is_empty());
        let redo = h.take_redo().unwrap_or_default();
        assert_eq!(redo.len(), 1);
        assert_eq!(h.undo.len(), 1);
        assert_eq!(group_of(&h, 0), group);
    }
}
