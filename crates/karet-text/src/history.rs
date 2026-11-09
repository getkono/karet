//! In-memory undo/redo history for a [`TextBuffer`](crate::TextBuffer).
//!
//! Each [`Revision`] stores the *inverse* change (to undo) and the *forward*
//! change (to redo), tagged with a coalescing `group`. Consecutive single-`char`
//! typing within a short time window and contiguous in position share one group,
//! so it undoes as a unit. History records the last saved group for undo/redo
//! bookkeeping, while [`TextBuffer`](crate::TextBuffer) derives dirty state from
//! a content fingerprint so equivalent manual edits become clean again.
//!
//! History lives in memory for the buffer's lifetime; it is dropped only by
//! [`TextBuffer::adopt_content`](crate::TextBuffer::adopt_content) after an
//! accepted external reload (the recorded inverse edits no longer match the new
//! on-disk content). An optional cap
//! ([`TextBuffer::set_undo_limit`](crate::TextBuffer::set_undo_limit)) bounds it
//! to the newest N undo steps, forgetting the oldest. A caller that wants undo to
//! outlive the process takes a [`HistorySnapshot`] and restores it into a buffer
//! holding the same text; the snapshot carries a fingerprint of that text, so a
//! restore into anything else is refused rather than replaying edits that no
//! longer line up.

use std::collections::VecDeque;

use karet_core::Change;
use karet_core::CursorState;
use karet_core::EditCause;
use karet_core::LineCol;
use karet_core::TextEdit;

/// Coalesce consecutive typing within this many milliseconds into one undo step.
const COALESCE_WINDOW_MS: u64 = 200;

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
    /// The cursor right after this edit, restored when the group is redone;
    /// `None` until the client reports it (see [`History::note_cursor_after`]).
    cursor_after: Option<CursorState>,
}

/// The undo/redo stacks plus coalescing bookkeeping.
#[derive(Clone, Default)]
pub(crate) struct History {
    /// Oldest revision at the front, so the cap can forget it cheaply.
    undo: VecDeque<Revision>,
    redo: Vec<Revision>,
    /// How many distinct groups `undo` holds (kept, not recounted, so a push
    /// under a cap stays O(1)).
    undo_groups: usize,
    /// The most undo groups kept, or `None` for no cap.
    limit: Option<usize>,
    next_group: u64,
    last_tick_ms: u64,
    last_caret_after: Option<LineCol>,
}

/// A serializable copy of a buffer's undo/redo history, taken by
/// [`TextBuffer::history_snapshot`](crate::TextBuffer::history_snapshot) and put
/// back by [`TextBuffer::restore_history`](crate::TextBuffer::restore_history).
///
/// Opaque on purpose: its only contract is that it round-trips (through
/// `serde`, with the `serde` feature) and restores into a buffer whose text is
/// the text it was taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct HistorySnapshot {
    /// Fingerprint of the buffer text the history was taken at.
    content_hash: u64,
    /// Undo steps, oldest first.
    undo: Vec<StepSnapshot>,
    /// Redo steps, in stack order (the next redo last).
    redo: Vec<StepSnapshot>,
}

/// One persisted [`Revision`]. Document versions are deliberately not kept:
/// they number one buffer's lifetime and mean nothing to the next one.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct StepSnapshot {
    inverse: Vec<TextEdit>,
    forward: Vec<TextEdit>,
    group: u64,
    cursor_before: CursorState,
    cursor_after: Option<CursorState>,
}

impl HistorySnapshot {
    /// The fingerprint of the text this history belongs to.
    #[must_use]
    pub(crate) fn content_hash(&self) -> u64 {
        self.content_hash
    }

    /// Whether the snapshot holds nothing to undo or redo.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

impl History {
    /// The undo-step cap, or `None` when history is unbounded.
    pub(crate) fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// Cap the undo stack at `limit` groups (`None` lifts the cap), forgetting
    /// the oldest groups at once when it already holds more.
    pub(crate) fn set_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
        self.enforce_limit();
    }

    /// An empty history that keeps this one's cap.
    pub(crate) fn cleared(&self) -> Self {
        Self {
            limit: self.limit,
            ..Self::default()
        }
    }

    /// Drop the oldest undo groups until at most `limit` remain.
    fn enforce_limit(&mut self) {
        let Some(limit) = self.limit else {
            return;
        };
        while self.undo_groups > limit {
            let Some(oldest) = self.undo.front().map(|r| r.group) else {
                break;
            };
            while self.undo.front().is_some_and(|r| r.group == oldest) {
                self.undo.pop_front();
            }
            self.undo_groups -= 1;
        }
    }

    /// A serializable copy of both stacks, tagged with `content_hash`.
    pub(crate) fn snapshot(&self, content_hash: u64) -> HistorySnapshot {
        let step = |r: &Revision| StepSnapshot {
            inverse: r.inverse.edits.clone(),
            forward: r.forward.edits.clone(),
            group: r.group,
            cursor_before: r.cursor_before.clone(),
            cursor_after: r.cursor_after.clone(),
        };
        HistorySnapshot {
            content_hash,
            undo: self.undo.iter().map(step).collect(),
            redo: self.redo.iter().map(step).collect(),
        }
    }

    /// Rebuild a history from `snapshot`, keeping this one's cap. Every change is stamped with `base_version`, which the
    /// caller keeps below every version the buffer reaches afterwards, so no
    /// restored step is mistaken for the edit that produced a later version.
    pub(crate) fn restored(&self, snapshot: HistorySnapshot, base_version: u64) -> Self {
        let revision = |s: StepSnapshot| Revision {
            inverse: Change::new(base_version, s.inverse),
            forward: Change::new(base_version, s.forward),
            group: s.group,
            cursor_before: s.cursor_before,
            cursor_after: s.cursor_after,
        };
        let undo: VecDeque<Revision> = snapshot.undo.into_iter().map(revision).collect();
        let redo: Vec<Revision> = snapshot.redo.into_iter().map(revision).collect();
        let next_group = undo.iter().chain(&redo).map(|r| r.group).max().unwrap_or(0);
        let mut history = Self {
            undo_groups: count_groups(undo.iter()),
            undo,
            redo,
            limit: self.limit,
            next_group,
            ..Self::default()
        };
        history.enforce_limit();
        history
    }

    /// How many undo steps (groups) the history holds.
    pub(crate) fn undo_len(&self) -> usize {
        self.undo_groups
    }

    fn alloc_group(&mut self) -> u64 {
        self.next_group += 1;
        self.next_group
    }

    /// The group id of the current head state (0 == the initial, never-edited state).
    fn current_group(&self) -> u64 {
        self.undo.back().map_or(0, |r| r.group)
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
        self.push(inverse, forward, ctx, edit_end, group);
    }

    /// Record an applied edit as part of the current head group, so one undo
    /// reverts it together with the edit before it. With nothing to join (an
    /// empty undo stack) it starts a group of its own, exactly like
    /// [`record`](Self::record) would.
    pub(crate) fn record_joined(
        &mut self,
        inverse: Change,
        forward: Change,
        ctx: &EditContext,
        edit_end: Option<LineCol>,
    ) {
        self.redo.clear();
        let group = if self.undo.is_empty() {
            self.alloc_group()
        } else {
            self.current_group()
        };
        self.push(inverse, forward, ctx, edit_end, group);
    }

    fn push(
        &mut self,
        inverse: Change,
        forward: Change,
        ctx: &EditContext,
        edit_end: Option<LineCol>,
        group: u64,
    ) {
        if self.undo.is_empty() || group != self.current_group() {
            self.undo_groups += 1;
        }
        self.undo.push_back(Revision {
            inverse,
            forward,
            group,
            cursor_before: ctx.cursor_before.clone(),
            cursor_after: None,
        });
        self.last_tick_ms = ctx.tick_ms;
        self.last_caret_after = edit_end;
        self.enforce_limit();
    }

    /// Pop the top undo group, moving it to the redo stack. Returns the inverse
    /// changes to apply (in order) and the cursor to restore.
    pub(crate) fn take_undo(&mut self) -> Option<(Vec<Change>, CursorState)> {
        let group = self.undo.back()?.group;
        let mut changes = Vec::new();
        let mut cursor = CursorState::default();
        while self.undo.back().is_some_and(|r| r.group == group) {
            if let Some(rev) = self.undo.pop_back() {
                changes.push(rev.inverse.clone());
                cursor = rev.cursor_before.clone();
                self.redo.push(rev);
            }
        }
        self.undo_groups = self.undo_groups.saturating_sub(1);
        self.last_caret_after = None;
        Some((changes, cursor))
    }

    /// Pop the top redo group, moving it back to the undo stack. Returns the
    /// forward changes to re-apply (in order) and the cursor recorded right
    /// after the group's last edit, when one was.
    pub(crate) fn take_redo(&mut self) -> Option<(Vec<Change>, Option<CursorState>)> {
        let group = self.redo.last()?.group;
        let mut changes = Vec::new();
        let mut cursor = None;
        while self.redo.last().is_some_and(|r| r.group == group) {
            if let Some(rev) = self.redo.pop() {
                changes.push(rev.forward.clone());
                // Popped oldest first, so the last one seen is the newest edit.
                cursor.clone_from(&rev.cursor_after);
                self.undo.push_back(rev);
            }
        }
        self.undo_groups += 1;
        // A cap lowered while groups sat on the redo stack still holds.
        self.enforce_limit();
        self.last_caret_after = None;
        Some((changes, cursor))
    }

    /// Record `cursor` as the state right after the newest edit, provided that
    /// edit is what produced `version` (an undo or redo since leaves the head
    /// on an older edit, so the report is about something else) and nothing
    /// was recorded for it yet. The first report after an edit is the one that
    /// describes it; later ones at the same version are caret moves made since.
    pub(crate) fn note_cursor_after(&mut self, version: u64, cursor: &CursorState) {
        if let Some(rev) = self.undo.back_mut()
            && rev.inverse.base_version == version
            && rev.cursor_after.is_none()
        {
            rev.cursor_after = Some(cursor.clone());
        }
    }
}

/// How many runs of equal `group` the revisions form (one run per undo step).
fn count_groups<'a>(revisions: impl Iterator<Item = &'a Revision>) -> usize {
    let mut count = 0;
    let mut previous = None;
    for rev in revisions {
        if previous != Some(rev.group) {
            count += 1;
            previous = Some(rev.group);
        }
    }
    count
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
    fn only_a_single_non_newline_char_coalesces() {
        // Every pair below is adjacent, typed and inside the window, so the
        // shape of the second insert is the only thing that can break the group.
        for second in ["bc", "\n", ""] {
            let mut h = History::default();
            h.record(
                insert(0, 0, "a"),
                insert(0, 0, "a"),
                &typed(0, 0, 0),
                Some(LineCol::new(0, 1)),
            );
            h.record(
                insert(0, 1, second),
                insert(0, 1, second),
                &typed(10, 0, 1),
                Some(LineCol::new(0, 1)),
            );
            assert_ne!(group_of(&h, 0), group_of(&h, 1), "{second:?} coalesced");
        }
    }

    #[test]
    fn a_single_char_replacement_does_not_coalesce() {
        let mut h = History::default();
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        let replace = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 1),
                    end: LineCol::new(0, 2),
                },
                new_text: "b".to_string(),
            }],
        );
        h.record(
            replace.clone(),
            replace,
            &typed(10, 0, 1),
            Some(LineCol::new(0, 2)),
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
        let (redo, cursor) = h.take_redo().unwrap_or_default();
        assert_eq!(redo.len(), 1);
        assert_eq!(cursor, None, "no cursor was reported after the edit");
        assert_eq!(h.undo.len(), 1);
        assert_eq!(group_of(&h, 0), group);
    }

    #[test]
    fn joined_edit_undoes_with_the_one_before_it_and_restores_its_cursor() {
        let mut h = History::default();
        h.record(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &typed(0, 0, 0),
            Some(LineCol::new(0, 1)),
        );
        // A non-coalescing replace starts its own group.
        let replace = |line, col| EditContext {
            tick_ms: 0,
            cause: EditCause::Replace,
            cursor_before: CursorState::single(Selection::caret(LineCol::new(line, col))),
        };
        h.record(insert(0, 1, "b"), insert(0, 1, "b"), &replace(3, 4), None);
        h.record_joined(insert(0, 2, "c"), insert(0, 2, "c"), &replace(9, 9), None);
        assert_ne!(group_of(&h, 0), group_of(&h, 1));
        assert_eq!(
            group_of(&h, 1),
            group_of(&h, 2),
            "the joined edit shares a group"
        );

        let (changes, cursor) = h.take_undo().unwrap_or_default();
        assert_eq!(changes.len(), 2, "one undo reverts both edits");
        assert_eq!(
            cursor.primary().head,
            LineCol::new(3, 4),
            "undo restores the cursor from before the first edit of the group"
        );
        assert_eq!(h.undo.len(), 1, "the edit before the group is untouched");
    }

    #[test]
    fn joined_edit_on_empty_history_starts_its_own_group() {
        let mut h = History::default();
        h.record_joined(
            insert(0, 0, "a"),
            insert(0, 0, "a"),
            &EditContext::default(),
            None,
        );
        assert_eq!(h.undo.len(), 1);
        assert_ne!(
            group_of(&h, 0),
            0,
            "group 0 is reserved for the unedited state"
        );
    }

    /// Record `n` edits that never coalesce, so each is its own undo step.
    fn record_steps(h: &mut History, n: u32) {
        for i in 0..n {
            h.record(
                insert(0, i, "x"),
                insert(0, i, "x"),
                &EditContext::default(),
                None,
            );
        }
    }

    #[test]
    fn cap_forgets_the_oldest_steps() {
        let mut h = History::default();
        h.set_limit(Some(3));
        record_steps(&mut h, 5);
        assert_eq!(h.undo_len(), 3);
        assert_eq!(h.undo.len(), 3);
        // Steps 1 and 2 were forgotten; the oldest kept is the third edit.
        assert_eq!(
            h.undo
                .front()
                .and_then(|r| r.forward.edits.first())
                .map(|e| e.range.start),
            Some(LineCol::new(0, 2))
        );
        let mut undone = 0;
        while h.take_undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, 3, "only the capped number of steps undo");
    }

    #[test]
    fn cap_counts_a_coalesced_run_as_one_step() {
        let mut h = History::default();
        h.set_limit(Some(1));
        // One typed word: three revisions, one group.
        for col in 0..3 {
            h.record(
                insert(0, col, "a"),
                insert(0, col, "a"),
                &typed(u64::from(col) * 10, 0, col),
                Some(LineCol::new(0, col + 1)),
            );
        }
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo.len(), 3, "the whole word survives the cap of one");
        record_steps(&mut h, 1);
        assert_eq!(h.undo.len(), 1, "the word is forgotten as a unit");
    }

    #[test]
    fn lowering_the_cap_trims_at_once_and_holds_through_redo() {
        let mut h = History::default();
        record_steps(&mut h, 4);
        h.take_undo();
        h.take_undo();
        h.set_limit(Some(1));
        assert_eq!(h.undo_len(), 1);
        // Redoing both undone steps cannot grow the stack past the cap.
        h.take_redo();
        h.take_redo();
        assert_eq!(h.undo_len(), 1);
        assert_eq!(h.undo.len(), 1);
        h.set_limit(None);
        record_steps(&mut h, 3);
        assert_eq!(h.undo_len(), 4, "lifting the cap stops forgetting");
    }

    #[test]
    fn zero_cap_keeps_no_undo() {
        let mut h = History::default();
        h.set_limit(Some(0));
        record_steps(&mut h, 2);
        assert!(h.take_undo().is_none());
    }

    #[test]
    fn group_count_tracks_undo_and_redo() {
        let mut h = History::default();
        record_steps(&mut h, 3);
        assert_eq!(h.undo_len(), 3);
        h.take_undo();
        assert_eq!(h.undo_len(), 2);
        h.take_redo();
        assert_eq!(h.undo_len(), 3);
        assert_eq!(h.undo_len(), count_groups(h.undo.iter()));
    }

    #[test]
    fn cleared_history_keeps_its_cap() {
        let mut h = History::default();
        h.set_limit(Some(7));
        record_steps(&mut h, 2);
        let cleared = h.cleared();
        assert_eq!(cleared.limit(), Some(7));
        assert_eq!(cleared.undo_len(), 0);
    }

    #[test]
    fn snapshot_restores_both_stacks_and_their_groups() {
        let mut h = History::default();
        record_steps(&mut h, 3);
        h.take_undo();
        let snapshot = h.snapshot(99);
        assert_eq!(snapshot.content_hash(), 99);
        assert!(!snapshot.is_empty());

        let restored = History::default().restored(snapshot, 5);
        assert_eq!(restored.undo_len(), 2);
        assert_eq!(restored.redo.len(), 1);
        assert!(
            restored
                .undo
                .iter()
                .chain(&restored.redo)
                .all(|r| r.inverse.base_version == 5 && r.forward.base_version == 5)
        );
        assert_eq!(
            restored.next_group, h.next_group,
            "new edits never reuse a restored group id"
        );
    }

    #[test]
    fn restore_applies_the_receiving_cap() {
        let mut h = History::default();
        record_steps(&mut h, 4);
        let mut capped = History::default();
        capped.set_limit(Some(2));
        let restored = capped.restored(h.snapshot(0), 0);
        assert_eq!(restored.undo_len(), 2);
        assert_eq!(restored.limit(), Some(2));
    }

    #[test]
    fn empty_history_snapshots_empty() {
        assert!(History::default().snapshot(0).is_empty());
    }

    #[test]
    fn either_stack_alone_makes_a_snapshot_non_empty() {
        let mut undo_only = History::default();
        record_steps(&mut undo_only, 1);
        assert!(!undo_only.snapshot(0).is_empty(), "an undo step alone");

        let mut redo_only = History::default();
        record_steps(&mut redo_only, 1);
        redo_only.take_undo();
        assert_eq!(redo_only.undo_len(), 0);
        assert!(!redo_only.snapshot(0).is_empty(), "a redo step alone");
    }
}
