//! Neutral, serializable edit and selection types.
//!
//! These describe *what changed* and *where the cursors are* in line/column space,
//! independent of the rope. `karet-text` applies them; the presentation layer and
//! the client-server seam exchange them.

use std::path::PathBuf;

use crate::coord::LineCol;
use crate::coord::Range;

/// A single replacement of `range` with `new_text` (LSP-shaped).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TextEdit {
    /// The range to replace.
    pub range: Range,
    /// The text to insert in its place.
    pub new_text: String,
}

/// An atomic batch of non-overlapping [`TextEdit`]s applied to one document.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Change {
    /// The document version these edits are relative to.
    pub base_version: u64,
    /// The edits, applied as a single atomic, non-overlapping batch.
    pub edits: Vec<TextEdit>,
}

impl Change {
    /// A change set applying `edits` atomically to document `base_version`.
    #[must_use]
    pub fn new(base_version: u64, edits: Vec<TextEdit>) -> Self {
        Self {
            base_version,
            edits,
        }
    }
}

/// Edits spanning multiple files (e.g. an LSP rename or workspace fix).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkspaceEdit {
    /// Per-file edit batches.
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
}

/// A single selection: a `head` (the moving caret) anchored at `anchor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Selection {
    /// The fixed end of the selection.
    pub anchor: LineCol,
    /// The moving end (where the caret is).
    pub head: LineCol,
}

impl Selection {
    /// An empty selection (a bare cursor) at `pos`.
    #[must_use]
    pub const fn caret(pos: LineCol) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    /// Whether the selection is empty (`anchor == head`).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    /// The caret (moving) end of the selection.
    #[must_use]
    pub const fn caret_pos(self) -> LineCol {
        self.head
    }

    /// The selection as a normalized half-open [`Range`] (`min..max`).
    #[must_use]
    pub fn range(self) -> Range {
        if self.anchor <= self.head {
            Range {
                start: self.anchor,
                end: self.head,
            }
        } else {
            Range {
                start: self.head,
                end: self.anchor,
            }
        }
    }
}

/// A multi-cursor state: a set of [`Selection`]s with a designated primary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CursorState {
    /// The selections, in document order.
    pub selections: Vec<Selection>,
    /// Index into `selections` of the primary selection.
    pub primary: usize,
}

impl CursorState {
    /// A single-cursor state from one selection.
    #[must_use]
    pub fn single(sel: Selection) -> Self {
        Self {
            selections: vec![sel],
            primary: 0,
        }
    }

    /// The primary selection, or a caret at the origin if there are none.
    #[must_use]
    pub fn primary(&self) -> Selection {
        self.selections
            .get(self.primary)
            .copied()
            .unwrap_or_else(|| Selection::caret(LineCol::new(0, 0)))
    }

    /// Add `sel` as the new primary selection, then merge any overlaps. Use this to
    /// grow the cursor set (add-caret, add-next-occurrence).
    pub fn push(&mut self, sel: Selection) {
        self.selections.push(sel);
        self.primary = self.selections.len().saturating_sub(1);
        self.normalize();
    }

    /// Collapse the set to just the primary selection (e.g. the `Esc` fold-back).
    pub fn collapse_to_primary(&mut self) {
        let p = self.primary();
        self.selections = vec![p];
        self.primary = 0;
    }

    /// Merge coincident carets and overlapping/touching selections into their
    /// forward-oriented union, preserving document order and which selection is
    /// primary. A single selection is left untouched (its orientation is preserved),
    /// so the common single-cursor path is a no-op. `selections` stays non-empty.
    pub fn normalize(&mut self) {
        if self.selections.len() <= 1 {
            return;
        }
        let primary_head = self.primary().head;
        let mut order = self.selections.clone();
        order.sort_by_key(|s| {
            let r = s.range();
            (r.start, r.end)
        });
        let mut merged: Vec<Selection> = Vec::with_capacity(order.len());
        for s in order {
            match merged.last_mut() {
                // Sorted, so `prev.start <= s.start`; they touch/overlap when
                // `s.start <= prev.end`. Fuse into the forward-oriented union.
                Some(prev) if s.range().start <= prev.range().end => {
                    let lo = prev.range().start.min(s.range().start);
                    let hi = prev.range().end.max(s.range().end);
                    *prev = Selection {
                        anchor: lo,
                        head: hi,
                    };
                },
                _ => merged.push(s),
            }
        }
        // The primary is the merged group that spans the old primary caret.
        self.primary = merged
            .iter()
            .position(|m| {
                let r = m.range();
                r.start <= primary_head && primary_head <= r.end
            })
            .unwrap_or(0);
        self.selections = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_caret_and_range() {
        let c = Selection::caret(LineCol::new(2, 3));
        assert!(c.is_empty());
        assert_eq!(c.caret_pos(), LineCol::new(2, 3));

        let s = Selection {
            anchor: LineCol::new(4, 0),
            head: LineCol::new(1, 2),
        };
        assert_eq!(
            s.range(),
            Range {
                start: LineCol::new(1, 2),
                end: LineCol::new(4, 0)
            }
        );
    }

    #[test]
    fn cursor_state_primary() {
        let cs = CursorState::single(Selection::caret(LineCol::new(0, 5)));
        assert_eq!(cs.primary().head, LineCol::new(0, 5));
        assert_eq!(
            CursorState::default().primary(),
            Selection::caret(LineCol::new(0, 0))
        );
    }

    #[test]
    fn normalize_merges_coincident_carets() {
        let mut cs = CursorState::single(Selection::caret(LineCol::new(1, 2)));
        cs.selections.push(Selection::caret(LineCol::new(1, 2)));
        cs.normalize();
        assert_eq!(cs.selections.len(), 1);
        assert_eq!(cs.primary().head, LineCol::new(1, 2));
    }

    #[test]
    fn normalize_merges_overlapping_and_tracks_primary() {
        // Three selections; the last (primary) overlaps the first.
        let mut cs = CursorState {
            selections: vec![
                Selection {
                    anchor: LineCol::new(0, 0),
                    head: LineCol::new(0, 4),
                },
                Selection::caret(LineCol::new(2, 0)),
                Selection {
                    anchor: LineCol::new(0, 3),
                    head: LineCol::new(0, 7),
                },
            ],
            primary: 2,
        };
        cs.normalize();
        // The two overlapping [0,0..0,4] and [0,3..0,7] fuse into [0,0..0,7]; the
        // caret at (2,0) stays separate.
        assert_eq!(cs.selections.len(), 2);
        assert_eq!(
            cs.selections[0].range(),
            Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 7),
            }
        );
        // The primary caret (0,7) still lives in the merged group.
        let p = cs.primary().range();
        assert!(p.start <= LineCol::new(0, 7) && LineCol::new(0, 7) <= p.end);
    }

    #[test]
    fn push_sets_primary_then_collapse_keeps_it() {
        let mut cs = CursorState::single(Selection::caret(LineCol::new(0, 0)));
        cs.push(Selection::caret(LineCol::new(3, 1)));
        assert_eq!(cs.selections.len(), 2);
        assert_eq!(cs.primary().head, LineCol::new(3, 1));
        cs.collapse_to_primary();
        assert_eq!(cs.selections, vec![Selection::caret(LineCol::new(3, 1))]);
        assert_eq!(cs.primary, 0);
    }
}
