//! Whole-document scans a code view re-runs once per buffer version, gated so
//! the common case reads the rope in place instead of copying the document.
//!
//! Both scans need one contiguous `&str` once they run (a line parser and a
//! CommonMark parser), so each first asks the rope whether the one marker it
//! cannot do without appears at all. Nearly every file has no merge-conflict
//! marker, and a Markdown file with no `|` has no table, so those versions cost
//! no copy; the rest read the tab's shared [`TextSnapshot`], materialized at most
//! once per version for every reader of it.

use std::ops::RangeInclusive;

use karet_core::BytePos;
use karet_core::Decoration;
use karet_core::Span;
use karet_text::TextBuffer;
use karet_text::TextSnapshot;

/// The start marker every complete merge conflict opens with (in column zero).
const CONFLICT_START: &str = "<<<<<<<";

/// The pipe every GFM table's delimiter row carries.
const TABLE_PIPE: &str = "|";

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

/// [`karet_markdown::table_line_ranges`] for `buffer`, whose text `text` is.
pub(super) fn table_line_ranges(
    buffer: &TextBuffer,
    text: &TextSnapshot,
) -> Vec<RangeInclusive<u32>> {
    if !contains(buffer, TABLE_PIPE) {
        return Vec::new();
    }
    karet_markdown::table_line_ranges(text)
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

    #[test]
    fn markdown_without_a_pipe_is_never_copied() {
        let (found, copied) = scan("# Title\n\n- item\n", table_line_ranges);
        assert!(found.is_empty());
        assert!(!copied, "no pipe: no table, and no copy");
    }

    #[test]
    fn a_table_is_still_found() {
        let source = "intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let (found, _) = scan(source, table_line_ranges);
        assert_eq!(found, karet_markdown::table_line_ranges(source));
        assert_eq!(found, vec![2..=4]);
    }
}
