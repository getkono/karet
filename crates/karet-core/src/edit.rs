//! Neutral, serializable edit and selection types.
//!
//! These describe *what changed* and *where the cursors are* in line/column space,
//! independent of the rope. `karet-text` applies them; the presentation layer and
//! the client-server seam exchange them.

use crate::coord::{LineCol, Range};
use std::path::PathBuf;

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
}
