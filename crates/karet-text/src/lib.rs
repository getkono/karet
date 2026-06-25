//! `karet-text` — a headless text-editing model for the karet toolkit.
//!
//! A rope-backed [`TextBuffer`] with editing history plus a cursor/selection model
//! (the [`cursor`] module), usable by any editor backend (TUI or otherwise)
//! without pulling in rendering dependencies. It is the one place that converts
//! between byte offsets and line/column positions, since that requires the rope.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! editing/undo/conversion logic is filled in separately.

use karet_core::{BytePos, Change, LineCol};
use std::io::Read;
use std::path::Path;

/// Errors produced by [`TextBuffer`] operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TextError {
    /// An I/O error while reading or saving a buffer.
    #[error("text i/o error: {0}")]
    Io(String),
    /// A coordinate fell outside the buffer.
    #[error("position out of bounds")]
    OutOfBounds,
    /// A change was applied against a stale document version.
    #[error("change applied against a stale buffer version")]
    StaleVersion,
}

/// A headless, editable text buffer backed by a rope.
///
/// Tracks a monotonically increasing edit `version` (used to validate
/// [`Change`]s) and a dirty flag for unsaved edits.
#[derive(Clone, Default)]
pub struct TextBuffer {
    rope: ropey::Rope,
    version: u64,
    dirty: bool,
}

impl TextBuffer {
    /// Create an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a buffer from in-memory text.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: ropey::Rope::from_str(text),
            version: 0,
            dirty: false,
        }
    }

    /// Create a buffer by reading all of `reader`.
    ///
    /// # Errors
    /// Returns [`TextError::Io`] if reading fails.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, TextError> {
        let _ = reader;
        todo!()
    }

    /// The total length of the buffer in bytes.
    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The number of lines in the buffer.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// The current edit version (incremented on every applied change).
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whether the buffer has unsaved changes.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Apply an atomic [`Change`], bumping the version and dirty flag.
    ///
    /// # Errors
    /// Returns [`TextError::StaleVersion`] if `change.base_version` does not match
    /// the current [`version`](Self::version), or [`TextError::OutOfBounds`] for an
    /// out-of-range edit.
    pub fn apply(&mut self, change: &Change) -> Result<(), TextError> {
        let _ = change;
        todo!()
    }

    /// Undo the most recent change, returning the change that was reverted.
    pub fn undo(&mut self) -> Option<Change> {
        todo!()
    }

    /// Redo the most recently undone change.
    pub fn redo(&mut self) -> Option<Change> {
        todo!()
    }

    /// Convert an absolute byte offset to a line/column position.
    #[must_use]
    pub fn byte_to_line_col(&self, byte: BytePos) -> LineCol {
        let _ = byte;
        todo!()
    }

    /// Convert a line/column position to an absolute byte offset.
    ///
    /// # Errors
    /// Returns [`TextError::OutOfBounds`] if the position is past the buffer end.
    pub fn line_col_to_byte(&self, pos: LineCol) -> Result<BytePos, TextError> {
        let _ = pos;
        todo!()
    }

    /// Save the buffer to `path`, clearing the dirty flag on success.
    ///
    /// # Errors
    /// Returns [`TextError::Io`] if writing fails.
    pub fn save(&mut self, path: &Path) -> Result<(), TextError> {
        let _ = path;
        todo!()
    }
}

/// Cursor and selection behavior built on the neutral [`karet_core::edit`] types.
pub mod cursor {
    use super::TextBuffer;
    use karet_core::edit::CursorState;

    /// The direction of a motion.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Direction {
        /// Toward the end of the buffer.
        Forward,
        /// Toward the start of the buffer.
        Backward,
    }

    /// A multi-cursor with per-cursor desired-column memory, built on
    /// [`CursorState`].
    #[derive(Clone, Debug, Default)]
    pub struct MultiCursor {
        state: CursorState,
    }

    impl MultiCursor {
        /// Create a multi-cursor from an initial [`CursorState`].
        #[must_use]
        pub fn new(state: CursorState) -> Self {
            Self { state }
        }

        /// The underlying selection state.
        #[must_use]
        pub fn state(&self) -> &CursorState {
            &self.state
        }

        /// Move every cursor by one word in `dir`.
        pub fn move_word(&mut self, buf: &TextBuffer, dir: Direction) {
            let _ = (buf, dir);
            todo!()
        }

        /// Expand every selection to its enclosing bracket pair.
        pub fn expand_to_bracket(&mut self, buf: &TextBuffer) {
            let _ = buf;
            todo!()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_basics() {
        let b = TextBuffer::from_text("hello\nworld");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.version(), 0);
        assert!(!b.is_dirty());
        assert_eq!(b.len_bytes(), 11);
    }

    #[test]
    fn error_displays() {
        assert_eq!(TextError::OutOfBounds.to_string(), "position out of bounds");
    }
}
