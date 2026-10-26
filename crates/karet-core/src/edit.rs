//! Neutral, serializable edit and selection types.
//!
//! These describe *what changed* and *where the cursors are* in line/column space,
//! independent of the rope. `karet-text` applies them; the presentation layer and
//! the client-server seam exchange them.

use std::path::PathBuf;

use crate::coord::LineCol;
use crate::coord::Range;

/// A `(row, byte-column)` grid point, as tree-sitter counts them: the column is a
/// **byte** offset from the line start, not a `char` column like [`LineCol`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BytePoint {
    /// Zero-based row (line) index.
    pub row: usize,
    /// Byte offset from the start of the row.
    pub col: usize,
}

impl BytePoint {
    /// Create a point at `row` / byte-`col`.
    #[must_use]
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// One edit as applied to a text buffer, in tree-sitter `InputEdit` shape.
///
/// This is the neutral vocabulary between the buffer that applies edits
/// (`karet-text`) and the parse host that replays them incrementally
/// (`karet-treesitter`): the buffer produces these, `SyntaxTree::edit` consumes
/// them, and neither crate needs to know the other exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppliedEdit {
    /// Byte offset where the edit starts (same in old and new text).
    pub start_byte: usize,
    /// Byte offset of the end of the replaced region in the old text.
    pub old_end_byte: usize,
    /// Byte offset of the end of the inserted text in the new text.
    pub new_end_byte: usize,
    /// Grid point of `start_byte`.
    pub start_point: BytePoint,
    /// Grid point of `old_end_byte` (old text).
    pub old_end_point: BytePoint,
    /// Grid point of `new_end_byte` (new text).
    pub new_end_point: BytePoint,
    /// The text that was replaced (the inverse insertion).
    pub replaced: String,
}

/// Why an edit happened. Drives undo coalescing (only [`EditCause::Type`] is
/// eligible to merge with the preceding edit) and rides
/// the backend seam alongside each [`Change`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    /// Forward-compatibility fallback: an unrecognized cause from a newer peer.
    /// Never coalesces.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

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
///
/// Two halves, mirroring the LSP shape. [`changes`](Self::changes) is a set of
/// per-file text edits with no order between files;
/// [`document_changes`](Self::document_changes) is an ordered sequence that can
/// also create, rename, and delete files, and pins each text edit to the
/// document version it was computed against. A consumer applies `changes`
/// first and then `document_changes` in order; a producer normally fills only
/// one of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WorkspaceEdit {
    /// Per-file edit batches.
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
    /// Ordered text edits and file operations, applied after
    /// [`changes`](Self::changes).
    #[cfg_attr(feature = "serde", serde(default))]
    pub document_changes: Vec<DocumentChange>,
}

impl WorkspaceEdit {
    /// Whether applying this edit would change nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.iter().all(|(_, edits)| edits.is_empty()) && self.document_changes.is_empty()
    }
}

/// One step of [`WorkspaceEdit::document_changes`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DocumentChange {
    /// Apply text edits to one file.
    Edit(DocumentEdit),
    /// Create an empty file.
    Create {
        /// The file to create.
        path: PathBuf,
        /// What to do when it already exists.
        options: FileOptions,
    },
    /// Move a file or directory.
    Rename {
        /// The current path.
        from: PathBuf,
        /// The new path.
        to: PathBuf,
        /// What to do when `to` already exists.
        options: FileOptions,
    },
    /// Delete a file or directory.
    Delete {
        /// The path to delete.
        path: PathBuf,
        /// How to treat a directory, and a path that does not exist.
        options: DeleteOptions,
    },
}

/// Text edits to one file, pinned to the version they were computed against.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DocumentEdit {
    /// The file to edit.
    pub path: PathBuf,
    /// The document version the edits are relative to, in the producer's
    /// numbering (an LSP server's `TextDocumentEdit` version). `None` when the
    /// producer did not say, which a consumer treats as "the current text".
    pub version: Option<i32>,
    /// The edits, as one atomic, non-overlapping batch.
    pub edits: Vec<TextEdit>,
}

/// How a create or rename treats a destination that already exists.
///
/// When both are set, `overwrite` wins, as LSP specifies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileOptions {
    /// Replace the existing file.
    pub overwrite: bool,
    /// Leave the existing file alone and skip this step.
    pub ignore_if_exists: bool,
}

/// How a delete treats a directory, and a path that is already gone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeleteOptions {
    /// Delete a directory together with everything in it.
    pub recursive: bool,
    /// Skip this step when the path does not exist.
    pub ignore_if_not_exists: bool,
}

/// The outcome of applying a [`WorkspaceEdit`] (LSP's
/// `ApplyWorkspaceEditResult`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ApplyWorkspaceEditResult {
    /// Whether the edit was applied.
    pub applied: bool,
    /// Why it was not, when it was not.
    pub failure_reason: Option<String>,
    /// The index into [`WorkspaceEdit::document_changes`] of the step that
    /// failed, when the failure belongs to one step there.
    pub failed_change: Option<usize>,
}

impl ApplyWorkspaceEditResult {
    /// A fully applied edit.
    #[must_use]
    pub fn applied() -> Self {
        Self {
            applied: true,
            failure_reason: None,
            failed_change: None,
        }
    }

    /// A refused or failed edit, with the reason and the failing step, if any.
    #[must_use]
    pub fn failed(reason: impl Into<String>, failed_change: Option<usize>) -> Self {
        Self {
            applied: false,
            failure_reason: Some(reason.into()),
            failed_change,
        }
    }
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

    /// Collapse the set to just the primary selection (e.g. before a Shift+click
    /// extends it).
    pub fn collapse_to_primary(&mut self) {
        let p = self.primary();
        self.selections = vec![p];
        self.primary = 0;
    }

    /// Sort the selections into document order and merge the ones that collide,
    /// keeping the primary on the selection it was on.
    ///
    /// Two selections collide when they overlap, when they are coincident carets,
    /// or when a caret sits on or inside a selection. Two non-empty selections that
    /// only touch (one ends where the other starts) stay separate. A merged
    /// selection spans the union and keeps the direction of its members: the
    /// primary's when the primary is a non-empty member, else the first non-empty
    /// member's (a caret has no direction).
    /// `selections` stays non-empty when it was.
    pub fn normalize(&mut self) {
        let _ = self.normalize_tracking(0);
    }

    /// [`normalize`](Self::normalize), returning the index that the selection at
    /// `index` ended up in, so a caller can follow one selection (for example the
    /// first-placed caret) through sorting and merging. An out-of-range `index`
    /// follows the last selection.
    pub fn normalize_tracking(&mut self, index: usize) -> usize {
        let len = self.selections.len();
        if len == 0 {
            return 0;
        }
        let index = index.min(len - 1);
        let primary = self.primary.min(len - 1);
        let mut order: Vec<usize> = (0..len).collect();
        order.sort_by_key(|&i| {
            let r = self.selections[i].range();
            (r.start, r.end)
        });
        // Each merged selection, the members it absorbed, and where each original
        // index went.
        let mut merged: Vec<(Selection, Vec<usize>)> = Vec::with_capacity(len);
        let mut slot = vec![0; len];
        for i in order {
            let s = self.selections[i];
            match merged.last_mut() {
                Some((prev, members)) if collides(*prev, s) => {
                    members.push(i);
                    let lo = prev.range().start.min(s.range().start);
                    let hi = prev.range().end.max(s.range().end);
                    // A caret has no direction, so only a non-empty member leads.
                    let directed = |m: &usize| !self.selections[*m].is_empty();
                    let leader = members
                        .iter()
                        .filter(|m| directed(m))
                        .find(|&&m| m == primary)
                        .or_else(|| members.iter().find(|m| directed(m)))
                        .map_or(s, |&m| self.selections[m]);
                    let backward = leader.head < leader.anchor;
                    *prev = if backward {
                        Selection {
                            anchor: hi,
                            head: lo,
                        }
                    } else {
                        Selection {
                            anchor: lo,
                            head: hi,
                        }
                    };
                },
                _ => merged.push((s, vec![i])),
            }
            slot[i] = merged.len() - 1;
        }
        self.primary = slot[primary];
        self.selections = merged.into_iter().map(|(s, _)| s).collect();
        slot[index]
    }
}

/// Whether `next`, which starts at or after `prev` in document order, must merge
/// with it: they overlap, or one of them is a caret on or inside the other.
fn collides(prev: Selection, next: Selection) -> bool {
    let (p, n) = (prev.range(), next.range());
    if prev.is_empty() || next.is_empty() {
        n.start <= p.end
    } else {
        n.start < p.end
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

    fn sel(anchor: u32, head: u32) -> Selection {
        Selection {
            anchor: LineCol::new(0, anchor),
            head: LineCol::new(0, head),
        }
    }

    #[test]
    fn normalize_keeps_touching_selections_apart() {
        // Adjacent `aa` matches in `aaaa`, and two backward selections that meet.
        for selections in [vec![sel(0, 2), sel(2, 4)], vec![sel(4, 0), sel(8, 4)]] {
            let mut cs = CursorState {
                selections: selections.clone(),
                primary: 1,
            };
            cs.normalize();
            assert_eq!(cs.selections, selections);
            assert_eq!(cs.primary, 1);
        }
    }

    #[test]
    fn normalize_merges_a_caret_on_a_selection_edge() {
        let mut cs = CursorState {
            selections: vec![sel(0, 0), sel(1, 0)],
            primary: 0,
        };
        cs.normalize();
        assert_eq!(cs.selections, vec![sel(1, 0)]);
        assert_eq!(cs.primary, 0);
    }

    #[test]
    fn normalize_keeps_the_direction_of_merged_selections() {
        // Two backward selections that overlap stay backward, head at the start.
        let mut cs = CursorState {
            selections: vec![sel(4, 0), sel(8, 3)],
            primary: 1,
        };
        cs.normalize();
        assert_eq!(cs.selections, vec![sel(8, 0)]);

        // Mixed directions: the primary's wins, whichever end it sits at.
        let mut cs = CursorState {
            selections: vec![sel(0, 4), sel(8, 3)],
            primary: 1,
        };
        cs.normalize();
        assert_eq!(cs.selections, vec![sel(8, 0)]);
        let mut cs = CursorState {
            selections: vec![sel(0, 4), sel(8, 3)],
            primary: 0,
        };
        cs.normalize();
        assert_eq!(cs.selections, vec![sel(0, 8)]);

        // Without the primary in the group, the first non-empty member leads.
        let mut cs = CursorState {
            selections: vec![sel(5, 2), sel(3, 7), sel(9, 9)],
            primary: 2,
        };
        cs.normalize();
        assert_eq!(cs.selections, vec![sel(7, 2), sel(9, 9)]);
        assert_eq!(cs.primary, 1);
    }

    #[test]
    fn normalize_sorts_and_tracks_the_primary_and_a_followed_index() {
        // Out of document order: the primary and the followed index move with
        // their selections, not with their positions in the vector.
        let mut cs = CursorState {
            selections: vec![sel(6, 6), sel(1, 1), sel(3, 3)],
            primary: 2,
        };
        assert_eq!(cs.normalize_tracking(0), 2);
        assert_eq!(cs.selections, vec![sel(1, 1), sel(3, 3), sel(6, 6)]);
        assert_eq!(cs.primary, 1);

        // A followed caret that merges lands on the merged selection.
        let mut cs = CursorState {
            selections: vec![sel(2, 2), sel(0, 0), sel(2, 2)],
            primary: 1,
        };
        assert_eq!(cs.normalize_tracking(2), 1);
        assert_eq!(cs.selections, vec![sel(0, 0), sel(2, 2)]);
        assert_eq!(cs.primary, 0);
        assert_eq!(CursorState::default().normalize_tracking(3), 0);

        // An out-of-range followed index and primary both clamp to the last
        // selection in the input, then move with it.
        let mut cs = CursorState {
            selections: vec![sel(5, 5), sel(1, 1)],
            primary: 9,
        };
        assert_eq!(cs.normalize_tracking(9), 0);
        assert_eq!(cs.selections, vec![sel(1, 1), sel(5, 5)]);
        assert_eq!(cs.primary, 0);
    }

    /// Whether `inner`'s range lies within `outer`'s.
    fn covers(outer: Selection, inner: Selection) -> bool {
        let (o, i) = (outer.range(), inner.range());
        o.start <= i.start && i.end <= o.end
    }

    /// Whether `a` and `b` would merge under normalize's collision rule, stated
    /// independently of the implementation's sorted-pair `collides`.
    fn overlap(a: Selection, b: Selection) -> bool {
        let (ra, rb) = (a.range(), b.range());
        if a.is_empty() || b.is_empty() {
            ra.start <= rb.end && rb.start <= ra.end
        } else {
            ra.start < rb.end && rb.start < ra.end
        }
    }

    /// Seeded generator over a small grid, so collisions of every shape are
    /// common: coincident carets, carets on edges, touching and nested ranges,
    /// both directions.
    #[test]
    fn normalize_holds_its_invariants_on_generated_cursor_sets() {
        let mut seed: u64 = 0x5eed_c0de;
        let mut next = |bound: u32| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            u32::try_from((seed >> 33) % u64::from(bound)).unwrap_or(0)
        };
        for _ in 0..2000 {
            let count = next(6) as usize + 1;
            let mut point = || LineCol::new(next(3), next(5));
            let input: Vec<Selection> = (0..count)
                .map(|_| Selection {
                    anchor: point(),
                    head: point(),
                })
                .collect();
            let primary = next(u32::try_from(count).unwrap_or(1)) as usize;
            let followed = next(u32::try_from(count).unwrap_or(1)) as usize;
            let mut cs = CursorState {
                selections: input.clone(),
                primary,
            };
            let slot = cs.normalize_tracking(followed);
            let out = &cs.selections;
            let context = format!("input {input:?} primary {primary} -> {out:?}");

            // Non-empty stays non-empty, and merging only ever shrinks the set.
            assert!(!out.is_empty() && out.len() <= input.len(), "{context}");
            // Document order, with no two survivors still colliding.
            for pair in out.windows(2) {
                assert!(pair[0].range().start <= pair[1].range().start, "{context}");
                assert!(!overlap(pair[0], pair[1]), "{context}");
            }
            // The primary and the followed selection land where they went.
            assert!(cs.primary < out.len(), "{context}");
            assert!(covers(out[cs.primary], input[primary]), "{context}");
            assert!(
                slot < out.len() && covers(out[slot], input[followed]),
                "{context}"
            );
            // Every input is inside a survivor, and every survivor is the union of
            // inputs: its ends are input ends, never invented positions.
            for s in &input {
                assert!(out.iter().any(|o| covers(*o, *s)), "{context}");
            }
            for o in out {
                let r = o.range();
                assert!(
                    input.iter().any(|s| s.range().start == r.start),
                    "{context}"
                );
                assert!(input.iter().any(|s| s.range().end == r.end), "{context}");
            }
            // A merged selection points the way its leader does: the primary when
            // it is a non-empty member, else the first non-empty member in document
            // order (by `(start, end)`, ties kept in input order).
            let backward = |s: &Selection| s.head < s.anchor;
            for (k, o) in out.iter().enumerate() {
                let first_directed = input
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| !s.is_empty() && covers(*o, **s))
                    .min_by_key(|(i, s)| (s.range().start, s.range().end, *i))
                    .map(|(_, s)| s);
                if k == cs.primary && !input[primary].is_empty() {
                    assert_eq!(backward(o), backward(&input[primary]), "{context}");
                } else if let Some(leader) = first_directed {
                    assert_eq!(backward(o), backward(leader), "{context}");
                }
            }
            // An input that collides with no other survives verbatim, direction
            // included.
            for (i, s) in input.iter().enumerate() {
                let alone = input
                    .iter()
                    .enumerate()
                    .all(|(j, t)| i == j || !overlap(*s, *t));
                if alone {
                    assert!(out.contains(s), "{context}");
                }
            }
            // Normalizing again changes nothing.
            let mut again = cs.clone();
            again.normalize();
            assert_eq!(again, cs, "{context}");
        }
    }

    #[test]
    fn workspace_edit_is_empty_only_without_edits_or_steps() {
        assert!(WorkspaceEdit::default().is_empty());
        // A file named with no edits changes nothing.
        let named = WorkspaceEdit {
            changes: vec![(PathBuf::from("/a.rs"), Vec::new())],
            ..WorkspaceEdit::default()
        };
        assert!(named.is_empty());
        // A file operation changes the workspace even with no text edit.
        let create = WorkspaceEdit {
            document_changes: vec![DocumentChange::Create {
                path: PathBuf::from("/b.rs"),
                options: FileOptions::default(),
            }],
            ..WorkspaceEdit::default()
        };
        assert!(!create.is_empty());
    }

    #[test]
    fn apply_result_constructors() {
        assert_eq!(
            ApplyWorkspaceEditResult::applied(),
            ApplyWorkspaceEditResult {
                applied: true,
                failure_reason: None,
                failed_change: None,
            }
        );
        let failed = ApplyWorkspaceEditResult::failed("stale", Some(2));
        assert!(!failed.applied);
        assert_eq!(failed.failure_reason.as_deref(), Some("stale"));
        assert_eq!(failed.failed_change, Some(2));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn edits_and_cursors_round_trip_through_serde() -> Result<(), serde_json::Error> {
        fn rt<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
            value: &T,
        ) -> Result<(), serde_json::Error> {
            let json = serde_json::to_string(value)?;
            assert_eq!(&serde_json::from_str::<T>(&json)?, value);
            Ok(())
        }
        let edit = TextEdit {
            range: Range {
                start: LineCol::new(0, 1),
                end: LineCol::new(2, 0),
            },
            new_text: "x\n".to_owned(),
        };
        let overwrite = FileOptions {
            overwrite: true,
            ignore_if_exists: false,
        };
        rt(&WorkspaceEdit {
            changes: vec![(PathBuf::from("a.rs"), vec![edit.clone()])],
            document_changes: vec![
                DocumentChange::Edit(DocumentEdit {
                    path: PathBuf::from("b.rs"),
                    version: Some(3),
                    edits: vec![edit.clone()],
                }),
                DocumentChange::Create {
                    path: PathBuf::from("c.rs"),
                    options: overwrite,
                },
                DocumentChange::Rename {
                    from: PathBuf::from("c.rs"),
                    to: PathBuf::from("d.rs"),
                    options: FileOptions::default(),
                },
                DocumentChange::Delete {
                    path: PathBuf::from("dir"),
                    options: DeleteOptions {
                        recursive: true,
                        ignore_if_not_exists: true,
                    },
                },
            ],
        })?;
        rt(&Change::new(9, vec![edit]))?;
        rt(&ApplyWorkspaceEditResult::failed("stale", Some(1)))?;
        rt(&CursorState {
            selections: vec![sel(0, 0), sel(9, 4)],
            primary: 1,
        })?;

        // A payload from before `document_changes` existed still decodes.
        let old = serde_json::from_str::<WorkspaceEdit>(r#"{"changes":[]}"#)?;
        assert_eq!(old, WorkspaceEdit::default());
        // A newer peer's cause decodes to `Unknown` instead of failing.
        assert_eq!(
            serde_json::from_str::<EditCause>("\"Dictation\"")?,
            EditCause::Unknown
        );
        Ok(())
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
