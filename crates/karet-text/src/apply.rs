//! Buffer mutation: applying a [`Change`], recording its inverse for undo, and
//! replaying undo/redo.
//!
//! The ordering is the whole game. Every edit's byte span is resolved against the
//! *pre-edit* rope first (so resolving a later edit isn't thrown off by an earlier
//! mutation), edits are then applied **descending by start byte** (so smaller
//! offsets stay valid), and each is reported as an [`AppliedEdit`] in tree-sitter
//! `InputEdit` shape — also descending — so a parse host can call `tree.edit` in
//! that order without per-edit delta bookkeeping.

use crate::history::EditContext;
use crate::{TextBuffer, TextError};
use karet_core::{BytePos, Change, CursorState, Range, TextEdit};

/// One edit as applied to the rope, in tree-sitter `InputEdit` shape. Points are
/// `(row, column-in-bytes)` — byte columns, **not** the `char` columns of
/// [`LineCol`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppliedEdit {
    /// Byte offset where the edit starts (same in old and new text).
    pub start_byte: usize,
    /// Byte offset of the end of the replaced region in the old text.
    pub old_end_byte: usize,
    /// Byte offset of the end of the inserted text in the new text.
    pub new_end_byte: usize,
    /// `(row, byte-column)` of `start_byte`.
    pub start_point: (usize, usize),
    /// `(row, byte-column)` of `old_end_byte` (old text).
    pub old_end_point: (usize, usize),
    /// `(row, byte-column)` of `new_end_byte` (new text).
    pub new_end_point: (usize, usize),
    /// The text that was replaced (the inverse insertion).
    pub replaced: String,
}

/// The result of an [`apply`](TextBuffer::apply), [`undo`](TextBuffer::undo), or
/// [`redo`](TextBuffer::redo): the new version and the per-edit ranges (descending
/// by `start_byte`, ready to feed `tree.edit`), plus an optional cursor to restore.
#[derive(Clone, Debug, Default)]
pub struct Applied {
    /// The buffer version after the operation.
    pub version: u64,
    /// The applied edits, descending by `start_byte`.
    pub edits: Vec<AppliedEdit>,
    /// The cursor to restore (set by [`undo`](TextBuffer::undo); `None` otherwise).
    pub restored_cursor: Option<CursorState>,
}

/// An edit resolved against the pre-edit rope (all coordinates in the pre-edit
/// frame).
struct Resolved {
    start_byte: usize,
    old_end_byte: usize,
    new_text: String,
    replaced: String,
    start_point: (usize, usize),
    old_end_point: (usize, usize),
    new_end_point: (usize, usize),
}

impl TextBuffer {
    /// Apply an atomic [`Change`], recording it for undo and returning the applied
    /// edits (for incremental reparse) and the new version.
    ///
    /// # Errors
    /// - [`TextError::StaleVersion`] if `change.base_version` is not the current
    ///   [`version`](Self::version).
    /// - [`TextError::OverlappingEdits`] if the batch's edits overlap.
    /// - [`TextError::OutOfBounds`] for an edit addressing a line past the end.
    pub fn apply(&mut self, change: &Change, ctx: EditContext) -> Result<Applied, TextError> {
        if change.base_version != self.version {
            return Err(TextError::StaleVersion);
        }
        let resolved = self.resolve(&change.edits)?;
        // Caret-after position for coalescing (single-edit insertions only).
        let edit_end_byte = match resolved.as_slice() {
            [single] => Some(single.start_byte + single.new_text.len()),
            _ => None,
        };
        let edits = self.mutate(&resolved);
        let inverse_edits = self.build_inverse(&resolved);
        self.version += 1;
        let inverse = Change {
            base_version: self.version,
            edits: inverse_edits,
        };
        let edit_end = edit_end_byte.map(|b| self.byte_to_line_col(BytePos(b)));
        self.history.record(inverse, change.clone(), &ctx, edit_end);
        Ok(Applied {
            version: self.version,
            edits,
            restored_cursor: None,
        })
    }

    /// Apply a change with default [`EditContext`] (its own undo step, no clock).
    ///
    /// # Errors
    /// As [`apply`](Self::apply).
    pub fn apply_simple(&mut self, change: &Change) -> Result<Applied, TextError> {
        self.apply(change, EditContext::default())
    }

    /// Undo the most recent edit group, returning the applied (inverse) edits and
    /// the cursor to restore, or `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<Applied> {
        let (changes, cursor) = self.history.take_undo()?;
        let edits = self.replay(&changes);
        self.version += 1;
        Some(Applied {
            version: self.version,
            edits,
            restored_cursor: Some(cursor),
        })
    }

    /// Redo the most recently undone edit group, or `None` when there is nothing
    /// to redo.
    pub fn redo(&mut self) -> Option<Applied> {
        let changes = self.history.take_redo()?;
        let edits = self.replay(&changes);
        self.version += 1;
        Some(Applied {
            version: self.version,
            edits,
            restored_cursor: None,
        })
    }

    /// Apply a sequence of trusted changes (history replay) to the rope, with no
    /// version check or history recording, collecting the applied edits in order.
    fn replay(&mut self, changes: &[Change]) -> Vec<AppliedEdit> {
        let mut edits = Vec::new();
        for ch in changes {
            if let Ok(resolved) = self.resolve(&ch.edits) {
                edits.extend(self.mutate(&resolved));
            }
        }
        edits
    }

    /// Resolve every edit against the current rope, sort ascending, and validate
    /// that the batch is non-overlapping.
    fn resolve(&self, edits: &[TextEdit]) -> Result<Vec<Resolved>, TextError> {
        let mut out = Vec::with_capacity(edits.len());
        for edit in edits {
            let start_byte = self.line_col_to_byte(edit.range.start)?.0;
            let old_end_byte = self.line_col_to_byte(edit.range.end)?.0;
            if old_end_byte < start_byte {
                return Err(TextError::OutOfBounds);
            }
            let replaced = self.rope.byte_slice(start_byte..old_end_byte).to_string();
            let start_point = self.byte_to_point(start_byte);
            let old_end_point = self.byte_to_point(old_end_byte);
            let new_end_point = point_after_insert(start_point, &edit.new_text);
            out.push(Resolved {
                start_byte,
                old_end_byte,
                new_text: edit.new_text.clone(),
                replaced,
                start_point,
                old_end_point,
                new_end_point,
            });
        }
        out.sort_by_key(|r| r.start_byte);
        for pair in out.windows(2) {
            if pair[1].start_byte < pair[0].old_end_byte {
                return Err(TextError::OverlappingEdits);
            }
        }
        Ok(out)
    }

    /// Mutate the rope, applying edits descending by start byte so smaller offsets
    /// stay valid, returning the applied edits (also descending).
    fn mutate(&mut self, resolved: &[Resolved]) -> Vec<AppliedEdit> {
        let mut applied = Vec::with_capacity(resolved.len());
        for r in resolved.iter().rev() {
            let start_char = self.rope.byte_to_char(r.start_byte);
            let old_end_char = self.rope.byte_to_char(r.old_end_byte);
            // In-bounds by construction (resolved against this rope, applied high
            // offset first); ignore the panic-free Result rather than unwrap.
            if old_end_char > start_char {
                let _ = self.rope.try_remove(start_char..old_end_char);
            }
            if !r.new_text.is_empty() {
                let _ = self.rope.try_insert(start_char, &r.new_text);
            }
            applied.push(AppliedEdit {
                start_byte: r.start_byte,
                old_end_byte: r.old_end_byte,
                new_end_byte: r.start_byte + r.new_text.len(),
                start_point: r.start_point,
                old_end_point: r.old_end_point,
                new_end_point: r.new_end_point,
                replaced: r.replaced.clone(),
            });
        }
        applied
    }

    /// Build the inverse edits (post-edit coordinate frame) using one ascending
    /// running-delta pass. Must be called after [`mutate`](Self::mutate).
    fn build_inverse(&self, resolved: &[Resolved]) -> Vec<TextEdit> {
        let mut inverse = Vec::with_capacity(resolved.len());
        let mut delta: i64 = 0;
        for r in resolved {
            let new_len = r.new_text.len();
            let old_len = r.old_end_byte - r.start_byte;
            let post_start = (r.start_byte as i64 + delta) as usize;
            let post_end = post_start + new_len;
            inverse.push(TextEdit {
                range: Range {
                    start: self.byte_to_line_col(BytePos(post_start)),
                    end: self.byte_to_line_col(BytePos(post_end)),
                },
                new_text: r.replaced.clone(),
            });
            delta += new_len as i64 - old_len as i64;
        }
        inverse
    }
}

/// The tree-sitter point at the end of inserting `new_text` starting at `start`.
fn point_after_insert(start: (usize, usize), new_text: &str) -> (usize, usize) {
    let bytes = new_text.as_bytes();
    match memchr::memrchr(b'\n', bytes) {
        Some(last_nl) => {
            let nlines = memchr::memchr_iter(b'\n', bytes).count();
            (start.0 + nlines, bytes.len() - last_nl - 1)
        }
        None => (start.0, start.1 + bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use karet_core::LineCol;

    fn ins(version: u64, line: u32, col: u32, text: &str) -> Change {
        Change::new(
            version,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(line, col),
                    end: LineCol::new(line, col),
                },
                new_text: text.to_string(),
            }],
        )
    }

    fn del(version: u64, range: Range) -> Change {
        Change::new(
            version,
            vec![TextEdit {
                range,
                new_text: String::new(),
            }],
        )
    }

    #[test]
    fn insert_bumps_version_and_text() {
        let mut b = TextBuffer::from_text("helloworld");
        let r = b.apply_simple(&ins(0, 0, 5, ", ")).unwrap_or_default();
        assert_eq!(r.version, 1);
        assert_eq!(b.version(), 1);
        assert!(b.is_dirty());
        assert_eq!(b.line(0).as_deref(), Some("hello, world"));
    }

    #[test]
    fn stale_version_rejected() {
        let mut b = TextBuffer::from_text("x");
        assert_eq!(
            b.apply_simple(&ins(7, 0, 0, "a")).err(),
            Some(TextError::StaleVersion)
        );
    }

    #[test]
    fn delete_removes_range() {
        let mut b = TextBuffer::from_text("hello, world");
        let range = Range {
            start: LineCol::new(0, 5),
            end: LineCol::new(0, 7),
        };
        assert!(b.apply_simple(&del(0, range)).is_ok());
        assert_eq!(b.line(0).as_deref(), Some("helloworld"));
    }

    #[test]
    fn multibyte_insert_keeps_bytes_aligned() {
        let mut b = TextBuffer::from_text("aé");
        // Insert after 'é' (char col 2, byte 3).
        assert!(b.apply_simple(&ins(0, 0, 2, "ß")).is_ok());
        assert_eq!(b.line(0).as_deref(), Some("aéß"));
        assert_eq!(b.len_bytes(), 5); // a=1 é=2 ß=2
    }

    #[test]
    fn multi_edit_applies_all_non_overlapping() {
        let mut b = TextBuffer::from_text("abcdef");
        // Two non-overlapping inserts in one atomic change.
        let change = Change::new(
            0,
            vec![
                TextEdit {
                    range: Range {
                        start: LineCol::new(0, 1),
                        end: LineCol::new(0, 1),
                    },
                    new_text: "X".to_string(),
                },
                TextEdit {
                    range: Range {
                        start: LineCol::new(0, 4),
                        end: LineCol::new(0, 4),
                    },
                    new_text: "Y".to_string(),
                },
            ],
        );
        assert!(b.apply_simple(&change).is_ok());
        assert_eq!(b.line(0).as_deref(), Some("aXbcdYef"));
    }

    #[test]
    fn overlapping_edits_rejected() {
        let mut b = TextBuffer::from_text("abcdef");
        let change = Change::new(
            0,
            vec![
                TextEdit {
                    range: Range {
                        start: LineCol::new(0, 1),
                        end: LineCol::new(0, 3),
                    },
                    new_text: "X".to_string(),
                },
                TextEdit {
                    range: Range {
                        start: LineCol::new(0, 2),
                        end: LineCol::new(0, 4),
                    },
                    new_text: "Y".to_string(),
                },
            ],
        );
        assert_eq!(
            b.apply_simple(&change).err(),
            Some(TextError::OverlappingEdits)
        );
    }

    #[test]
    fn undo_redo_restores_content() {
        let mut b = TextBuffer::from_text("hello");
        assert!(b.apply_simple(&ins(0, 0, 5, " world")).is_ok());
        assert_eq!(b.line(0).as_deref(), Some("hello world"));
        let undone = b.undo().unwrap_or_default();
        assert_eq!(b.line(0).as_deref(), Some("hello"));
        assert!(undone.restored_cursor.is_some());
        assert!(b.redo().is_some());
        assert_eq!(b.line(0).as_deref(), Some("hello world"));
    }

    #[test]
    fn coalesced_typing_undoes_as_one_step() {
        let mut b = TextBuffer::from_text("");
        let mut tick = 0;
        for (i, ch) in "abc".chars().enumerate() {
            let col = i as u32;
            let ctx = EditContext {
                tick_ms: tick,
                cause: crate::EditCause::Type,
                cursor_before: CursorState::single(karet_core::Selection::caret(LineCol::new(
                    0, col,
                ))),
            };
            assert!(
                b.apply(&ins(b.version(), 0, col, &ch.to_string()), ctx)
                    .is_ok()
            );
            tick += 50;
        }
        assert_eq!(b.line(0).as_deref(), Some("abc"));
        // One undo removes the whole coalesced word.
        assert!(b.undo().is_some());
        assert_eq!(b.line(0).as_deref(), Some(""));
        assert!(!b.is_dirty());
    }

    #[test]
    fn applied_edit_reports_tree_sitter_points() {
        let mut b = TextBuffer::from_text("ab");
        // Insert a newline-containing string after 'a' (char col 1, byte 1).
        let applied = b.apply_simple(&ins(0, 0, 1, "X\nY")).unwrap_or_default();
        assert_eq!(applied.edits.len(), 1);
        let e = applied.edits.first().cloned().unwrap_or_default();
        assert_eq!(e.start_byte, 1);
        assert_eq!(e.old_end_byte, 1); // pure insertion
        assert_eq!(e.new_end_byte, 4); // 1 + len("X\nY")
        assert_eq!(e.start_point, (0, 1));
        assert_eq!(e.new_end_point, (1, 1)); // one new line, then "Y" → byte col 1
        assert_eq!(b.line(0).as_deref(), Some("aX"));
        assert_eq!(b.line(1).as_deref(), Some("Yb"));
    }
}
