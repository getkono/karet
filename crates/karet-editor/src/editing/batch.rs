//! Composing per-caret edits into one atomic multi-caret batch.
//!
//! Each selection builds its own [`Edit`] against the same base buffer. The
//! batch the buffer applies must be non-overlapping, so edits that overlap (two
//! carets deleting a word back to the same boundary, say) are merged into one
//! edit covering both, rather than letting the whole batch fail. A selection
//! whose edit is a no-op keeps its place, shifted by the edits before it, so
//! no caret is ever dropped.

use karet_core::Change;
use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::Selection;
use karet_core::TextEdit;

use super::Edit;
use super::MultiEdit;
use super::caret_after_insert;
use super::reflow_caret;

/// One merged edit of the batch and the selections that contributed to it.
struct Group {
    edit: TextEdit,
    owners: Vec<usize>,
}

/// Compose one [`Edit`] per selection into a single non-overlapping batch over
/// `base`, with the cursor set it leaves behind.
///
/// `items` holds every selection of the cursor set, in its order, with the edit
/// it built (`None` where the operation is a no-op for that selection, such as
/// Backspace at the start of the buffer). `primary` indexes `items`.
///
/// Overlapping edits merge into one edit spanning the union of their ranges,
/// whose text is theirs concatenated in document order; every caret of a merged
/// edit lands at the end of that text. A selection without an edit is kept,
/// both ends moved through the batch by [`map_position`]. Each other caret lands
/// where its own edit put it, shifted by the edits before it. The returned
/// cursor set keeps one selection per item, in the same order, so `primary`
/// still names the same caret.
///
/// Returns `None` when no selection built an edit with any text change.
#[must_use]
pub fn compose(
    base: u64,
    primary: usize,
    items: &[(Selection, Option<Edit>)],
) -> Option<MultiEdit> {
    let mut flat: Vec<(TextEdit, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(owner, (_, edit))| edit.as_ref().map(|e| (owner, e)))
        .flat_map(|(owner, e)| e.change.edits.iter().cloned().map(move |t| (t, owner)))
        .collect();
    if flat.is_empty() {
        return None;
    }
    flat.sort_by_key(|(t, owner)| (t.range.start, t.range.end, *owner));
    let groups = merge(flat);
    let edits: Vec<TextEdit> = groups.iter().map(|g| g.edit.clone()).collect();
    let selections = items
        .iter()
        .enumerate()
        .map(|(owner, (selection, edit))| {
            let first = groups.iter().position(|g| g.owners.contains(&owner));
            match (edit, first) {
                (Some(edit), Some(first)) => {
                    let merged = groups
                        .iter()
                        .position(|g| g.owners.len() > 1 && g.owners.contains(&owner));
                    let (local, k) = match merged {
                        Some(m) => {
                            let group = &groups[m].edit;
                            (caret_after_insert(group.range.start, &group.new_text), m)
                        },
                        None => (edit.caret, first),
                    };
                    Selection::caret(reflow_caret(local, &edits[..k]))
                },
                // An edit with no text change still places its caret.
                (Some(edit), None) => Selection::caret(map_position(edit.caret, &edits)),
                (None, _) => map_selection(*selection, &edits),
            }
        })
        .collect();
    Some(MultiEdit {
        change: Change::new(base, edits),
        cursors: CursorState {
            selections,
            primary: primary.min(items.len().saturating_sub(1)),
        },
    })
}

/// Merge sorted edits whose ranges overlap into one edit each, remembering every
/// selection that contributed. Edits that only touch stay apart, as the buffer
/// accepts them.
fn merge(sorted: Vec<(TextEdit, usize)>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::with_capacity(sorted.len());
    for (edit, owner) in sorted {
        match groups.last_mut() {
            Some(last) if edit.range.start < last.edit.range.end => {
                last.edit.range.end = last.edit.range.end.max(edit.range.end);
                last.edit.new_text.push_str(&edit.new_text);
                if !last.owners.contains(&owner) {
                    last.owners.push(owner);
                }
            },
            _ => groups.push(Group {
                edit,
                owners: vec![owner],
            }),
        }
    }
    groups
}

/// Where `pos`, a position in the buffer before `edits` apply, sits after they
/// apply. `edits` must be sorted and non-overlapping, as [`compose`] builds them.
///
/// A position at or after an edit's end moves with the text around it (an
/// insertion exactly at `pos` pushes it along); one strictly inside a replaced
/// range lands at the end of the replacement text.
#[must_use]
pub fn map_position(pos: LineCol, edits: &[TextEdit]) -> LineCol {
    let before = edits.iter().take_while(|e| e.range.end <= pos).count();
    match edits.get(before) {
        Some(e) if e.range.start < pos => {
            let landed = caret_after_insert(e.range.start, &e.new_text);
            reflow_caret(landed, &edits[..before])
        },
        _ => reflow_caret(pos, &edits[..before]),
    }
}

/// [`map_position`] over both ends of `selection`, keeping its direction.
#[must_use]
pub fn map_selection(selection: Selection, edits: &[TextEdit]) -> Selection {
    Selection {
        anchor: map_position(selection.anchor, edits),
        head: map_position(selection.head, edits),
    }
}

#[cfg(test)]
mod tests {
    use karet_core::Range;

    use super::*;

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn delete(start: LineCol, end: LineCol) -> Edit {
        Edit {
            change: Change::new(
                0,
                vec![TextEdit {
                    range: Range { start, end },
                    new_text: String::new(),
                }],
            ),
            caret: start,
        }
    }

    fn insert(pos: LineCol, text: &str) -> Edit {
        Edit {
            change: Change::new(
                0,
                vec![TextEdit {
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                    new_text: text.to_string(),
                }],
            ),
            caret: caret_after_insert(pos, text),
        }
    }

    fn caret(pos: LineCol) -> Selection {
        Selection::caret(pos)
    }

    /// [`compose`] where the case builds at least one edit.
    fn composed(base: u64, primary: usize, items: &[(Selection, Option<Edit>)]) -> MultiEdit {
        let out = compose(base, primary, items);
        assert!(out.is_some(), "an edit was built");
        out.unwrap_or_else(|| MultiEdit {
            change: Change::new(base, Vec::new()),
            cursors: CursorState::single(caret(at(0, 0))),
        })
    }

    fn heads(edit: &MultiEdit) -> Vec<LineCol> {
        edit.cursors.selections.iter().map(|s| s.head).collect()
    }

    #[test]
    fn nothing_to_apply_is_none() {
        let items = vec![(caret(at(0, 0)), None), (caret(at(1, 0)), None)];
        assert!(compose(3, 0, &items).is_none());
    }

    #[test]
    fn a_no_op_caret_is_kept_and_shifted() {
        // Backspace with carets at (0,0) and (1,3): only the second deletes.
        let items = vec![
            (caret(at(0, 0)), None),
            (caret(at(1, 3)), Some(delete(at(1, 2), at(1, 3)))),
        ];
        let out = composed(7, 1, &items);
        assert_eq!(out.change.base_version, 7);
        assert_eq!(heads(&out), vec![at(0, 0), at(1, 2)]);
        assert_eq!(out.cursors.primary, 1);
    }

    #[test]
    fn a_no_op_caret_after_an_edit_moves_with_the_text() {
        // Delete at the end of the buffer is a no-op; the edit before it on the
        // same line pulls it left.
        let items = vec![
            (caret(at(0, 1)), Some(delete(at(0, 1), at(0, 2)))),
            (caret(at(0, 4)), None),
        ];
        let out = composed(0, 0, &items);
        assert_eq!(heads(&out), vec![at(0, 1), at(0, 3)]);
    }

    #[test]
    fn a_no_op_selection_keeps_its_extent() {
        let items = vec![
            (caret(at(0, 0)), Some(insert(at(0, 0), "xy"))),
            (
                Selection {
                    anchor: at(0, 5),
                    head: at(0, 3),
                },
                None,
            ),
        ];
        let out = composed(0, 0, &items);
        assert_eq!(
            out.cursors.selections[1],
            Selection {
                anchor: at(0, 7),
                head: at(0, 5),
            }
        );
    }

    #[test]
    fn overlapping_deletes_merge_into_their_union() {
        // `abcdefg`, carets at 3 and 5, Ctrl+Backspace: both reach column 0.
        let items = vec![
            (caret(at(0, 3)), Some(delete(at(0, 0), at(0, 3)))),
            (caret(at(0, 5)), Some(delete(at(0, 0), at(0, 5)))),
        ];
        let out = composed(0, 1, &items);
        assert_eq!(out.change.edits.len(), 1);
        assert_eq!(
            out.change.edits[0].range,
            Range {
                start: at(0, 0),
                end: at(0, 5),
            }
        );
        assert_eq!(heads(&out), vec![at(0, 0), at(0, 0)]);
        assert_eq!(out.cursors.primary, 1);
    }

    #[test]
    fn partially_overlapping_edits_concatenate_their_text() {
        let replace = |start: LineCol, end: LineCol, text: &str| Edit {
            change: Change::new(
                0,
                vec![TextEdit {
                    range: Range { start, end },
                    new_text: text.to_string(),
                }],
            ),
            caret: caret_after_insert(start, text),
        };
        let items = vec![
            (caret(at(0, 1)), Some(replace(at(0, 1), at(0, 4), "X"))),
            (caret(at(0, 3)), Some(replace(at(0, 3), at(0, 6), "Y"))),
            (caret(at(0, 9)), Some(insert(at(0, 9), "Z"))),
        ];
        let out = composed(0, 0, &items);
        assert_eq!(out.change.edits.len(), 2);
        assert_eq!(out.change.edits[0].new_text, "XY");
        assert_eq!(
            out.change.edits[0].range,
            Range {
                start: at(0, 1),
                end: at(0, 6),
            }
        );
        // `1..6` became `XY` (net -3), so the third caret lands at 9 + 1 - 3.
        assert_eq!(heads(&out), vec![at(0, 3), at(0, 3), at(0, 7)]);
    }

    #[test]
    fn touching_edits_stay_apart() {
        let items = vec![
            (caret(at(0, 1)), Some(delete(at(0, 0), at(0, 1)))),
            (caret(at(0, 2)), Some(delete(at(0, 1), at(0, 2)))),
        ];
        let out = composed(0, 0, &items);
        assert_eq!(out.change.edits.len(), 2);
        assert_eq!(heads(&out), vec![at(0, 0), at(0, 0)]);
    }

    #[test]
    fn map_position_inside_a_replacement_lands_after_it() {
        let edits = vec![TextEdit {
            range: Range {
                start: at(0, 2),
                end: at(1, 1),
            },
            new_text: "ab".to_string(),
        }];
        assert_eq!(map_position(at(0, 5), &edits), at(0, 4));
        assert_eq!(map_position(at(0, 2), &edits), at(0, 2));
        assert_eq!(map_position(at(1, 4), &edits), at(0, 7));
        assert_eq!(map_position(at(2, 1), &edits), at(1, 1));
    }

    #[test]
    fn map_position_after_an_insertion_at_it() {
        let edits = vec![TextEdit {
            range: Range {
                start: at(0, 2),
                end: at(0, 2),
            },
            new_text: "x\ny".to_string(),
        }];
        assert_eq!(map_position(at(0, 2), &edits), at(1, 1));
        assert_eq!(map_position(at(0, 1), &edits), at(0, 1));
    }

    #[test]
    fn map_selection_maps_both_ends_and_keeps_direction() {
        let edits = vec![TextEdit {
            range: Range {
                start: at(0, 0),
                end: at(0, 1),
            },
            new_text: String::new(),
        }];
        let backward = Selection {
            anchor: at(0, 5),
            head: at(0, 3),
        };
        assert_eq!(
            map_selection(backward, &edits),
            Selection {
                anchor: at(0, 4),
                head: at(0, 2),
            }
        );
    }

    #[test]
    fn a_caret_whose_second_edit_merges_lands_after_the_merged_edit() {
        // The first caret edits twice; only its second edit overlaps the other
        // caret's, so it lands after the merged edit, not after its first one.
        let first = Edit {
            change: Change::new(
                0,
                vec![
                    TextEdit {
                        range: Range {
                            start: at(0, 0),
                            end: at(0, 0),
                        },
                        new_text: "(".to_string(),
                    },
                    TextEdit {
                        range: Range {
                            start: at(0, 2),
                            end: at(0, 4),
                        },
                        new_text: String::new(),
                    },
                ],
            ),
            caret: at(0, 3),
        };
        let items = vec![
            (caret(at(0, 2)), Some(first)),
            (caret(at(0, 5)), Some(delete(at(0, 3), at(0, 5)))),
        ];
        let out = composed(0, 0, &items);
        assert_eq!(out.change.edits.len(), 2);
        // `(` shifts the merged `2..5` deletion's landing one column right.
        assert_eq!(heads(&out), vec![at(0, 3), at(0, 3)]);
    }
}
