//! Completion UI state and pure helpers (issue #57).
//!
//! The app talks to LSP only through the session seam: a trigger sends
//! [`karet_session::Command::Completion`], the answering
//! `Event::Completions` (tagged with the request id, document, and version)
//! fills [`CompletionUi`], and the popup renders through
//! `karet_widgets::completion`. Everything stateful lives on `App`; this module
//! holds the state types and the pure logic (trigger classification, prefix
//! math, the syntax-error gate) so they are unit-testable without an `App`.

use karet_core::LineCol;
use karet_core::Range;
use karet_session::DocumentId;
use karet_session::RequestId;
use karet_widgets::CompletionState;

/// The set of characters that re-request completions instead of client-side
/// filtering (v1 static set; `.` for member access and the second `:` of a
/// `::` path — servers narrow the candidate set at these boundaries).
pub(crate) const TRIGGER_DOT: char = '.';

/// See [`TRIGGER_DOT`]; a colon only triggers as the second char of `::`.
pub(crate) const TRIGGER_COLON: char = ':';

/// An in-flight completion request awaiting its `Event::Completions`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingCompletion {
    /// The request id the answering event must carry.
    pub id: RequestId,
    /// The document the request targeted.
    pub doc: DocumentId,
    /// Where the word being completed starts; the filter is the text between
    /// here and the caret.
    pub anchor: LineCol,
}

/// The open completion popup.
#[derive(Debug)]
pub(crate) struct CompletionUi {
    /// The candidate items (edit ranges already in buffer coordinates).
    pub items: Vec<karet_core::CompletionItem>,
    /// The popup's selection/scroll state.
    pub list: CompletionState,
    /// The document the popup completes in.
    pub doc: DocumentId,
    /// Where the completed word starts; text from here to the caret is the
    /// live filter, and accepting replaces exactly this span.
    pub anchor: LineCol,
    /// The filter at the last render, to reset the selection when it changes.
    pub last_filter: String,
}

/// Whether `c` is an identifier character for auto-trigger purposes (matches
/// the editor's word-boundary vocabulary).
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether any syntax-error line range in `errors` (inclusive `(start, end)`
/// rows) covers `line` — the "no outright errors" auto-trigger gate.
pub(crate) fn line_has_syntax_error(errors: &[(u32, u32)], line: u32) -> bool {
    errors
        .iter()
        .any(|&(start, end)| start <= line && line <= end)
}

/// The caret position after inserting `text` at `start` (multi-line aware).
pub(crate) fn caret_after_insert(start: LineCol, text: &str) -> LineCol {
    let newlines = u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX);
    let tail = text.rsplit('\n').next().unwrap_or("");
    let tail_cols = u32::try_from(tail.chars().count()).unwrap_or(u32::MAX);
    if newlines == 0 {
        LineCol::new(start.line, start.col.saturating_add(tail_cols))
    } else {
        LineCol::new(start.line.saturating_add(newlines), tail_cols)
    }
}

/// The span an accept replaces: from the popup's `anchor` to the current
/// `caret` (the typed prefix). `None` when the caret has wandered somewhere an
/// accept no longer makes sense (other line, or before the anchor).
pub(crate) fn accept_range(anchor: LineCol, caret: LineCol) -> Option<Range> {
    (caret.line == anchor.line && caret.col >= anchor.col).then_some(Range {
        start: anchor,
        end: caret,
    })
}

/// Whether the popup/pending request is still valid for `caret` — same
/// document line as the anchor and not before it. Any other movement
/// dismisses.
pub(crate) fn caret_still_anchored(anchor: LineCol, caret: LineCol) -> bool {
    caret.line == anchor.line && caret.col >= anchor.col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_chars_cover_identifiers() {
        assert!(is_word_char('a'));
        assert!(is_word_char('Z'));
        assert!(is_word_char('9'));
        assert!(is_word_char('_'));
        assert!(is_word_char('é')); // unicode identifiers count
        assert!(!is_word_char('.'));
        assert!(!is_word_char(' '));
        assert!(!is_word_char('('));
    }

    #[test]
    fn the_gate_flags_only_covered_lines() {
        let errors = [(2, 4), (9, 9)];
        assert!(!line_has_syntax_error(&errors, 0));
        assert!(line_has_syntax_error(&errors, 2));
        assert!(line_has_syntax_error(&errors, 3));
        assert!(line_has_syntax_error(&errors, 4));
        assert!(!line_has_syntax_error(&errors, 5));
        assert!(line_has_syntax_error(&errors, 9));
        assert!(!line_has_syntax_error(&[], 1));
    }

    #[test]
    fn caret_lands_after_the_inserted_text() {
        let start = LineCol::new(3, 4);
        assert_eq!(caret_after_insert(start, "push"), LineCol::new(3, 8));
        assert_eq!(caret_after_insert(start, ""), start);
        // Multi-line inserts land at the end of the last line.
        assert_eq!(
            caret_after_insert(start, "if cond {\n    \n}"),
            LineCol::new(5, 1)
        );
        // Char counting, not bytes: emoji are one column here.
        assert_eq!(caret_after_insert(start, "😀x"), LineCol::new(3, 6));
    }

    #[test]
    fn accepts_replace_anchor_to_caret_only_when_anchored() {
        let anchor = LineCol::new(1, 4);
        assert_eq!(
            accept_range(anchor, LineCol::new(1, 7)),
            Some(Range {
                start: anchor,
                end: LineCol::new(1, 7),
            })
        );
        assert_eq!(
            accept_range(anchor, anchor),
            Some(Range {
                start: anchor,
                end: anchor,
            })
        );
        assert_eq!(accept_range(anchor, LineCol::new(1, 3)), None);
        assert_eq!(accept_range(anchor, LineCol::new(2, 8)), None);
    }

    #[test]
    fn anchoring_follows_the_same_rule() {
        let anchor = LineCol::new(1, 4);
        assert!(caret_still_anchored(anchor, LineCol::new(1, 4)));
        assert!(caret_still_anchored(anchor, LineCol::new(1, 9)));
        assert!(!caret_still_anchored(anchor, LineCol::new(1, 3)));
        assert!(!caret_still_anchored(anchor, LineCol::new(0, 4)));
        assert!(!caret_still_anchored(anchor, LineCol::new(2, 4)));
    }
}
