//! [`ListSelection`] — a shared multi-select model for list-like panes.
//!
//! A single headless primitive (no ratatui) that every list pane — the file
//! explorer, the source-control change list, and later the search results — uses
//! to track which rows are selected. It supports the two gestures editors expect:
//!
//! - a **contiguous range** anchored at a pivot (Shift+Arrows / Shift-click), and
//! - an **arbitrary toggle-set** of individual rows (Space/Ctrl+click).
//!
//! # Model
//!
//! [`marked`](ListSelection) is the committed toggle-set. An active range is a
//! *live overlay* pivoted at [`anchor`](ListSelection) and led by
//! [`cursor`](ListSelection); it is recomputed on every read so repeatedly
//! extending never leaves a stale range behind. Starting a fresh toggle first
//! *commits* the current live range into `marked`, which is what makes mixed
//! range-then-toggle gestures behave predictably (Shift-select 2–5, then
//! Ctrl-click 8, yields `{2,3,4,5,8}` — not `{2..=8}`).
//!
//! The effective selection is therefore:
//!
//! ```text
//! marked ∪ (anchor ? cursor..=anchor : ∅)
//!        |> (if empty then {cursor})   // never an empty actionable selection
//!        |> clamp to [0, len)
//! ```
//!
//! The `empty → {cursor}` fallback keeps the "cursor *is* the selection" invariant
//! that single-row actions rely on, while still letting an explicit multi-set
//! exclude the cursor row (Ctrl+click the lead row to drop it).

use std::collections::BTreeSet;

/// A cursor-plus-selection model over a list of `len` rows, supporting a
/// contiguous anchored range and an arbitrary toggle-set (see the [module
/// docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListSelection {
    /// The number of selectable rows; every index is kept within `[0, len)`.
    len: usize,
    /// The lead ("active") row. `< len` whenever `len > 0`, else `0`.
    cursor: usize,
    /// The range pivot; `Some` only while a range gesture is active.
    anchor: Option<usize>,
    /// The committed toggle-set (explicit membership, independent of the range).
    marked: BTreeSet<usize>,
}

impl ListSelection {
    /// A fresh selection over `len` rows, cursor at `0` and nothing marked.
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            len,
            cursor: 0,
            anchor: None,
            marked: BTreeSet::new(),
        }
    }

    /// The lead (active) row index.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The range pivot, if a range gesture is active.
    #[must_use]
    pub fn anchor(&self) -> Option<usize> {
        self.anchor
    }

    /// The number of selectable rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no selectable rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Clamp `i` into `[0, len)`, mapping everything to `0` when the list is empty.
    fn clamp(&self, i: usize) -> usize {
        if self.len == 0 {
            0
        } else {
            i.min(self.len - 1)
        }
    }

    /// Whether row `i` is part of the effective selection.
    ///
    /// Consistent with [`selected_indices`](Self::selected_indices): a row counts
    /// if it is marked or inside the live range; failing that, the cursor row
    /// counts only when nothing else is selected (the empty-selection fallback).
    #[must_use]
    pub fn is_selected(&self, i: usize) -> bool {
        if i >= self.len {
            return false;
        }
        if self.marked.contains(&i) {
            return true;
        }
        if let Some(a) = self.anchor {
            let (lo, hi) = (a.min(self.cursor), a.max(self.cursor));
            if (lo..=hi).contains(&i) {
                return true;
            }
        }
        // Empty-selection fallback: the cursor is selected only when neither an
        // explicit mark nor an active range contributes anything.
        self.marked.is_empty() && self.anchor.is_none() && i == self.cursor
    }

    /// The effective selection as a sorted, de-duplicated list of row indices.
    #[must_use]
    pub fn selected_indices(&self) -> Vec<usize> {
        let mut set: BTreeSet<usize> = self
            .marked
            .iter()
            .copied()
            .filter(|&i| i < self.len)
            .collect();
        if let Some(a) = self.anchor {
            let (lo, hi) = (a.min(self.cursor), a.max(self.cursor));
            for i in lo..=hi {
                if i < self.len {
                    set.insert(i);
                }
            }
        }
        if set.is_empty() && self.cursor < self.len {
            set.insert(self.cursor);
        }
        set.into_iter().collect()
    }

    /// Move the cursor to `i`, collapsing to a single-row selection (a plain
    /// arrow / unmodified click: clears the range and the toggle-set).
    pub fn move_to(&mut self, i: usize) {
        self.cursor = self.clamp(i);
        self.anchor = None;
        self.marked.clear();
    }

    /// Move the cursor by `delta` rows (saturating at the ends), collapsing the
    /// selection like [`move_to`](Self::move_to).
    pub fn move_by(&mut self, delta: i32) {
        if self.len == 0 {
            return;
        }
        let next = (self.cursor as i64 + i64::from(delta)).clamp(0, self.len as i64 - 1);
        self.move_to(next as usize);
    }

    /// Extend the range to `i`: set the anchor at the current cursor on the first
    /// extension, then move the cursor, leaving the toggle-set untouched.
    pub fn extend_to(&mut self, i: usize) {
        if self.len == 0 {
            return;
        }
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        self.cursor = self.clamp(i);
    }

    /// Extend the range by `delta` rows (saturating at the ends), like
    /// [`extend_to`](Self::extend_to).
    pub fn extend_by(&mut self, delta: i32) {
        if self.len == 0 {
            return;
        }
        let next = (self.cursor as i64 + i64::from(delta)).clamp(0, self.len as i64 - 1);
        self.extend_to(next as usize);
    }

    /// Toggle row `i` in the selection and make it the cursor.
    ///
    /// The current effective selection is committed into the toggle-set first —
    /// an active range, or (when nothing explicit is selected) the fallback cursor
    /// row — so that toggling a *new* row keeps whatever was already selected
    /// (Ctrl-clicking a second row unions it with the first). Then `i` is flipped.
    pub fn toggle(&mut self, i: usize) {
        if self.len == 0 {
            return;
        }
        let i = self.clamp(i);
        if let Some(a) = self.anchor.take() {
            let (lo, hi) = (a.min(self.cursor), a.max(self.cursor));
            for j in lo..=hi {
                self.marked.insert(j);
            }
        } else if self.marked.is_empty() {
            // Promote the implicit single-row (fallback) selection to an explicit
            // mark so it survives this toggle.
            self.marked.insert(self.cursor);
        }
        if !self.marked.remove(&i) {
            self.marked.insert(i);
        }
        self.cursor = i;
    }

    /// Toggle the current cursor row (Space in a list pane).
    pub fn toggle_cursor(&mut self) {
        self.toggle(self.cursor);
    }

    /// Select every row, clearing any active range (the cursor is unchanged).
    pub fn select_all(&mut self) {
        self.marked = (0..self.len).collect();
        self.anchor = None;
    }

    /// Clear the toggle-set and any active range; the cursor stays put (so the
    /// effective selection falls back to the cursor row).
    pub fn clear(&mut self) {
        self.marked.clear();
        self.anchor = None;
    }

    /// Reconcile with a new row count after the underlying list changes: clamp
    /// the cursor, drop marks and an anchor that no longer exist.
    ///
    /// Reconciliation is *by index* — after a list reorders, a preserved cursor
    /// points at whatever row now occupies that slot.
    pub fn set_len(&mut self, n: usize) {
        self.len = n;
        if n == 0 {
            self.cursor = 0;
        } else if self.cursor >= n {
            self.cursor = n - 1;
        }
        self.marked.retain(|&i| i < n);
        if let Some(a) = self.anchor
            && a >= n
        {
            self.anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selects_only_the_cursor() {
        let sel = ListSelection::new(3);
        assert_eq!(sel.cursor(), 0);
        assert_eq!(sel.selected_indices(), vec![0]);
        assert!(sel.is_selected(0));
        assert!(!sel.is_selected(1));
    }

    #[test]
    fn empty_list_has_no_selection() {
        let mut sel = ListSelection::new(0);
        assert!(sel.is_empty());
        assert_eq!(sel.selected_indices(), Vec::<usize>::new());
        // Mutators are safe no-ops on an empty list.
        sel.move_by(1);
        sel.extend_by(1);
        sel.toggle(0);
        assert_eq!(sel.selected_indices(), Vec::<usize>::new());
    }

    #[test]
    fn move_by_collapses_range_and_marks() {
        let mut sel = ListSelection::new(10);
        sel.extend_by(3); // range 0..=3
        sel.toggle(6); // commit range, mark 6 → {0,1,2,3,6}
        assert_eq!(sel.selected_indices(), vec![0, 1, 2, 3, 6]);
        sel.move_by(1); // plain move collapses everything
        assert_eq!(sel.cursor(), 7);
        assert_eq!(sel.anchor(), None);
        assert_eq!(sel.selected_indices(), vec![7]);
    }

    #[test]
    fn extend_builds_a_contiguous_range() {
        let mut sel = ListSelection::new(10);
        sel.move_to(2);
        sel.extend_to(5);
        assert_eq!(sel.anchor(), Some(2));
        assert_eq!(sel.cursor(), 5);
        assert_eq!(sel.selected_indices(), vec![2, 3, 4, 5]);
        // Extending backwards past the anchor flips the range direction.
        sel.extend_to(0);
        assert_eq!(sel.selected_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn range_then_toggle_composes_without_stale_range() {
        // The canonical case: Shift-select 2–5, then Ctrl-click 8 → {2,3,4,5,8}.
        let mut sel = ListSelection::new(10);
        sel.move_to(2);
        sel.extend_to(5);
        sel.toggle(8);
        assert_eq!(sel.anchor(), None);
        assert_eq!(sel.cursor(), 8);
        assert_eq!(sel.selected_indices(), vec![2, 3, 4, 5, 8]);
    }

    #[test]
    fn toggle_preserves_the_prior_single_selection() {
        // Plain-select row 1, then toggle row 3: Ctrl-clicking a second row unions
        // it with the first rather than dropping it.
        let mut sel = ListSelection::new(5);
        sel.move_to(1);
        sel.toggle(3);
        assert_eq!(sel.selected_indices(), vec![1, 3]);
        // Toggling the cursor row off leaves the other mark; cursor excluded.
        sel.toggle(3);
        assert_eq!(sel.cursor(), 3);
        assert!(!sel.is_selected(3));
        assert_eq!(sel.selected_indices(), vec![1]);
    }

    #[test]
    fn toggling_off_the_only_selected_row_falls_back_to_the_cursor() {
        let mut sel = ListSelection::new(5);
        sel.move_to(2); // single {2}
        sel.toggle(4); // {2,4}
        sel.toggle(4); // {2}
        sel.toggle(2); // removed → empty union → fallback to cursor (2)
        assert_eq!(sel.cursor(), 2);
        assert_eq!(sel.selected_indices(), vec![2]);
    }

    #[test]
    fn select_all_and_clear() {
        let mut sel = ListSelection::new(4);
        sel.select_all();
        assert_eq!(sel.selected_indices(), vec![0, 1, 2, 3]);
        sel.clear();
        assert_eq!(sel.selected_indices(), vec![0]); // fallback to cursor
    }

    #[test]
    fn set_len_reconciles_cursor_marks_and_anchor() {
        let mut sel = ListSelection::new(10);
        sel.move_to(2); // single {2}
        sel.toggle(7); // {2,7}, cursor 7
        sel.toggle(9); // {2,7,9}, cursor 9
        sel.set_len(5); // drop marks >= 5, clamp cursor 9 → 4
        assert_eq!(sel.len(), 5);
        assert_eq!(sel.cursor(), 4);
        assert_eq!(sel.selected_indices(), vec![2]);
    }

    #[test]
    fn set_len_drops_out_of_range_anchor() {
        let mut sel = ListSelection::new(10);
        sel.move_to(1);
        sel.extend_to(8); // anchor 1, cursor 8
        sel.set_len(4); // cursor → 3, anchor 1 still valid
        assert_eq!(sel.anchor(), Some(1));
        assert_eq!(sel.selected_indices(), vec![1, 2, 3]);
        sel.set_len(10);
        sel.move_to(0);
        sel.extend_to(9); // anchor 0, cursor 9
        sel.set_len(3); // cursor → 2, anchor 0 valid
        assert_eq!(sel.selected_indices(), vec![0, 1, 2]);
    }
}
