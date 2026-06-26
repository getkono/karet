//! `karet-text` — a headless text-editing model for the karet toolkit.
//!
//! A rope-backed [`TextBuffer`] with editing history plus a cursor/selection model
//! (the [`cursor`] module), usable by any editor backend (TUI or otherwise)
//! without pulling in rendering dependencies. It is the one place that converts
//! between byte offsets and line/column positions, since that requires the rope.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! editing/undo/conversion logic is filled in separately.

use karet_core::{BytePos, Change, LineCol, Span};
use std::io::Read;
use std::path::Path;

/// Errors produced by [`TextBuffer`] operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
        let rope = ropey::Rope::from_reader(reader).map_err(|e| TextError::Io(e.to_string()))?;
        Ok(Self {
            rope,
            version: 0,
            dirty: false,
        })
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
    ///
    /// The column is counted in Unicode scalar values (`char`s), matching karet's
    /// internal [`PositionEncoding::Utf32`](karet_core::PositionEncoding). An offset
    /// past the end of the buffer is clamped to the buffer end.
    #[must_use]
    pub fn byte_to_line_col(&self, byte: BytePos) -> LineCol {
        let b = byte.0.min(self.rope.len_bytes());
        let line = self.rope.byte_to_line(b);
        let line_start_char = self.rope.line_to_char(line);
        let char_idx = self.rope.byte_to_char(b);
        LineCol {
            line: line as u32,
            col: (char_idx - line_start_char) as u32,
        }
    }

    /// Convert a line/column position to an absolute byte offset.
    ///
    /// `col` is interpreted in `char`s and clamped to the line's content length
    /// (excluding the trailing line break).
    ///
    /// # Errors
    /// Returns [`TextError::OutOfBounds`] if the line is past the buffer end.
    pub fn line_col_to_byte(&self, pos: LineCol) -> Result<BytePos, TextError> {
        let line = pos.line as usize;
        if line >= self.rope.len_lines() {
            return Err(TextError::OutOfBounds);
        }
        let line_start_char = self.rope.line_to_char(line);
        let content_chars = line_content_chars(self.rope.line(line));
        let col = (pos.col as usize).min(content_chars);
        Ok(BytePos(self.rope.char_to_byte(line_start_char + col)))
    }

    /// The text of line `idx` (zero-based) without its trailing line break, or
    /// `None` when `idx` is past the last line.
    #[must_use]
    pub fn line(&self, idx: usize) -> Option<String> {
        if idx >= self.rope.len_lines() {
            return None;
        }
        let line = self.rope.line(idx);
        let end = line_content_chars(line);
        Some(line.slice(..end).to_string())
    }

    /// The byte [`Span`] of line `idx`'s content — `[line_start, content_end)`,
    /// excluding the trailing line break — or `None` when `idx` is past the last
    /// line. Suitable for indexing highlight spans line by line.
    #[must_use]
    pub fn line_to_byte_range(&self, idx: usize) -> Option<Span> {
        if idx >= self.rope.len_lines() {
            return None;
        }
        let start_char = self.rope.line_to_char(idx);
        let content_chars = line_content_chars(self.rope.line(idx));
        Some(Span {
            start: BytePos(self.rope.char_to_byte(start_char)),
            end: BytePos(self.rope.char_to_byte(start_char + content_chars)),
        })
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

/// The number of `char`s in `line`, excluding a trailing `\n` or `\r\n`.
fn line_content_chars(line: ropey::RopeSlice<'_>) -> usize {
    let mut end = line.len_chars();
    if end > 0 && line.char(end - 1) == '\n' {
        end -= 1;
        if end > 0 && line.char(end - 1) == '\r' {
            end -= 1;
        }
    }
    end
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

    #[test]
    fn from_reader_reads_utf8() {
        let result = TextBuffer::from_reader(std::io::Cursor::new("abc\ndéf"));
        assert!(result.is_ok());
        let b = result.unwrap_or_default();
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(0).as_deref(), Some("abc"));
        assert_eq!(b.line(1).as_deref(), Some("déf"));
    }

    #[test]
    fn coord_roundtrip_multibyte() {
        // "héllo\nwörld": 'é' and 'ö' are two bytes each; columns count chars.
        let b = TextBuffer::from_text("héllo\nwörld");
        // The 'o' ending line 0 is char 4, byte 5 (h=1, é=2, l=1, l=1).
        assert_eq!(b.line_col_to_byte(LineCol::new(0, 4)), Ok(BytePos(5)));
        assert_eq!(b.byte_to_line_col(BytePos(5)), LineCol::new(0, 4));
        // Start of line 1 is byte 7 ("héllo\n" = 6 + 1).
        assert_eq!(b.line_col_to_byte(LineCol::new(1, 0)), Ok(BytePos(7)));
        assert_eq!(b.byte_to_line_col(BytePos(7)), LineCol::new(1, 0));
        // 'r' is char 2 on line 1 → byte 10 (w=1, ö=2 from byte 7).
        assert_eq!(b.byte_to_line_col(BytePos(10)), LineCol::new(1, 2));
    }

    #[test]
    fn line_col_to_byte_clamps_and_bounds() {
        let b = TextBuffer::from_text("héllo\nwörld");
        // Column past the line end clamps to the content end (char 5 → byte 6).
        assert_eq!(b.line_col_to_byte(LineCol::new(0, 99)), Ok(BytePos(6)));
        // A line past the end is out of bounds.
        assert_eq!(
            b.line_col_to_byte(LineCol::new(9, 0)),
            Err(TextError::OutOfBounds)
        );
        // A byte past the end clamps to the buffer end.
        assert_eq!(b.byte_to_line_col(BytePos(999)), LineCol::new(1, 5));
    }

    #[test]
    fn line_and_range_accessors() {
        let b = TextBuffer::from_text("héllo\nwörld");
        assert_eq!(b.line(0).as_deref(), Some("héllo"));
        assert_eq!(b.line(1).as_deref(), Some("wörld"));
        assert_eq!(b.line(2), None);
        // Content ranges exclude the trailing newline.
        assert_eq!(
            b.line_to_byte_range(0),
            Some(Span {
                start: BytePos(0),
                end: BytePos(6),
            })
        );
        assert_eq!(
            b.line_to_byte_range(1),
            Some(Span {
                start: BytePos(7),
                end: BytePos(13),
            })
        );
        assert_eq!(b.line_to_byte_range(2), None);
    }

    #[test]
    fn trailing_newline_yields_empty_last_line() {
        // A trailing "\n" produces an empty final line (rope line semantics).
        let b = TextBuffer::from_text("a\nb\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(2).as_deref(), Some(""));
        assert_eq!(b.line(3), None);
    }
}
