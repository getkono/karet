//! Turning "the text changed underneath the client" into an edit it can apply.
//!
//! A client applies its own edits optimistically, so the backend normally has
//! nothing to say about text. Sometimes it does: format-on-save rewrites a
//! buffer, an LSP rename lands, a file reloads after changing on disk, an undo
//! restores a previous state. Sending the whole document for those is correct but
//! costly, and undo is interactive — an undo that ships a megabyte is an undo the
//! user waits for.
//!
//! So the backend keeps the text at the version the client last knew and, when it
//! falls behind, derives one edit covering the difference.
//!
//! The derivation is common-prefix/common-suffix trimming rather than a real
//! diff. For the localized cases — an undo, a rename, a completion applied by the
//! server — that yields the minimal edit, which is what matters. For a whole-file
//! reformat it degrades to "replace nearly everything", which is an honest
//! description of a whole-file reformat.

use karet_core::Change;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;

/// One edit turning `old` into `new`, relative to `base_version`.
///
/// `None` when the texts are equal — there is nothing to tell the client.
pub(super) fn minimal_change(old: &str, new: &str, base_version: u64) -> Option<Change> {
    if old == new {
        return None;
    }
    let prefix = common_prefix(old, new);
    // The suffix may not overlap the prefix, or the replaced range would be
    // inverted — which happens whenever one text is a prefix of the other.
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let suffix = common_suffix(&old[prefix..], &new[prefix..], max_suffix);
    let start = position_of(old, prefix);
    let end = position_of(old, old.len() - suffix);
    let range = Range::new(start, end).ok()?;
    Some(Change::new(
        base_version,
        vec![TextEdit {
            range,
            new_text: new[prefix..new.len() - suffix].to_owned(),
        }],
    ))
}

/// The byte length of the longest common prefix, on a character boundary.
fn common_prefix(old: &str, new: &str) -> usize {
    let limit = old.len().min(new.len());
    let mut shared = 0;
    for (index, (left, right)) in old.bytes().zip(new.bytes()).enumerate() {
        if left != right || index >= limit {
            break;
        }
        shared = index + 1;
    }
    // Trimming mid-character would split a multi-byte scalar across the edit
    // boundary and produce invalid text on the client.
    while shared > 0 && !old.is_char_boundary(shared) {
        shared -= 1;
    }
    shared
}

/// The byte length of the longest common suffix, bounded by `max` and landing on
/// a character boundary.
fn common_suffix(old: &str, new: &str, max: usize) -> usize {
    let mut shared = 0;
    for (left, right) in old.bytes().rev().zip(new.bytes().rev()) {
        if left != right || shared >= max {
            break;
        }
        shared += 1;
    }
    while shared > 0 && !old.is_char_boundary(old.len() - shared) {
        shared -= 1;
    }
    shared
}

/// The line/column of byte offset `offset` in `text`.
///
/// Columns count Unicode scalar values, matching [`LineCol`]'s contract — a
/// column measured in bytes would land mid-character on any non-ASCII line.
fn position_of(text: &str, offset: usize) -> LineCol {
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let col = text[line_start..offset].chars().count() as u32;
    LineCol { line, col }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply `change` to `old` the way a client's rope would, so a test asserts
    /// convergence rather than the shape of an edit.
    ///
    /// The change is rebased onto the buffer's real version first: a rope refuses
    /// a change built against a version it is not at, and what is under test here
    /// is the edit's *content*, not its version bookkeeping.
    fn apply(old: &str, change: &Change) -> Option<String> {
        let mut buffer = karet_text::TextBuffer::from_bytes(old.as_bytes()).ok()?;
        let rebased = Change::new(buffer.version(), change.edits.clone());
        buffer
            .apply(&rebased, karet_text::EditContext::default())
            .ok()?;
        Some(buffer.text())
    }

    fn round_trip(old: &str, new: &str) -> Option<String> {
        let change = minimal_change(old, new, 1)?;
        apply(old, &change)
    }

    #[test]
    fn identical_text_produces_no_change() {
        assert!(minimal_change("same\n", "same\n", 1).is_none());
    }

    #[test]
    fn a_single_word_replacement_converges_and_stays_local() {
        let old = "fn alpha() {}\nfn beta() {}\n";
        let new = "fn alpha() {}\nfn gamma() {}\n";

        let change = minimal_change(old, new, 4);

        let Some(change) = change else {
            return;
        };
        assert_eq!(change.base_version, 4);
        assert_eq!(change.edits.len(), 1);
        // The edit must touch only the changed line, not the whole document.
        assert_eq!(change.edits[0].range.start.line, 1);
        assert_eq!(apply(old, &change).as_deref(), Some(new));
    }

    #[test]
    fn an_insertion_at_the_end_converges() {
        assert_eq!(
            round_trip("a\nb\n", "a\nb\nc\n").as_deref(),
            Some("a\nb\nc\n")
        );
    }

    #[test]
    fn an_insertion_at_the_start_converges() {
        assert_eq!(
            round_trip("b\nc\n", "a\nb\nc\n").as_deref(),
            Some("a\nb\nc\n")
        );
    }

    /// One text being a prefix of the other is where a naive prefix+suffix trim
    /// overlaps itself and inverts the range.
    #[test]
    fn a_pure_truncation_converges() {
        assert_eq!(round_trip("abcdef", "abc").as_deref(), Some("abc"));
    }

    #[test]
    fn a_pure_extension_converges() {
        assert_eq!(round_trip("abc", "abcdef").as_deref(), Some("abcdef"));
    }

    #[test]
    fn emptying_a_document_converges() {
        assert_eq!(round_trip("something\n", "").as_deref(), Some(""));
    }

    #[test]
    fn filling_an_empty_document_converges() {
        assert_eq!(
            round_trip("", "something\n").as_deref(),
            Some("something\n")
        );
    }

    /// Trimming mid-character would split a multi-byte scalar across the edit
    /// boundary and hand the client invalid text.
    #[test]
    fn a_multibyte_change_converges_without_splitting_a_character() {
        assert_eq!(
            round_trip("héllo wörld\n", "héllo wérld\n").as_deref(),
            Some("héllo wérld\n")
        );
    }

    #[test]
    fn a_change_among_emoji_converges() {
        assert_eq!(
            round_trip("a 👩‍💻 b\n", "a 🧑‍🚀 b\n").as_deref(),
            Some("a 🧑‍🚀 b\n")
        );
    }

    /// Columns are Unicode scalars, not bytes: a byte column on an accented line
    /// would place the edit in the wrong place.
    #[test]
    fn a_column_after_multibyte_text_counts_scalars_not_bytes() {
        let old = "héllo X\n";
        let new = "héllo Y\n";

        let Some(change) = minimal_change(old, new, 1) else {
            return;
        };

        assert_eq!(change.edits[0].range.start.col, 6);
        assert_eq!(apply(old, &change).as_deref(), Some(new));
    }

    /// A whole-file reformat has no shared prefix or suffix worth speaking of.
    /// It must still converge — this is the case the trimming degrades on, and
    /// degrading must mean "large edit", never "wrong edit".
    #[test]
    fn a_whole_file_reformat_converges() {
        let old = "fn a(){let x=1;}\nfn b(){let y=2;}\n";
        let new = "fn a() {\n    let x = 1;\n}\nfn b() {\n    let y = 2;\n}\n";

        assert_eq!(round_trip(old, new).as_deref(), Some(new));
    }

    #[test]
    fn a_change_spanning_many_lines_converges() {
        let old = "keep\nold one\nold two\nold three\nkeep\n";
        let new = "keep\nnew\nkeep\n";

        assert_eq!(round_trip(old, new).as_deref(), Some(new));
    }

    /// Repeated text is where prefix/suffix trimming can meet in the middle and
    /// over-count the shared region.
    #[test]
    fn repeated_text_converges() {
        assert_eq!(round_trip("aaaa", "aaaaa").as_deref(), Some("aaaaa"));
        assert_eq!(round_trip("aaaaa", "aaaa").as_deref(), Some("aaaa"));
    }

    #[test]
    fn a_change_that_only_adds_a_trailing_newline_converges() {
        assert_eq!(
            round_trip("no newline", "no newline\n").as_deref(),
            Some("no newline\n")
        );
    }
}
