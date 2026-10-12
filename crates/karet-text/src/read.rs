//! Read accessors that work on the rope directly, so a hot path can slice or
//! search the buffer without first copying the whole document into a `String`,
//! plus [`TextSnapshot`], the shared, lazily materialized whole-text view for the
//! consumers that genuinely need one contiguous `&str`.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::OnceLock;

use karet_core::BytePos;
use karet_core::Span;

use crate::TextBuffer;

impl TextBuffer {
    /// The text in the byte [`Span`] `[start, end)`, copying only that range.
    ///
    /// `None` when the span is reversed, runs past the end of the buffer, or
    /// either end falls inside a multi-byte character.
    #[must_use]
    pub fn slice(&self, span: Span) -> Option<String> {
        let (start, end) = (span.start.0, span.end.0);
        if start > end || end > self.rope.len_bytes() {
            return None;
        }
        if !self.is_char_boundary(start) || !self.is_char_boundary(end) {
            return None;
        }
        Some(self.rope.byte_slice(start..end).to_string())
    }

    /// The byte offset of the first occurrence of `needle` lying wholly inside
    /// `within`, searching the rope chunk by chunk (no whole-document copy).
    ///
    /// An empty `needle` matches at `within.start`. A span reaching past the end
    /// of the buffer is clamped to it; a reversed span finds nothing.
    #[must_use]
    pub fn find(&self, needle: &str, within: Span) -> Option<BytePos> {
        let len = self.rope.len_bytes();
        let start = within.start.0;
        let end = within.end.0.min(len);
        if start > end {
            return None;
        }
        let needle = needle.as_bytes();
        if needle.is_empty() {
            return Some(BytePos(start));
        }
        if end - start < needle.len() {
            return None;
        }
        let (chunks, mut chunk_start, _, _) = self.rope.chunks_at_byte(start);
        // The last `needle.len() - 1` bytes before the current chunk, so a match
        // straddling a chunk boundary is still found. Bounded by the needle.
        let mut carry: Vec<u8> = Vec::with_capacity(2 * needle.len());
        let mut carry_start = start;
        for chunk in chunks {
            if chunk_start >= end {
                break;
            }
            let bytes = chunk.as_bytes();
            let from = start.saturating_sub(chunk_start);
            let to = (end - chunk_start).min(bytes.len());
            let piece = bytes.get(from..to).unwrap_or_default();
            let piece_start = chunk_start + from;
            if !carry.is_empty() {
                let head = piece.get(..needle.len() - 1).unwrap_or(piece);
                let boundary_len = carry.len();
                carry.extend_from_slice(head);
                if let Some(at) = memchr::memmem::find(&carry, needle)
                    && at < boundary_len
                {
                    return Some(BytePos(carry_start + at));
                }
                carry.truncate(boundary_len);
            }
            if let Some(at) = memchr::memmem::find(piece, needle) {
                return Some(BytePos(piece_start + at));
            }
            carry.extend_from_slice(piece);
            let keep = needle.len() - 1;
            if carry.len() > keep {
                let drop = carry.len() - keep;
                carry.drain(..drop);
            }
            carry_start = piece_start + piece.len() - carry.len();
            chunk_start += bytes.len();
        }
        None
    }

    /// A shared, read-only view of the whole text that copies it into one
    /// contiguous `&str` only when (and the first time) a consumer asks for it.
    ///
    /// Taking the snapshot is O(1) (it shares the rope), so a caller can refresh
    /// it on every edit and leave the O(document) copy to whichever consumer
    /// actually needs it; clones share that one copy.
    #[must_use]
    pub fn text_snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            rope: self.rope.clone(),
            text: Arc::new(OnceLock::new()),
        }
    }

    fn is_char_boundary(&self, byte: usize) -> bool {
        if byte >= self.rope.len_bytes() {
            return byte == self.rope.len_bytes();
        }
        self.rope.char_to_byte(self.rope.byte_to_char(byte)) == byte
    }
}

/// A cheap, shareable view of a [`TextBuffer`]'s text at one version.
///
/// Holds the rope (structurally shared, O(1) to take) and builds the contiguous
/// `String` lazily on the first [`as_str`](Self::as_str) (or deref). Clones share
/// both the rope and that one materialized copy, so several readers of the same
/// version pay for at most one whole-document allocation between them, and a
/// writer that only refreshes the snapshot pays for none.
#[derive(Clone)]
pub struct TextSnapshot {
    rope: ropey::Rope,
    text: Arc<OnceLock<Box<str>>>,
}

impl TextSnapshot {
    /// The whole text, materializing it on first use.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.text
            .get_or_init(|| self.rope.to_string().into_boxed_str())
    }

    /// Whether the contiguous copy has been built yet (by this snapshot or a
    /// clone of it). Lets a caller, or a test guarding a hot path, confirm that
    /// refreshing the snapshot did not copy the document.
    #[must_use]
    pub fn is_materialized(&self) -> bool {
        self.text.get().is_some()
    }

    /// The text's length in bytes, read from the rope (no materialization).
    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }
}

impl Default for TextSnapshot {
    fn default() -> Self {
        TextBuffer::new().text_snapshot()
    }
}

impl Deref for TextSnapshot {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for TextSnapshot {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for TextSnapshot {
    fn from(text: &str) -> Self {
        TextBuffer::from_text(text).text_snapshot()
    }
}

impl From<String> for TextSnapshot {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}

impl PartialEq<str> for TextSnapshot {
    fn eq(&self, other: &str) -> bool {
        self.len_bytes() == other.len() && self.as_str() == other
    }
}

impl PartialEq<&str> for TextSnapshot {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}

impl fmt::Debug for TextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextSnapshot")
            .field("len_bytes", &self.len_bytes())
            .field("materialized", &self.is_materialized())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize) -> Span {
        Span {
            start: BytePos(start),
            end: BytePos(end),
        }
    }

    /// A buffer large enough that ropey splits it over many chunks, with the
    /// needle planted so it straddles chunk boundaries at varying offsets.
    fn multi_chunk(needle: &str, at: &[usize], len: usize) -> String {
        let mut text = "x".repeat(len);
        for &pos in at {
            text.replace_range(pos..pos + needle.len(), needle);
        }
        text
    }

    #[test]
    fn slice_copies_only_the_range() {
        let b = TextBuffer::from_text("héllo\nworld");
        assert_eq!(b.slice(span(0, 6)).as_deref(), Some("héllo"));
        assert_eq!(b.slice(span(7, 12)).as_deref(), Some("world"));
        assert_eq!(b.slice(span(3, 3)).as_deref(), Some(""));
    }

    #[test]
    fn slice_rejects_bad_spans() {
        let b = TextBuffer::from_text("héllo");
        assert_eq!(b.slice(span(2, 1)), None, "reversed");
        assert_eq!(b.slice(span(0, 99)), None, "past the end");
        assert_eq!(b.slice(span(2, 4)), None, "starts inside `é`");
        assert_eq!(b.slice(span(0, 2)), None, "ends inside `é`");
    }

    #[test]
    fn find_respects_the_span() {
        let b = TextBuffer::from_text("ab ab ab");
        assert_eq!(b.find("ab", span(0, 8)), Some(BytePos(0)));
        assert_eq!(b.find("ab", span(1, 8)), Some(BytePos(3)));
        // A match must lie wholly inside the span.
        assert_eq!(b.find("ab", span(1, 4)), None);
        assert_eq!(b.find("ab", span(7, 8)), None);
        assert_eq!(b.find("zz", span(0, 8)), None);
    }

    #[test]
    fn find_edge_cases() {
        let b = TextBuffer::from_text("abc");
        assert_eq!(b.find("", span(2, 3)), Some(BytePos(2)));
        assert_eq!(b.find("c", span(0, 99)), Some(BytePos(2)), "end clamps");
        assert_eq!(b.find("a", span(2, 1)), None, "reversed");
        assert_eq!(TextBuffer::new().find("a", span(0, 0)), None);
    }

    #[test]
    fn find_matches_str_find_across_chunk_boundaries() {
        // Every offset in a window around each chunk boundary: if the boundary
        // carry were wrong, a straddling needle would be missed or misplaced.
        let needle = "<<<<<<<";
        let len = 20_000;
        let b = TextBuffer::from_text(&"x".repeat(len));
        let boundaries: Vec<usize> = {
            let mut out = Vec::new();
            let mut at = 0;
            for chunk in b.rope.chunks() {
                at += chunk.len();
                out.push(at);
            }
            out
        };
        assert!(boundaries.len() > 2, "the fixture must span several chunks");
        for &edge in boundaries.iter().take(3) {
            for pos in edge.saturating_sub(needle.len())..=edge {
                let text = multi_chunk(needle, &[pos], len);
                let b = TextBuffer::from_text(&text);
                assert_eq!(
                    b.find(needle, span(0, len)).map(|p| p.0),
                    text.find(needle),
                    "needle at {pos} (boundary {edge})"
                );
            }
        }
    }

    #[test]
    fn find_from_mid_document_skips_earlier_matches() {
        let needle = "needle";
        let text = multi_chunk(needle, &[100, 9_000, 15_000], 20_000);
        let b = TextBuffer::from_text(&text);
        assert_eq!(b.find(needle, span(101, 20_000)), Some(BytePos(9_000)));
        assert_eq!(b.find(needle, span(9_001, 20_000)), Some(BytePos(15_000)));
        assert_eq!(b.find(needle, span(9_001, 15_005)), None);
    }

    #[test]
    fn snapshot_materializes_lazily_and_shares_the_copy() {
        let b = TextBuffer::from_text("hello\nworld");
        let snapshot = b.text_snapshot();
        let clone = snapshot.clone();
        assert!(
            !snapshot.is_materialized(),
            "taking a snapshot copies nothing"
        );
        assert_eq!(snapshot.len_bytes(), 11);
        assert!(!snapshot.is_materialized(), "len_bytes reads the rope");
        assert_eq!(clone.as_str(), "hello\nworld");
        assert!(snapshot.is_materialized(), "clones share one copy");
        assert_eq!(&*snapshot, "hello\nworld");
    }

    #[test]
    fn snapshot_conversions() {
        let snapshot = TextSnapshot::from("abc");
        assert_eq!(snapshot.as_ref(), "abc");
        assert_eq!(TextSnapshot::from(String::from("abc")), "abc");
        assert!(snapshot == *"abc");
        let longer = TextSnapshot::from("abcd");
        assert!(longer != "abc");
        assert!(
            !longer.is_materialized(),
            "a length mismatch reads the rope only"
        );
        assert_eq!(TextSnapshot::default().as_str(), "");
        assert!(format!("{snapshot:?}").contains("len_bytes: 3"));
    }
}
