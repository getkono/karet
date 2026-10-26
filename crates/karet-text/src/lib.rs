//! `karet-text` — a headless text-editing model for the karet toolkit.
//!
//! A rope-backed [`TextBuffer`] with editing history, usable by any editor backend
//! (TUI or otherwise) without pulling in rendering dependencies. Cursors and
//! selections are not modelled here; they live in `karet_core::CursorState`.
//! It is the one place that converts between byte offsets and line/column
//! positions, since that requires the rope — including the UTF-16 conversions
//! LSP needs at its edge.
//!
//! The mutation surface ([`apply`](TextBuffer::apply) / [`undo`](TextBuffer::undo)
//! / [`redo`](TextBuffer::redo)) records an in-memory edit history and reports
//! each applied edit back as an [`AppliedEdit`] (with tree-sitter-shaped points) so
//! a parse host can reparse incrementally. Loading ([`load`](TextBuffer::load)) is
//! strict UTF-8 with line-ending/BOM detection; saving ([`save`](TextBuffer::save))
//! round-trips the detected encoding and is atomic, falling back to a direct
//! overwrite only where the temp-file rename fails.

use std::io::Read;

use karet_core::BytePos;
use karet_core::LineCol;
use karet_core::Span;

mod apply;
mod history;
mod lines;
mod load;
mod read;
mod save;

pub use apply::Applied;
pub use history::EditContext;
use history::History;
pub use history::HistorySnapshot;
pub use karet_core::AppliedEdit;
pub use karet_core::EditCause;
pub use lines::lines;
pub use load::Encoding;
pub use load::Eol;
pub use load::LoadError;
pub use read::TextSnapshot;
pub use save::SavedState;
pub use save::conflicts_with_disk;

/// The non-cryptographic fingerprint used for [`SavedState::hash`]: hashing a file's
/// bytes with this yields the same value karet records on save/load, so a caller can
/// detect whether a file changed under it (e.g. backup/swap conflict detection).
#[must_use]
pub fn content_fingerprint(bytes: &[u8]) -> u64 {
    save::hash_bytes(bytes)
}

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
    /// The file changed on disk since it was last read or saved; the save was
    /// refused rather than silently overwriting someone else's change.
    #[error("file changed on disk since it was last read")]
    Conflict,
    /// A [`HistorySnapshot`] was restored onto text other than the text it was
    /// taken from.
    #[error("undo history belongs to different content")]
    HistoryMismatch,
}

/// A headless, editable text buffer backed by a rope.
///
/// Tracks a monotonically increasing edit `version` (used to validate
/// [`Change`](karet_core::Change)s under optimistic concurrency), an in-memory
/// undo/redo history, and the detected line-ending / encoding so a save
/// round-trips the on-disk form. The dirty state is derived from the normalized
/// text content relative to the last loaded/saved content, so undo and equivalent
/// manual edits both clear it.
#[derive(Clone)]
pub struct TextBuffer {
    rope: ropey::Rope,
    version: u64,
    history: History,
    saved_text_hash: u64,
    eol: Eol,
    encoding: Encoding,
    mixed_eol: bool,
    saved_state: Option<SavedState>,
}

impl Default for TextBuffer {
    fn default() -> Self {
        let rope = ropey::Rope::new();
        let saved_text_hash = text_hash(&rope);
        Self {
            rope,
            version: 0,
            history: History::default(),
            saved_text_hash,
            eol: Eol::default(),
            encoding: Encoding::default(),
            mixed_eol: false,
            saved_state: None,
        }
    }
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
        let rope = ropey::Rope::from_str(text);
        let saved_text_hash = text_hash(&rope);
        Self {
            rope,
            saved_text_hash,
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
        let saved_text_hash = text_hash(&rope);
        Ok(Self {
            rope,
            saved_text_hash,
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
    ///
    /// Compares a hash of the current text with the hash taken at the last save
    /// point, so editing back to the saved text reads as clean. The hash is
    /// recomputed over the whole rope on every call, so each call costs O(buffer
    /// length).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        text_hash(&self.rope) != self.saved_text_hash
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

    /// Override the UTF-8 serialization variant used when saving.
    pub fn set_encoding(&mut self, encoding: Encoding) {
        self.encoding = encoding;
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
    /// buffer — on hot paths prefer [`slice`](Self::slice), [`find`](Self::find),
    /// the line accessors, or a lazily materialized
    /// [`text_snapshot`](Self::text_snapshot)).
    #[must_use]
    pub fn text(&self) -> String {
        self.rope.to_string()
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
            saved_text_hash: self.saved_text_hash,
            eol: self.eol,
            encoding: self.encoding,
            mixed_eol: self.mixed_eol,
            saved_state: self.saved_state.clone(),
        }
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
        self.saved_text_hash = content.saved_text_hash;
        self.eol = content.eol;
        self.encoding = content.encoding;
        self.mixed_eol = content.mixed_eol;
        self.saved_state = content.saved_state;
        self.version += 1;
        self.history = self.history.cleared();
    }

    /// The most undo steps this buffer keeps, or `None` when history is
    /// unbounded (the default).
    #[must_use]
    pub fn undo_limit(&self) -> Option<usize> {
        self.history.limit()
    }

    /// Keep at most `limit` undo steps, forgetting the oldest once a new one
    /// would exceed it; `None` lifts the cap. Lowering the cap below what the
    /// history already holds forgets the excess at once. `Some(0)` keeps no
    /// undo at all.
    pub fn set_undo_limit(&mut self, limit: Option<usize>) {
        self.history.set_limit(limit);
    }

    /// How many undo steps are available (a coalesced run of typing is one).
    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.history.undo_len()
    }

    /// A serializable copy of the undo/redo history, tagged with a fingerprint
    /// of the current text so it can only be restored onto that text.
    #[must_use]
    pub fn history_snapshot(&self) -> HistorySnapshot {
        self.history.snapshot(text_hash(&self.rope))
    }

    /// Replace the undo/redo history with `snapshot`, which must have been taken
    /// from a buffer holding exactly this text. The restored head becomes the
    /// save point and this buffer's undo cap still applies. The version is
    /// bumped, so no later cursor report is attributed to a restored step.
    ///
    /// # Errors
    /// Returns [`TextError::HistoryMismatch`] (leaving history untouched) when the
    /// snapshot was taken from different text: its edits would not line up.
    pub fn restore_history(&mut self, snapshot: HistorySnapshot) -> Result<(), TextError> {
        if snapshot.content_hash() != text_hash(&self.rope) {
            return Err(TextError::HistoryMismatch);
        }
        self.history = self.history.restored(snapshot, self.version);
        self.version += 1;
        Ok(())
    }

    /// Convert an absolute byte offset to a line/column position.
    ///
    /// The column is counted in Unicode scalar values (`char`s), karet's canonical
    /// internal unit. An offset past the end of the buffer is clamped to the
    /// buffer end.
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

    /// The nearest valid position to `pos`: a line past the end becomes the
    /// last line, and the column is clamped to that line's content length
    /// (excluding the trailing line break), so the result always round-trips
    /// through [`line_col_to_byte`](Self::line_col_to_byte) unchanged.
    #[must_use]
    pub fn clamp(&self, pos: LineCol) -> LineCol {
        let last = self.rope.len_lines().saturating_sub(1);
        let line = (pos.line as usize).min(last);
        let content_chars = line_content_chars(self.rope.line(line));
        LineCol {
            line: line as u32,
            col: (pos.col as usize).min(content_chars) as u32,
        }
    }

    /// Convert a line/column position to an absolute byte offset.
    ///
    /// `col` is interpreted in `char`s and silently clamped to the line's
    /// content length (excluding the trailing line break); use
    /// [`clamp`](Self::clamp) to learn the position that was actually used.
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
    ///
    /// A column past the line's content is clamped to the content length, never
    /// counting the line's trailing line break, as the LSP specification asks and
    /// as [`line_col_to_utf16`](Self::line_col_to_utf16) does in the other
    /// direction. A `line` past the last line is returned unchanged with column 0,
    /// so that result still does not address a position inside the buffer.
    #[must_use]
    pub fn utf16_to_line_col(&self, line: u32, utf16_col: u32) -> LineCol {
        let line_idx = line as usize;
        if line_idx >= self.rope.len_lines() {
            return LineCol::new(line, 0);
        }
        let lslice = self.rope.line(line_idx);
        let content = lslice.slice(..line_content_chars(lslice));
        let mut units = 0u32;
        let mut col = 0u32;
        for c in content.chars() {
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
    pub(crate) fn byte_to_point(&self, byte: usize) -> karet_core::BytePoint {
        let b = byte.min(self.rope.len_bytes());
        let row = self.rope.byte_to_line(b);
        let line_start = self.rope.line_to_byte(row);
        karet_core::BytePoint::new(row, b - line_start)
    }
}

/// Fingerprint the editor's normalized text content, independent of history
/// position and independent of on-disk bytes for encoded formats such as CBOR.
fn text_hash(rope: &ropey::Rope) -> u64 {
    // Same fixed algorithm as the on-disk fingerprint: this value is compared
    // against one recorded by an earlier run, so it cannot move with the
    // toolchain. The length is folded in first so a chunk boundary cannot
    // collide with a different split of the same bytes.
    let hash = save::fnv1a_extend(save::FNV1A_OFFSET, &rope.len_bytes().to_le_bytes());
    rope.chunks().fold(hash, |hash, chunk| {
        save::fnv1a_extend(hash, chunk.as_bytes())
    })
}

/// The number of `char`s in `line`, excluding its trailing line break: any
/// break the rope splits on ([`lines::is_break`]), with `\r\n` as one break.
fn line_content_chars(line: ropey::RopeSlice<'_>) -> usize {
    let mut end = line.len_chars();
    if end > 0 && lines::is_break(line.char(end - 1)) {
        let last = line.char(end - 1);
        end -= 1;
        if last == '\n' && end > 0 && line.char(end - 1) == '\r' {
            end -= 1;
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_hash_is_fixed_by_its_definition() {
        // A restored history snapshot is matched against this value from an
        // earlier run: golden FNV-1a over the little-endian length, then the text.
        assert_eq!(
            text_hash(&ropey::Rope::from_str("hello")),
            0xff7a_61ff_1132_0f78
        );
    }

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
    fn clamp_pulls_a_position_onto_the_text() {
        let b = TextBuffer::from_text("héllo\r\nwö\n");
        // Inside the text: unchanged.
        assert_eq!(b.clamp(LineCol::new(1, 1)), LineCol::new(1, 1));
        // Past a CRLF line's content: its end, before the `\r`.
        assert_eq!(b.clamp(LineCol::new(0, 40)), LineCol::new(0, 5));
        // Past the last line: the empty line after the final newline.
        assert_eq!(b.clamp(LineCol::new(9, 3)), LineCol::new(2, 0));
        // The empty buffer has exactly one position.
        let empty = TextBuffer::from_text("");
        assert_eq!(empty.clamp(LineCol::new(4, 4)), LineCol::new(0, 0));
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

    /// `text_hash` is persisted with saved undo history and compared on a later
    /// run, so it is fixed by its definition: FNV-1a over the length (as
    /// little-endian `usize` bytes) and then the text. These are golden values;
    /// if they change, every persisted history is refused as belonging to
    /// different content.
    #[test]
    fn content_hash_is_fixed_by_its_definition() {
        assert_eq!(text_hash(&ropey::Rope::new()), 0xa8c7_f832_281a_39c5);
        assert_eq!(
            text_hash(&ropey::Rope::from_str("hello\n")),
            0xe148_30a9_01eb_5991
        );

        // Large enough to span several rope chunks, built two ways so the
        // chunk boundaries differ: the hash must not depend on them.
        let text = "0123456789abcdef".repeat(256);
        let whole = ropey::Rope::from_str(&text);
        let mut grown = ropey::Rope::new();
        for piece in text
            .as_bytes()
            .chunks(16)
            .filter_map(|c| std::str::from_utf8(c).ok())
        {
            grown.insert(grown.len_chars(), piece);
        }
        assert!(whole.chunks().count() > 1, "the text spans several chunks");
        assert_eq!(grown.to_string(), text);
        assert_eq!(text_hash(&whole), 0xddb4_2317_550a_c415);
        assert_eq!(text_hash(&grown), 0xddb4_2317_550a_c415);

        // It is the value a persisted history snapshot carries.
        let b = TextBuffer::from_text("hello\n");
        assert_eq!(b.history_snapshot().content_hash(), 0xe148_30a9_01eb_5991);
    }

    #[test]
    fn utf16_column_past_the_end_of_an_lf_line() {
        let b = TextBuffer::from_text("ab\ncd");
        // The column stops at the content end (2), never counting the `\n`, so
        // the result already addresses a position inside the buffer (#634).
        assert_eq!(b.utf16_to_line_col(0, 99), LineCol::new(0, 2));
        assert_eq!(b.utf16_to_line_col(0, 3), LineCol::new(0, 2));
        assert_eq!(b.clamp(b.utf16_to_line_col(0, 99)), LineCol::new(0, 2));
        assert_eq!(
            b.line_col_to_byte(b.utf16_to_line_col(0, 99)),
            Ok(BytePos(2))
        );
        // The last line has no break, so it stops at its content end.
        assert_eq!(b.utf16_to_line_col(1, 99), LineCol::new(1, 2));
        // A line past the end keeps its line number and gets column 0.
        assert_eq!(b.utf16_to_line_col(7, 3), LineCol::new(7, 0));
        // The opposite direction clamps to the content end.
        assert_eq!(b.line_col_to_utf16(LineCol::new(0, 99)), 2);
        assert_eq!(b.line_col_to_utf16(LineCol::new(7, 1)), 0);
    }

    #[test]
    fn utf16_column_past_the_end_excludes_every_break_type() {
        // `\r\n` is two chars of break; a lone `\r` and U+2028 are one each.
        let b = TextBuffer::from_text("ab\r\ncd\rx\u{2028}y");
        assert_eq!(b.utf16_to_line_col(0, 99), LineCol::new(0, 2));
        assert_eq!(b.utf16_to_line_col(0, 3), LineCol::new(0, 2));
        assert_eq!(b.utf16_to_line_col(1, 99), LineCol::new(1, 2));
        assert_eq!(b.utf16_to_line_col(2, 99), LineCol::new(2, 1));
        assert_eq!(b.utf16_to_line_col(3, 99), LineCol::new(3, 1));
        // Round-trips with the opposite direction at every line end.
        for line in 0..4 {
            let end = b.utf16_to_line_col(line, 99);
            assert_eq!(b.utf16_to_line_col(line, b.line_col_to_utf16(end)), end);
        }
        // An astral char before the break still counts as one column.
        let astral = TextBuffer::from_text("a\u{1F600}\nz");
        assert_eq!(astral.utf16_to_line_col(0, 99), LineCol::new(0, 2));
    }

    #[test]
    fn line_accessors_on_each_break_type() {
        // Every break the rope splits on is stripped from a line's content,
        // `\r\n` as one break, so a clamped end-of-line position sits before
        // the break rather than at the next line's start.
        for brk in [
            "\n", "\r\n", "\r", "\u{0B}", "\u{0C}", "\u{85}", "\u{2028}", "\u{2029}",
        ] {
            let text = format!("a{brk}b");
            let text = text.as_str();
            let break_len = brk.len();
            let b = TextBuffer::from_text(text);
            assert_eq!(b.line_count(), 2, "for {text:?}");
            assert_eq!(b.line(0).as_deref(), Some("a"), "for {text:?}");
            assert_eq!(
                b.line_to_byte_range(0),
                Some(Span {
                    start: BytePos(0),
                    end: BytePos(1),
                }),
                "for {text:?}"
            );
            assert_eq!(b.clamp(LineCol::new(0, 9)), LineCol::new(0, 1));
            assert_eq!(b.line_col_to_byte(LineCol::new(0, 9)), Ok(BytePos(1)));
            assert_eq!(
                b.line_col_to_byte(LineCol::new(1, 0)),
                Ok(BytePos(1 + break_len))
            );
            assert_eq!(b.line(1).as_deref(), Some("b"), "for {text:?}");
            assert_eq!(b.line_col_to_utf16(LineCol::new(0, 9)), 1, "for {text:?}");
            // `lines` splits on the same break and agrees with `line`.
            assert_eq!(lines(text).collect::<Vec<_>>(), vec!["a", "b"]);
        }
        // A `\r` not followed by `\n` is its own break: `"a\r\rb"` is three
        // lines, the middle one empty, and neither `\r` is content.
        let b = TextBuffer::from_text("a\r\rb");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(0).as_deref(), Some("a"));
        assert_eq!(b.line(1).as_deref(), Some(""));
        assert_eq!(b.clamp(LineCol::new(1, 9)), LineCol::new(1, 0));
        assert_eq!(b.line_col_to_byte(LineCol::new(1, 9)), Ok(BytePos(2)));
        // `"\n\r"` is two breaks, not a reversed `\r\n`.
        let b = TextBuffer::from_text("a\n\rb");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1).as_deref(), Some(""));
    }

    #[test]
    fn content_snapshot_shares_content_but_not_history() {
        let mut b = TextBuffer::from_bytes(b"\xEF\xBB\xBFa\r\nb\nc\r\n").unwrap_or_default();
        let at_end = LineCol::new(3, 0);
        let change = karet_core::Change::new(
            0,
            vec![karet_core::TextEdit {
                range: karet_core::Range {
                    start: at_end,
                    end: at_end,
                },
                new_text: "d".to_string(),
            }],
        );
        assert!(b.apply_simple(&change).is_ok());

        let mut snap = b.content_snapshot();
        assert_eq!(snap.text(), "a\nb\nc\nd");
        assert_eq!(snap.version(), b.version());
        assert_eq!(snap.eol(), Eol::Crlf);
        assert_eq!(snap.encoding(), Encoding::Utf8Bom);
        assert!(snap.has_mixed_eol());
        assert_eq!(snap.saved_state(), b.saved_state());
        assert_eq!(snap.undo_len(), 0, "a snapshot carries no history");
        assert!(snap.undo().is_none());

        // Editing the source afterwards leaves the snapshot as it was.
        assert!(b.undo().is_some());
        assert_eq!(b.text(), "a\nb\nc\n");
        assert_eq!(snap.text(), "a\nb\nc\nd");
    }

    #[test]
    fn byte_to_point_uses_byte_columns() {
        // 'é' is two bytes, so the byte column after it is 2 even though it is one char.
        let b = TextBuffer::from_text("é=1\nx");
        use karet_core::BytePoint;
        assert_eq!(b.byte_to_point(0), BytePoint::new(0, 0));
        // After 'é' — byte column 2, char column 1.
        assert_eq!(b.byte_to_point(2), BytePoint::new(0, 2));
        // Start of line 1 ("é=1\n" = 5 bytes).
        assert_eq!(b.byte_to_point(5), BytePoint::new(1, 0));
    }
}
