//! Atomic, crash-safe saving with line-ending and BOM round-trip.
//!
//! The in-memory rope is LF-normalized; on save it is re-serialized to the
//! detected [`Eol`](crate::Eol) (re-adding the BOM if the file had one) and written
//! crash-safely: to a temp file in the same directory, fsynced, then atomically
//! renamed over the target. A fingerprint of the bytes written is returned so a
//! file-watcher can recognize the editor's own write.

use crate::load::{Encoding, Eol};
use crate::{TextBuffer, TextError};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

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
        // Resolve symlinks so we replace the real file's directory entry, not the
        // link. `canonicalize` fails for a not-yet-existing file; then use the path.
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let dir = target
            .parent()
            .ok_or_else(|| TextError::Io("save path has no parent directory".to_string()))?;
        write_atomic(dir, &target, &bytes)?;
        let meta = std::fs::metadata(&target).map_err(|e| TextError::Io(e.to_string()))?;
        let state = SavedState {
            mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: meta.len(),
            hash: hash_bytes(&bytes),
        };
        self.saved_state = Some(state.clone());
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
            }
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
            }
        }
        out
    }
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

/// A fast, non-cryptographic fingerprint of `bytes` (no extra dependency).
pub(crate) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use karet_core::{Change, LineCol, Range, TextEdit};

    /// Save `bytes` (parsed via `from_bytes`) to a fresh temp file and return the
    /// raw bytes read back, or `None` if the environment lacks a temp dir.
    fn round_trip(input: &[u8]) -> Option<(Vec<u8>, bool)> {
        let dir = tempfile::tempdir().ok()?;
        let path = dir.path().join("file.txt");
        let mut buf = TextBuffer::from_bytes(input).ok()?;
        let saved = buf.save(&path);
        assert!(saved.is_ok(), "save should succeed");
        let raw = std::fs::read(&path).ok()?;
        Some((raw, buf.is_dirty()))
    }

    #[test]
    fn lf_round_trips_and_clears_dirty() {
        if let Some((raw, dirty)) = round_trip(b"a\nb\n") {
            assert_eq!(raw, b"a\nb\n");
            assert!(!dirty, "save clears dirty");
        }
    }

    #[test]
    fn crlf_round_trips() {
        // Loaded as CRLF, normalized to LF in memory, must save back as CRLF.
        if let Some((raw, _)) = round_trip(b"a\r\nb\r\n") {
            assert_eq!(raw, b"a\r\nb\r\n");
        }
    }

    #[test]
    fn bom_round_trips() {
        let mut input = vec![0xEF, 0xBB, 0xBF];
        input.extend_from_slice(b"hi\n");
        if let Some((raw, _)) = round_trip(&input) {
            assert_eq!(raw, input.as_slice());
        }
    }

    #[test]
    fn no_trailing_newline_preserved() {
        if let Some((raw, _)) = round_trip(b"abc") {
            assert_eq!(raw, b"abc");
        }
    }

    #[test]
    fn edit_then_save_is_clean_with_fingerprint() {
        let Ok(dir) = tempfile::tempdir() else {
            return;
        };
        let path = dir.path().join("edit.txt");
        let mut buf = TextBuffer::from_bytes(b"hello\n").unwrap_or_default();
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
        assert!(buf.apply_simple(&change).is_ok());
        assert!(buf.is_dirty());
        assert!(buf.save(&path).is_ok());
        assert!(!buf.is_dirty());
        assert!(buf.saved_state().is_some());
        assert_eq!(std::fs::read(&path).unwrap_or_default(), b"hello!\n");
    }
}
