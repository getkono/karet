//! Atomic, crash-safe saving with line-ending and BOM round-trip.
//!
//! The in-memory rope is LF-normalized; on save it is re-serialized to the
//! detected [`Eol`](crate::Eol) (re-adding the BOM if the file had one) and written
//! crash-safely: to a temp file in the same directory, fsynced, then atomically
//! renamed over the target. A fingerprint of the bytes written is returned so a
//! file-watcher can recognize the editor's own write.

use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use crate::TextBuffer;
use crate::TextError;
use crate::load::Encoding;
use crate::load::Eol;
use crate::text_hash;

/// A fingerprint of a file's on-disk state at the moment karet last read or wrote
/// it. A file-watcher compares this against a fresh `stat` to tell the editor's own
/// write (matching `size`/`mtime`/`hash`) from an external modification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedState {
    /// Last-modified time as reported by the filesystem.
    pub mtime: SystemTime,
    /// File size in bytes.
    pub size: u64,
    /// Non-cryptographic hash of the exact bytes written.
    pub hash: u64,
}

impl TextBuffer {
    /// Save the buffer to `path` atomically, round-tripping the detected encoding
    /// and line ending, and clearing the dirty flag on success.
    ///
    /// Returns the on-disk [`SavedState`] fingerprint (also stored on the buffer).
    ///
    /// # Errors
    /// Returns [`TextError::Io`] if the file cannot be written.
    pub fn save(&mut self, path: &Path) -> Result<SavedState, TextError> {
        let bytes = self.serialize();
        self.save_bytes(path, &bytes)
    }

    /// Save a buffer whose destination did not exist when editing began.
    ///
    /// The destination is created atomically and is never overwritten. This is the
    /// safe first-save path for a new document opened by name.
    ///
    /// # Errors
    /// Returns [`TextError::Conflict`] if `path` already exists, or [`TextError::Io`]
    /// if the file cannot be written.
    pub fn save_new(&mut self, path: &Path) -> Result<SavedState, TextError> {
        let bytes = self.serialize();
        self.save_new_bytes(path, &bytes)
    }

    /// Save already-serialized `bytes` to `path` atomically, recording the on-disk
    /// fingerprint and clearing the dirty flag on success.
    ///
    /// Unlike [`save`](Self::save), this does **not** serialize the rope — the
    /// caller has produced the on-disk bytes itself. Use it for documents whose
    /// on-disk form differs from the edit buffer (e.g. a binary format decoded to
    /// editable text and re-encoded on save).
    ///
    /// # Errors
    /// Returns [`TextError::Io`] if the file cannot be written, or
    /// [`TextError::Conflict`] if the file changed on disk since it was last read
    /// or saved (nothing is written in that case).
    pub fn save_bytes(&mut self, path: &Path, bytes: &[u8]) -> Result<SavedState, TextError> {
        if let Some(saved) = &self.saved_state
            && conflicts_with_disk(path, saved)?
        {
            return Err(TextError::Conflict);
        }
        // Resolve symlinks so we replace the real file's directory entry, not the
        // link. `canonicalize` fails for a not-yet-existing file; then use the path.
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let dir = save_parent(&target)?;
        write_atomic(dir, &target, bytes)?;
        self.record_saved_bytes(&target, bytes)
    }

    /// Save already-serialized bytes to a destination that must not exist.
    ///
    /// This is the no-clobber counterpart to [`save_bytes`](Self::save_bytes) for
    /// decoded document formats whose edit text differs from their disk bytes.
    ///
    /// # Errors
    /// Returns [`TextError::Conflict`] if `path` already exists, or [`TextError::Io`]
    /// if the file cannot be written.
    pub fn save_new_bytes(&mut self, path: &Path, bytes: &[u8]) -> Result<SavedState, TextError> {
        let dir = save_parent(path)?;
        write_atomic_noclobber(dir, path, bytes)?;
        self.record_saved_bytes(path, bytes)
    }

    fn record_saved_bytes(&mut self, target: &Path, bytes: &[u8]) -> Result<SavedState, TextError> {
        let meta = std::fs::metadata(target).map_err(|e| TextError::Io(e.to_string()))?;
        let state = SavedState {
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: meta.len(),
            hash: hash_bytes(bytes),
        };
        self.saved_state = Some(state.clone());
        self.saved_text_hash = text_hash(&self.rope);
        self.history.mark_saved();
        Ok(state)
    }

    /// Serialize the rope to bytes: optional BOM, then the content with `\n`
    /// converted to the target line ending. Streams chunk-wise (no whole-file
    /// `String`).
    fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rope.len_bytes() + 3);
        if self.encoding == Encoding::Utf8Bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        match self.eol {
            Eol::Lf => {
                for chunk in self.rope.chunks() {
                    out.extend_from_slice(chunk.as_bytes());
                }
            },
            Eol::Crlf => {
                for chunk in self.rope.chunks() {
                    let b = chunk.as_bytes();
                    let mut last = 0;
                    for i in memchr::memchr_iter(b'\n', b) {
                        out.extend_from_slice(&b[last..i]);
                        out.extend_from_slice(b"\r\n");
                        last = i + 1;
                    }
                    out.extend_from_slice(&b[last..]);
                }
            },
        }
        out
    }
}

/// Whether the file at `path` has changed on disk since `saved` was recorded
/// (i.e. since it was last loaded or saved). A free function over `&Path`/
/// `&SavedState` rather than a `&TextBuffer` method, so a future non-text
/// (e.g. binary/hex) buffer type can reuse the same disk-fingerprinting
/// primitive without depending on `TextBuffer`.
///
/// Checks a cheap `stat` first (size + mtime, no read); only when those don't
/// match does it read and hash the file to rule out a false positive from a
/// bare `touch` (same content, different mtime). A missing file never
/// conflicts — there is nothing on disk to have diverged from.
///
/// # Errors
/// Returns [`TextError::Io`] if the file exists but can't be read.
pub fn conflicts_with_disk(path: &Path, saved: &SavedState) -> Result<bool, TextError> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(TextError::Io(e.to_string())),
    };
    if meta.len() == saved.size && meta.modified().is_ok_and(|m| m == saved.mtime) {
        return Ok(false);
    }
    let bytes = std::fs::read(path).map_err(|e| TextError::Io(e.to_string()))?;
    Ok(hash_bytes(&bytes) != saved.hash)
}

/// Write `bytes` to `target` crash-safely via a same-directory temp file + atomic
/// rename, falling back to a direct write if the rename crosses a filesystem.
fn write_atomic(dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), TextError> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| TextError::Io(e.to_string()))?;
    tmp.write_all(bytes)
        .map_err(|e| TextError::Io(e.to_string()))?;
    tmp.flush().map_err(|e| TextError::Io(e.to_string()))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| TextError::Io(e.to_string()))?;
    if let Ok(meta) = std::fs::metadata(target) {
        // Best-effort permission preservation (ignored on platforms without it).
        let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
    }
    match tmp.persist(target) {
        Ok(_) => Ok(()),
        // EXDEV (cross-filesystem rename) or similar: direct write fallback. The
        // dropped `PersistError` cleans up the temp file.
        Err(_) => std::fs::write(target, bytes).map_err(|e| TextError::Io(e.to_string())),
    }
}

fn save_parent(path: &Path) -> Result<&Path, TextError> {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Ok(Path::new(".")),
        Some(parent) => Ok(parent),
        None => Err(TextError::Io(
            "save path has no parent directory".to_string(),
        )),
    }
}

/// Write to an absent target without a check-then-rename race. `persist_noclobber`
/// uses the platform's atomic no-replace primitive, so another creator always wins
/// cleanly and its bytes remain untouched.
fn write_atomic_noclobber(dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), TextError> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| TextError::Io(e.to_string()))?;
    tmp.write_all(bytes)
        .and_then(|()| tmp.flush())
        .and_then(|()| tmp.as_file().sync_all())
        .map_err(|e| TextError::Io(e.to_string()))?;
    tmp.persist_noclobber(target).map(|_| ()).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            TextError::Conflict
        } else {
            TextError::Io(error.error.to_string())
        }
    })
}

/// A fast, non-cryptographic fingerprint of `bytes` (no extra dependency). This is
/// the exact function backing [`SavedState::hash`], exposed as
/// [`crate::content_fingerprint`] so callers can recompute a file's fingerprint and
/// compare it against a recorded one (e.g. backup/swap conflict detection).
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    fnv1a(bytes)
}

/// FNV-1a over `bytes` — a fixed, specified algorithm.
///
/// Deliberately not `DefaultHasher`: these fingerprints are written to disk in
/// crash-recovery swap files and recompared on a later run, and the standard
/// library does not promise its hasher's output stays the same across
/// releases. A toolchain upgrade would therefore make an untouched file look
/// modified, warning the user that recovering their swap discards newer
/// content — exactly when they can least judge it.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;

    /// The fingerprint is written into crash-recovery swap files and compared
    /// against a value recomputed on a later run, possibly by a binary built
    /// with a different toolchain. These are golden FNV-1a values: if they
    /// ever change, every saved swap starts reporting a false conflict.
    #[test]
    fn the_fingerprint_is_fixed_by_its_definition() {
        assert_eq!(hash_bytes(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(hash_bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash_bytes(b"foobar"), 0x85944171_f73967e8);
    }

    #[test]
    fn different_content_fingerprints_differently() {
        assert_ne!(hash_bytes(b"on disk\n"), hash_bytes(b"on disk!\n"));
        assert_ne!(hash_bytes(b"ab"), hash_bytes(b"ba"));
        assert_eq!(hash_bytes(b"same"), hash_bytes(b"same"));
    }
}

#[cfg(test)]
mod tests {
    use karet_core::Change;
    use karet_core::LineCol;
    use karet_core::Range;
    use karet_core::TextEdit;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Save `input` (parsed via `from_bytes`) to a fresh temp file and return the
    /// raw bytes read back and whether the buffer is still dirty. Every failure
    /// is an error, so a caller using `?` cannot pass without asserting.
    fn round_trip(input: &[u8]) -> TestResult<(Vec<u8>, bool)> {
        let mut buf = TextBuffer::from_bytes(input)?;
        save_and_read(&mut buf)
    }

    /// Save `buf` to a fresh temp file and return the raw bytes read back and
    /// whether the buffer is still dirty.
    fn save_and_read(buf: &mut TextBuffer) -> TestResult<(Vec<u8>, bool)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.txt");
        buf.save(&path)?;
        Ok((std::fs::read(&path)?, buf.is_dirty()))
    }

    /// Move `path`'s mtime to one second after `saved.mtime`, so the stat fast
    /// path in [`conflicts_with_disk`] cannot match whatever the filesystem's
    /// timestamp granularity is.
    fn force_newer_mtime(path: &Path, saved: &SavedState) -> TestResult {
        let later = saved.mtime + std::time::Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(path)?
            .set_modified(later)?;
        let now = std::fs::metadata(path)?.modified()?;
        assert_ne!(now, saved.mtime, "the mtime must have moved");
        Ok(())
    }

    #[test]
    fn lf_round_trips_and_clears_dirty() -> TestResult {
        let (raw, dirty) = round_trip(b"a\nb\n")?;
        assert_eq!(raw, b"a\nb\n");
        assert!(!dirty, "save clears dirty");
        Ok(())
    }

    #[test]
    fn crlf_round_trips() -> TestResult {
        // Loaded as CRLF, normalized to LF in memory, must save back as CRLF.
        let (raw, _) = round_trip(b"a\r\nb\r\n")?;
        assert_eq!(raw, b"a\r\nb\r\n");
        Ok(())
    }

    #[test]
    fn bom_round_trips() -> TestResult {
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"hi\n");
        let (raw, _) = round_trip(&input)?;
        assert_eq!(raw, input.as_slice());
        Ok(())
    }

    #[test]
    fn bom_and_crlf_round_trip_together() -> TestResult {
        let input = b"\xEF\xBB\xBFa\r\nb\r\n";
        let (raw, _) = round_trip(input)?;
        assert_eq!(raw, input);
        Ok(())
    }

    #[test]
    fn no_trailing_newline_preserved() -> TestResult {
        let (raw, _) = round_trip(b"abc")?;
        assert_eq!(raw, b"abc");
        Ok(())
    }

    #[test]
    fn lone_cr_survives_a_crlf_round_trip() -> TestResult {
        // An old-Mac `\r` is content, not a line ending to convert: it is kept
        // as-is next to the CRLF the file is saved with.
        let (raw, _) = round_trip(b"a\rb\r\nc")?;
        assert_eq!(raw, b"a\rb\r\nc");
        let (raw, _) = round_trip(b"a\rb\nc")?;
        assert_eq!(raw, b"a\rb\nc");
        Ok(())
    }

    #[test]
    fn mixed_endings_save_as_the_majority() -> TestResult {
        // Two CRLF against one LF: every line is saved CRLF.
        let (raw, _) = round_trip(b"a\r\nb\nc\r\n")?;
        assert_eq!(raw, b"a\r\nb\r\nc\r\n");
        // Two LF against one CRLF: every line is saved LF.
        let (raw, _) = round_trip(b"a\nb\r\nc\n")?;
        assert_eq!(raw, b"a\nb\nc\n");
        // A tie goes to LF.
        let (raw, _) = round_trip(b"a\r\nb\n")?;
        assert_eq!(raw, b"a\nb\n");
        Ok(())
    }

    #[test]
    fn set_eol_chooses_the_saved_line_ending() -> TestResult {
        let mut buf = TextBuffer::from_text("a\nb\n");
        buf.set_eol(Eol::Crlf);
        assert_eq!(buf.eol(), Eol::Crlf);
        assert!(!buf.is_dirty(), "the line ending is not content");
        assert_eq!(save_and_read(&mut buf)?.0, b"a\r\nb\r\n");

        let mut buf = TextBuffer::from_bytes(b"a\r\nb\r\n")?;
        buf.set_eol(Eol::Lf);
        assert_eq!(buf.eol(), Eol::Lf);
        assert_eq!(save_and_read(&mut buf)?.0, b"a\nb\n");
        Ok(())
    }

    #[test]
    fn save_bytes_writes_raw_and_records_state() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("blob.cbor");
        let mut buf = TextBuffer::from_text("null");
        // 0xf6 is CBOR `null`; the on-disk form differs from the edit text.
        let state = buf.save_bytes(&path, &[0xf6])?;
        assert_eq!(std::fs::read(&path)?, vec![0xf6]);
        assert_eq!(state.size, 1);
        assert_eq!(state.hash, hash_bytes(&[0xf6]));
        assert_eq!(buf.saved_state(), Some(&state));
        assert!(!buf.is_dirty());
        Ok(())
    }

    #[test]
    fn save_new_creates_once_without_overwriting() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("new.txt");
        let mut buffer = TextBuffer::from_text("mine\n");

        buffer.save_new(&path)?;
        assert_eq!(std::fs::read_to_string(&path)?, "mine\n");
        assert_eq!(buffer.save_new(&path), Err(TextError::Conflict));
        Ok(())
    }

    #[test]
    fn save_new_bytes_refuses_an_existing_destination() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("new.bin");
        std::fs::write(&path, b"external")?;
        let mut buffer = TextBuffer::from_text("mine");

        assert_eq!(
            buffer.save_new_bytes(&path, b"mine"),
            Err(TextError::Conflict)
        );
        assert_eq!(std::fs::read(&path)?, b"external");
        Ok(())
    }

    #[test]
    fn encoding_override_controls_the_saved_bom() -> TestResult {
        let mut buf = TextBuffer::from_text("hello\n");
        buf.set_encoding(Encoding::Utf8Bom);
        assert_eq!(save_and_read(&mut buf)?.0, b"\xEF\xBB\xBFhello\n");

        let mut buf = TextBuffer::from_bytes(b"\xEF\xBB\xBFhello\n")?;
        buf.set_encoding(Encoding::Utf8);
        assert_eq!(save_and_read(&mut buf)?.0, b"hello\n");
        Ok(())
    }

    #[test]
    fn save_is_refused_when_the_file_changed_on_disk_since_it_was_read() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("conflict.txt");
        std::fs::write(&path, "original\n")?;
        let mut buf = TextBuffer::load(&path)?;

        // Someone else changes the file after we read it but before we save.
        std::fs::write(&path, "someone else's edit\n")?;

        assert_eq!(buf.save(&path), Err(TextError::Conflict));
        assert_eq!(
            std::fs::read_to_string(&path)?,
            "someone else's edit\n",
            "a refused save must not touch the file"
        );
        Ok(())
    }

    #[test]
    fn same_size_edit_with_a_new_mtime_is_a_conflict() -> TestResult {
        // The stat fast path cannot decide this (the size matches), so the hash
        // fallback must see that the bytes differ.
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("same-size.txt");
        std::fs::write(&path, "aaaa\n")?;
        let mut buf = TextBuffer::load(&path)?;
        let saved = buf.saved_state().cloned().ok_or("load records a state")?;

        std::fs::write(&path, "bbbb\n")?;
        force_newer_mtime(&path, &saved)?;

        assert_eq!(conflicts_with_disk(&path, &saved), Ok(true));
        assert_eq!(buf.save(&path), Err(TextError::Conflict));
        assert_eq!(std::fs::read_to_string(&path)?, "bbbb\n");
        Ok(())
    }

    #[test]
    fn save_succeeds_when_disk_is_unchanged_even_after_a_bare_touch() -> TestResult {
        // A bare mtime bump (no content change) must not be treated as a conflict —
        // ruling that out is exactly what the read+hash fallback is for.
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("touched.txt");
        std::fs::write(&path, "same\n")?;
        let mut buf = TextBuffer::load(&path)?;
        let saved = buf.saved_state().cloned().ok_or("load records a state")?;

        // Move the mtime explicitly: rewriting the file within the filesystem's
        // timestamp granularity would leave it equal, and the fast path would
        // answer without ever reaching the hash fallback this test is about.
        force_newer_mtime(&path, &saved)?;

        assert_eq!(conflicts_with_disk(&path, &saved), Ok(false));
        assert!(
            buf.save(&path).is_ok(),
            "identical content is not a conflict"
        );
        Ok(())
    }

    #[test]
    fn a_missing_file_never_conflicts() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("gone.txt");
        std::fs::write(&path, "x\n")?;
        let mut buf = TextBuffer::load(&path)?;
        let saved = buf.saved_state().cloned().ok_or("load records a state")?;
        std::fs::remove_file(&path)?;

        assert_eq!(conflicts_with_disk(&path, &saved), Ok(false));
        buf.save(&path)?;
        assert_eq!(std::fs::read(&path)?, b"x\n");
        Ok(())
    }

    #[test]
    fn edit_then_save_is_clean_with_fingerprint() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("edit.txt");
        let mut buf = TextBuffer::from_bytes(b"hello\n")?;
        let change = Change::new(
            0,
            vec![TextEdit {
                range: Range {
                    start: LineCol::new(0, 5),
                    end: LineCol::new(0, 5),
                },
                new_text: "!".to_string(),
            }],
        );
        buf.apply_simple(&change)?;
        assert!(buf.is_dirty());
        let state = buf.save(&path)?;
        assert!(!buf.is_dirty());
        assert_eq!(state.hash, hash_bytes(b"hello!\n"));
        assert_eq!(state.size, 7);
        assert_eq!(std::fs::read(&path)?, b"hello!\n");
        Ok(())
    }
}
