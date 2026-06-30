//! Strict UTF-8 loading with line-ending and BOM detection.
//!
//! karet supports exactly one text encoding: UTF-8. Loading is **strict** — a file
//! that is not valid UTF-8 is reported as [`LoadError::NotUtf8`] rather than being
//! lossily decoded, so the caller can fall back to a hex view. A leading UTF-8 BOM
//! is detected and stripped (and re-emitted on save); the line ending is detected
//! (`\n` vs `\r\n`, or mixed) and the buffer is normalized to LF in memory, with
//! the detected ending preserved for a round-tripping save.

use crate::TextBuffer;
use crate::save::{SavedState, hash_bytes};
use std::path::Path;
use std::time::SystemTime;

/// The line ending a buffer was loaded with and is saved with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eol {
    /// Unix `\n`.
    #[default]
    Lf,
    /// Windows `\r\n`.
    Crlf,
}

impl Eol {
    /// The byte sequence for this line ending.
    #[must_use]
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Eol::Lf => b"\n",
            Eol::Crlf => b"\r\n",
        }
    }
}

/// The detected encoding. Only UTF-8 is supported; the variant records whether the
/// file carried a byte-order mark, which is re-emitted on save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    /// Plain UTF-8, no BOM.
    #[default]
    Utf8,
    /// UTF-8 with a leading `EF BB BF` byte-order mark.
    Utf8Bom,
}

/// Why a file could not be loaded as a karet text buffer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// The bytes are not valid UTF-8; `offset` is the first invalid byte.
    #[error("not valid utf-8 at byte {offset}")]
    NotUtf8 {
        /// Byte offset of the first invalid sequence.
        offset: usize,
    },
    /// An I/O error while reading the file.
    #[error("load i/o error: {0}")]
    Io(String),
}

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

impl TextBuffer {
    /// Build a buffer from in-memory file bytes: strip a BOM, validate UTF-8
    /// strictly, detect the line ending, and normalize to LF in memory.
    ///
    /// # Errors
    /// Returns [`LoadError::NotUtf8`] if the bytes (after any BOM) are not valid
    /// UTF-8.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LoadError> {
        let (encoding, rest) = if bytes.starts_with(&BOM) {
            (Encoding::Utf8Bom, &bytes[BOM.len()..])
        } else {
            (Encoding::Utf8, bytes)
        };
        let text = std::str::from_utf8(rest).map_err(|e| LoadError::NotUtf8 {
            offset: e.valid_up_to(),
        })?;
        let (eol, mixed_eol) = detect_eol(rest);
        let rope = if rest.contains(&b'\r') {
            ropey::Rope::from_str(&normalize_lf(text))
        } else {
            ropey::Rope::from_str(text)
        };
        Ok(Self {
            rope,
            eol,
            encoding,
            mixed_eol,
            ..Self::default()
        })
    }

    /// Load `path` strictly as UTF-8, recording the on-disk fingerprint so a
    /// file-watcher can later distinguish the editor's own writes from external
    /// edits.
    ///
    /// # Errors
    /// Returns [`LoadError::Io`] if the file cannot be read, or
    /// [`LoadError::NotUtf8`] if its contents are not valid UTF-8.
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        let bytes = std::fs::read(path).map_err(|e| LoadError::Io(e.to_string()))?;
        let mut buf = Self::from_bytes(&bytes)?;
        if let Ok(meta) = std::fs::metadata(path) {
            buf.saved_state = Some(SavedState {
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: meta.len(),
                hash: hash_bytes(&bytes),
            });
        }
        Ok(buf)
    }
}

/// Detect the dominant line ending and whether the file mixed `\n` and `\r\n`.
fn detect_eol(bytes: &[u8]) -> (Eol, bool) {
    let mut lf = 0usize;
    let mut crlf = 0usize;
    for i in memchr::memchr_iter(b'\n', bytes) {
        if i > 0 && bytes[i - 1] == b'\r' {
            crlf += 1;
        } else {
            lf += 1;
        }
    }
    let mixed = lf > 0 && crlf > 0;
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };
    (eol, mixed)
}

/// Normalize `\r\n` to `\n`, leaving any lone `\r` (old-Mac) untouched.
fn normalize_lf(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for i in memchr::memchr_iter(b'\n', bytes) {
        if i > 0 && bytes[i - 1] == b'\r' {
            // Emit everything up to (not including) the `\r`, then a bare `\n`.
            out.push_str(&text[last..i - 1]);
            out.push('\n');
            last = i + 1;
        }
        // A lone `\n` is left in place; it is flushed as part of a later segment.
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_lf_loads_clean() {
        let b = TextBuffer::from_bytes(b"a\nb\n").unwrap_or_default();
        assert_eq!(b.eol(), Eol::Lf);
        assert_eq!(b.encoding(), Encoding::Utf8);
        assert!(!b.has_mixed_eol());
        assert_eq!(b.line_count(), 3);
        assert!(!b.is_dirty());
    }

    #[test]
    fn crlf_detected_and_normalized() {
        let b = TextBuffer::from_bytes(b"a\r\nb\r\n").unwrap_or_default();
        assert_eq!(b.eol(), Eol::Crlf);
        assert!(!b.has_mixed_eol());
        // Normalized to LF in memory: no `\r` remains.
        assert_eq!(b.text(), "a\nb\n");
    }

    #[test]
    fn mixed_eol_flagged() {
        let b = TextBuffer::from_bytes(b"a\r\nb\nc\r\n").unwrap_or_default();
        assert!(b.has_mixed_eol());
        assert_eq!(b.eol(), Eol::Crlf); // 2 CRLF vs 1 LF → majority CRLF
        assert_eq!(b.text(), "a\nb\nc\n");
    }

    #[test]
    fn bom_stripped_and_recorded() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"hi");
        let b = TextBuffer::from_bytes(&bytes).unwrap_or_default();
        assert_eq!(b.encoding(), Encoding::Utf8Bom);
        assert_eq!(b.text(), "hi"); // BOM not part of content
    }

    #[test]
    fn invalid_utf8_reports_offset() {
        // Valid "ab" then an invalid lone continuation byte.
        assert_eq!(
            TextBuffer::from_bytes(b"ab\xFF").err(),
            Some(LoadError::NotUtf8 { offset: 2 })
        );
    }

    #[test]
    fn no_trailing_newline_preserved() {
        let b = TextBuffer::from_bytes(b"no newline").unwrap_or_default();
        assert_eq!(b.text(), "no newline");
        assert_eq!(b.line_count(), 1);
    }

    #[test]
    fn empty_file() {
        let b = TextBuffer::from_bytes(b"").unwrap_or_default();
        assert_eq!(b.len_bytes(), 0);
        assert!(!b.is_dirty());
    }
}
