//! Whole-document scans a code view re-runs once per buffer version, gated so
//! the common case reads the rope in place instead of copying the document.
//!
//! The merge-conflict scan needs one contiguous `&str` once it runs (a line
//! parser), so it first asks the rope whether the one marker it cannot do
//! without appears at all. Nearly every file has no merge-conflict marker, so
//! those versions cost no copy; the rest read the tab's shared [`TextSnapshot`],
//! materialized at most once per version for every reader of it.
//!
//! Markdown table lines are not scanned here: the session's highlight worker
//! reads them off the tree it already parses and publishes them on the
//! document snapshot.

use karet_core::BytePos;
use karet_core::Decoration;
use karet_core::Span;
use karet_text::TextBuffer;
use karet_text::TextSnapshot;

/// The start marker every complete merge conflict opens with (in column zero).
const CONFLICT_START: &str = "<<<<<<<";

fn contains(buffer: &TextBuffer, needle: &str) -> bool {
    let whole = Span {
        start: BytePos(0),
        end: BytePos(buffer.len_bytes()),
    };
    buffer.find(needle, whole).is_some()
}

/// [`karet_editor::conflict_decorations`] for `buffer`, whose text `text` is.
pub(super) fn conflict_decorations(buffer: &TextBuffer, text: &TextSnapshot) -> Vec<Decoration> {
    if !contains(buffer, CONFLICT_START) {
        return Vec::new();
    }
    karet_editor::conflict_decorations(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan<T>(source: &str, run: impl Fn(&TextBuffer, &TextSnapshot) -> T) -> (T, bool) {
        let buffer = TextBuffer::from_text(source);
        let text = buffer.text_snapshot();
        let found = run(&buffer, &text);
        (found, text.is_materialized())
    }

    #[test]
    fn a_file_without_a_conflict_marker_is_never_copied() {
        let (found, copied) = scan("fn main() {}\n// <<<<<< six\n", conflict_decorations);
        assert!(found.is_empty());
        assert!(!copied, "no start marker: the rope check settles it");
    }

    #[test]
    fn a_conflict_is_still_decorated() {
        let source = "<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n";
        let (found, copied) = scan(source, conflict_decorations);
        assert_eq!(found, karet_editor::conflict_decorations(source));
        assert!(!found.is_empty());
        assert!(copied);
    }
}
