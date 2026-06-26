//! Side-by-side alignment: turn a flat hunk into paired left/right rows.

use crate::model::{DiffLine, LineKind};

/// One side (left or right) of a [`SideBySideRow`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The 1-based line number on this side (`0` when unknown).
    pub lineno: u32,
    /// The line text.
    pub content: String,
    /// Whether this cell is context, an addition, or a removal.
    pub kind: LineKind,
    /// Index of this line within the parent hunk's `lines` vec (for span lookup).
    pub hunk_line_idx: usize,
}

/// A single row of the side-by-side view: an optional old cell and new cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideBySideRow {
    /// The left (old) cell, if any.
    pub left: Option<Cell>,
    /// The right (new) cell, if any.
    pub right: Option<Cell>,
}

/// Align a flat list of [`DiffLine`]s (from a single hunk) into paired rows.
///
/// Context lines appear on both sides; consecutive removals are paired 1:1 with the
/// consecutive additions that follow them, raggedly when the counts differ.
#[must_use]
pub fn align_hunk(lines: &[DiffLine]) -> Vec<SideBySideRow> {
    let mut rows = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = &lines[i];

        match line.kind {
            LineKind::Context => {
                rows.push(SideBySideRow {
                    left: Some(Cell {
                        lineno: line.old_lineno.unwrap_or(0),
                        content: line.content.clone(),
                        kind: LineKind::Context,
                        hunk_line_idx: i,
                    }),
                    right: Some(Cell {
                        lineno: line.new_lineno.unwrap_or(0),
                        content: line.content.clone(),
                        kind: LineKind::Context,
                        hunk_line_idx: i,
                    }),
                });
                i += 1;
            }
            LineKind::Remove | LineKind::Add => {
                // Collect the consecutive Remove block...
                let remove_start = i;
                while i < lines.len() && lines[i].kind == LineKind::Remove {
                    i += 1;
                }
                let remove_end = i;

                // ...then the consecutive Add block immediately following.
                let add_start = i;
                while i < lines.len() && lines[i].kind == LineKind::Add {
                    i += 1;
                }
                let add_end = i;

                let removes = &lines[remove_start..remove_end];
                let adds = &lines[add_start..add_end];

                let max_len = removes.len().max(adds.len());
                for j in 0..max_len {
                    let left = removes.get(j).map(|l| Cell {
                        lineno: l.old_lineno.unwrap_or(0),
                        content: l.content.clone(),
                        kind: LineKind::Remove,
                        hunk_line_idx: remove_start + j,
                    });
                    let right = adds.get(j).map(|l| Cell {
                        lineno: l.new_lineno.unwrap_or(0),
                        content: l.content.clone(),
                        kind: LineKind::Add,
                        hunk_line_idx: add_start + j,
                    });
                    rows.push(SideBySideRow { left, right });
                }
            }
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(old: u32, new: u32, s: &str) -> DiffLine {
        DiffLine {
            kind: LineKind::Context,
            old_lineno: Some(old),
            new_lineno: Some(new),
            content: s.to_string(),
        }
    }
    fn rem(old: u32, s: &str) -> DiffLine {
        DiffLine {
            kind: LineKind::Remove,
            old_lineno: Some(old),
            new_lineno: None,
            content: s.to_string(),
        }
    }
    fn add(new: u32, s: &str) -> DiffLine {
        DiffLine {
            kind: LineKind::Add,
            old_lineno: None,
            new_lineno: Some(new),
            content: s.to_string(),
        }
    }

    #[test]
    fn context_only() {
        let rows = align_hunk(&[ctx(1, 1, "a"), ctx(2, 2, "b")]);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].left.is_some() && rows[0].right.is_some());
    }

    #[test]
    fn equal_remove_add() {
        let rows = align_hunk(&[rem(1, "old"), add(1, "new")]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].left.as_ref().map(|c| c.content.as_str()),
            Some("old")
        );
        assert_eq!(
            rows[0].right.as_ref().map(|c| c.content.as_str()),
            Some("new")
        );
    }

    #[test]
    fn more_removes_than_adds() {
        let rows = align_hunk(&[rem(1, "r1"), rem(2, "r2"), add(1, "a1")]);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].left.is_some() && rows[0].right.is_some());
        assert!(rows[1].left.is_some() && rows[1].right.is_none());
    }

    #[test]
    fn more_adds_than_removes() {
        let rows = align_hunk(&[rem(1, "r1"), add(1, "a1"), add(2, "a2")]);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].left.is_some() && rows[0].right.is_some());
        assert!(rows[1].left.is_none() && rows[1].right.is_some());
    }

    #[test]
    fn lone_remove_and_lone_add() {
        let rows = align_hunk(&[rem(1, "only removed")]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].left.is_some() && rows[0].right.is_none());

        let rows = align_hunk(&[add(1, "only added")]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].left.is_none() && rows[0].right.is_some());
    }

    #[test]
    fn mixed_context_and_changes() {
        let rows = align_hunk(&[
            ctx(1, 1, "ctx"),
            rem(2, "old"),
            add(2, "new"),
            ctx(3, 3, "ctx2"),
        ]);
        assert_eq!(rows.len(), 3);
        assert!(rows[0].left.is_some() && rows[0].right.is_some());
        assert!(rows[1].left.is_some() && rows[1].right.is_some());
        assert!(rows[2].left.is_some() && rows[2].right.is_some());
    }
}
