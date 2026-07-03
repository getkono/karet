//! `karet-text` — a headless text-editing model for the karet toolkit.
//!
//! A rope-backed [`TextBuffer`] with editing history plus a cursor/selection model
//! (the [`cursor`] module), usable by any editor backend (TUI or otherwise)
//! without pulling in rendering dependencies. It is the one place that converts
//! between byte offsets and line/column positions, since that requires the rope —
//! including the UTF-16 conversions LSP needs at its edge.
//!
//! The mutation surface ([`apply`](TextBuffer::apply) / [`undo`](TextBuffer::undo)
//! / [`redo`](TextBuffer::redo)) records an in-memory edit [`history`] and reports
//! each applied edit back as an [`AppliedEdit`] (with tree-sitter-shaped points) so
//! a parse host can reparse incrementally. Loading ([`load`](TextBuffer::load)) is
//! strict UTF-8 with line-ending/BOM detection; saving ([`save`](TextBuffer::save))
//! is atomic and round-trips the detected encoding.

use std::io::Read;

use karet_core::BytePos;
use karet_core::LineCol;
use karet_core::Span;

mod apply;
mod history;
mod load;
mod save;

pub use apply::Applied;
pub use apply::AppliedEdit;
pub use history::EditCause;
pub use history::EditContext;
use history::History;
pub use load::Encoding;
pub use load::Eol;
pub use load::LoadError;
pub use save::SavedState;

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
    /// A change's edits overlap; an atomic batch must be non-overlapping.
    #[error("overlapping edits in a single change")]
    OverlappingEdits,
}

/// A headless, editable text buffer backed by a rope.
///
/// Tracks a monotonically increasing edit `version` (used to validate
/// [`Change`](karet_core::Change)s under optimistic concurrency), an in-memory
/// undo/redo [`History`], and the detected line-ending / encoding so a save
/// round-trips the on-disk form. The dirty state is derived from the history
/// relative to the last save point, so undoing back to a saved state clears it.
#[derive(Clone, Default)]
pub struct TextBuffer {
    rope: ropey::Rope,
    version: u64,
    history: History,
    eol: Eol,
    encoding: Encoding,
    mixed_eol: bool,
    saved_state: Option<SavedState>,
}

impl TextBuffer {
    /// Create an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a buffer from in-memory text (assumed already LF-normalized UTF-8).
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: ropey::Rope::from_str(text),
            ..Self::default()
        }
    }

    /// Create a buffer by reading all of `reader` as UTF-8.
    ///
    /// This is the low-level reader path (no line-ending/BOM detection); prefer
    /// [`TextBuffer::load`] / [`TextBuffer::from_bytes`] for files on disk.
    ///
    /// # Errors
    /// Returns [`TextError::Io`] if reading fails.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, TextError> {
        let rope = ropey::Rope::from_reader(reader).map_err(|e| TextError::Io(e.to_string()))?;
        Ok(Self {
            rope,
            ..Self::default()
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

    /// The current edit version (incremented on every applied change, undo or redo).
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Whether the buffer has unsaved changes relative to the last save point.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// The detected line ending used when saving.
    #[must_use]
    pub fn eol(&self) -> Eol {
        self.eol
    }

    /// Override the line ending used when saving (an undoable user choice is the
    /// caller's concern; this only sets the serialization target).
    pub fn set_eol(&mut self, eol: Eol) {
        self.eol = eol;
    }

    /// The detected encoding (plain UTF-8 vs UTF-8 with a BOM).
    #[must_use]
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Whether the file mixed `\n` and `\r\n` on load (normalized to LF in memory).
    #[must_use]
    pub fn has_mixed_eol(&self) -> bool {
        self.mixed_eol
    }

    /// The fingerprint of the last on-disk state (from load or save), used by a
    /// file-watcher to distinguish the editor's own writes from external edits.
    #[must_use]
    pub fn saved_state(&self) -> Option<&SavedState> {
        self.saved_state.as_ref()
    }

    /// The full text as an owned `String` (LF-normalized; allocates the whole
    /// buffer — prefer line/slice accessors or [`rope`](Self::rope) on hot paths).
    #[must_use]
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Borrow the underlying rope (read-only) for chunk-wise consumers such as an
    /// incremental parse host.
    #[must_use]
    pub fn rope(&self) -> &ropey::Rope {
        &self.rope
    }

    /// The buffer bytes starting at `byte`, as one contiguous rope chunk, or an
    /// empty slice at/after the end.
    ///
    /// This is the reader an incremental parser is fed with: call it repeatedly
    /// with advancing offsets to stream the whole buffer without ever allocating it
    /// as a single `String`.
    #[must_use]
    pub fn byte_chunk(&self, byte: usize) -> &[u8] {
        if byte >= self.rope.len_bytes() {
            return &[];
        }
        let (chunk, chunk_byte_start, _, _) = self.rope.chunk_at_byte(byte);
        &chunk.as_bytes()[byte - chunk_byte_start..]
    }

    /// A cheap, render-only clone: shares the rope (O(1) structural sharing) but
    /// carries no edit history. The result is read-only — its
    /// [`is_dirty`](Self::is_dirty) is meaningless; read dirtiness from the source.
    #[must_use]
    pub fn content_snapshot(&self) -> Self {
        Self {
            rope: self.rope.clone(),
            version: self.version,
            history: History::default(),
            eol: self.eol,
            encoding: self.encoding,
            mixed_eol: self.mixed_eol,
            saved_state: self.saved_state.clone(),
        }
    }

    /// Discard all undo/redo history and reset the save point to "clean".
    ///
    /// The session calls this after replacing the buffer's content with a fresh
    /// on-disk read (an accepted external reload): the recorded inverse edits no
    /// longer match the new content, so they must be dropped.
    pub fn reset_history(&mut self) {
        self.history = History::default();
    }

    /// Replace this buffer's content (and line-ending/encoding/on-disk fingerprint)
    /// with `content` from a fresh on-disk read, **bumping the version** and
    /// **dropping history**.
    ///
    /// Used for an external reload: the version stays monotonic (so an optimistic
    /// client's version tracking still lines up), but the recorded inverse edits are
    /// discarded because they no longer match the new content.
    pub fn adopt_content(&mut self, content: Self) {
        self.rope = content.rope;
        self.eol = content.eol;
        self.encoding = content.encoding;
        self.mixed_eol = content.mixed_eol;
        self.saved_state = content.saved_state;
        self.version += 1;
        self.history = History::default();
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

    /// Convert an internal [`LineCol`] (UTF-32 column) to its UTF-16 column, for
    /// the LSP edge. The line is unchanged; the returned value is the column in
    /// UTF-16 code units, clamped to the line's content length.
    #[must_use]
    pub fn line_col_to_utf16(&self, pos: LineCol) -> u32 {
        let line_idx = pos.line as usize;
        if line_idx >= self.rope.len_lines() {
            return 0;
        }
        let line = self.rope.line(line_idx);
        let max = line_content_chars(line);
        let col = (pos.col as usize).min(max);
        let mut units = 0u32;
        for c in line.chars().take(col) {
            units += c.len_utf16() as u32;
        }
        units
    }

    /// Convert a UTF-16 `(line, column)` (as LSP speaks) to an internal [`LineCol`]
    /// with a UTF-32 (`char`) column. A column landing inside a surrogate pair is
    /// rounded down to the start of that `char`.
    #[must_use]
    pub fn utf16_to_line_col(&self, line: u32, utf16_col: u32) -> LineCol {
        let line_idx = line as usize;
        if line_idx >= self.rope.len_lines() {
            return LineCol::new(line, 0);
        }
        let lslice = self.rope.line(line_idx);
        let mut units = 0u32;
        let mut col = 0u32;
        for c in lslice.chars() {
            let width = c.len_utf16() as u32;
            // Stop before any char we cannot fully include, so an offset landing
            // inside a surrogate pair rounds down to that char's start.
            if units + width > utf16_col {
                break;
            }
            units += width;
            col += 1;
        }
        LineCol::new(line, col)
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

    /// The tree-sitter `(row, column-in-bytes)` point for an absolute byte offset.
    ///
    /// Tree-sitter columns are byte offsets from the line start — **not** the
    /// `char` columns of [`LineCol`] — so this is the conversion the parse edit
    /// path must use.
    pub(crate) fn byte_to_point(&self, byte: usize) -> (usize, usize) {
        let b = byte.min(self.rope.len_bytes());
        let row = self.rope.byte_to_line(b);
        let line_start = self.rope.line_to_byte(row);
        (row, b - line_start)
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

    #[test]
    fn utf16_conversions_handle_astral() {
        // "a😀b": 'a' (1 utf16), '😀' (2 utf16 — a surrogate pair), 'b' (1 utf16).
        let b = TextBuffer::from_text("a😀b");
        assert_eq!(b.line_col_to_utf16(LineCol::new(0, 0)), 0);
        assert_eq!(b.line_col_to_utf16(LineCol::new(0, 1)), 1); // before emoji
        assert_eq!(b.line_col_to_utf16(LineCol::new(0, 2)), 3); // after emoji (1 + 2)
        assert_eq!(b.line_col_to_utf16(LineCol::new(0, 3)), 4); // after 'b'
        // Round-trip: utf16 col 3 → char col 2.
        assert_eq!(b.utf16_to_line_col(0, 3), LineCol::new(0, 2));
        // A column landing mid-surrogate (1) rounds down to the emoji start.
        assert_eq!(b.utf16_to_line_col(0, 2), LineCol::new(0, 1));
    }

    #[test]
    fn adopt_content_bumps_version_and_clears_dirty() {
        let mut b = TextBuffer::from_text("old"); // version 0
        let fresh = TextBuffer::from_bytes(b"new\n").unwrap_or_default();
        b.adopt_content(fresh);
        assert_eq!(b.version(), 1, "reload keeps the version monotonic");
        assert_eq!(b.line(0).as_deref(), Some("new"));
        assert!(!b.is_dirty(), "a reloaded buffer matches disk");
        assert!(b.undo().is_none(), "reload drops undo history");
    }

    #[test]
    fn byte_to_point_uses_byte_columns() {
        // 'é' is two bytes, so the byte column after it is 2 even though it is one char.
        let b = TextBuffer::from_text("é=1\nx");
        assert_eq!(b.byte_to_point(0), (0, 0));
        assert_eq!(b.byte_to_point(2), (0, 2)); // after 'é' — byte column 2, char column 1
        assert_eq!(b.byte_to_point(5), (1, 0)); // start of line 1 ("é=1\n" = 5 bytes)
    }
}
