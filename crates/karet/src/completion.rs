//! Completion UI state and pure helpers (issue #57, #371).
//!
//! The app talks to LSP only through the session seam: a trigger sends
//! [`karet_session::LspCommand::Completion`], the answering
//! `Event::Completions` (tagged with the request id, document, and version)
//! fills [`CompletionUi`], and the popup renders through
//! `karet_widgets::completion`. The selected item's deferred documentation and
//! additional edits arrive through `LspCommand::ResolveCompletion`. Everything
//! stateful lives on `App`; this module holds the state types and the pure
//! logic (trigger classification, edit-range and additional-edit math, the
//! syntax-error gate) so they are unit-testable without an `App`.

use karet_core::CompletionItem;
use karet_core::CompletionTrigger;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_editor::editing::MultiEdit;
use karet_editor::editing::map_position;
use karet_editor::editing::map_selection;
use karet_session::DocumentId;
use karet_session::RequestId;
use karet_widgets::CompletionState;

mod snippet;

pub(crate) use snippet::SnippetSession;

/// The characters that re-request completions before the document's server has
/// said which of its own do: `.` for member access and the second `:` of a `::`
/// path -- servers narrow the candidate set at these boundaries. Once a server
/// answered, its own trigger characters replace these.
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
    /// The caret the request was made at; a server's edit ranges end here.
    pub position: LineCol,
}

/// The open completion popup.
#[derive(Debug)]
pub(crate) struct CompletionUi {
    /// The candidate items (edit ranges already in buffer coordinates).
    pub items: Vec<CompletionItem>,
    /// The popup's selection/scroll state.
    pub list: CompletionState,
    /// The document the popup completes in.
    pub doc: DocumentId,
    /// Where the completed word starts; text from here to the caret is the
    /// live filter.
    pub anchor: LineCol,
    /// The filter at the last render, to reset the selection when it changes.
    pub last_filter: String,
    /// How candidates remain attached to the editor while it changes.
    pub mode: CompletionMode,
    /// The caret the answering request was made at: the items' edit ranges
    /// end there, and stretch by whatever was typed since.
    pub requested_at: LineCol,
    /// Whether the server left candidates out, so a changed filter asks again.
    pub incomplete: bool,
    /// The filter the items were asked for, so an incomplete list is asked
    /// again only when it changes.
    pub queried_filter: String,
    /// Resolves in flight: the request and the item (an index into `items`)
    /// it fills in.
    pub resolving: Vec<(RequestId, usize)>,
}

impl CompletionUi {
    /// A popup over `items` in `doc`, anchored at `anchor`, for a request made
    /// at `requested_at`, nothing selected beyond the first row and nothing
    /// being resolved.
    pub(crate) fn new(
        doc: DocumentId,
        anchor: LineCol,
        requested_at: LineCol,
        items: Vec<CompletionItem>,
        mode: CompletionMode,
    ) -> Self {
        Self {
            items,
            list: CompletionState::default(),
            doc,
            anchor,
            last_filter: String::new(),
            mode,
            requested_at,
            incomplete: false,
            queried_filter: String::new(),
            resolving: Vec::new(),
        }
    }
}

/// An accepted item whose additional edits were still deferred at the accept:
/// they follow as their own change once the resolve answers, if the document
/// has not moved on since.
#[derive(Clone, Debug)]
pub(crate) struct AcceptFollowUp {
    /// The resolve request whose answer carries them.
    pub id: RequestId,
    /// The document the item was accepted into.
    pub doc: DocumentId,
    /// The tab's next version right after the accept; any other version at
    /// the answer means the user edited since, and the edits are dropped.
    pub version: u64,
    /// The accept's own edits, in the coordinates before it applied, which the
    /// additional edits are mapped through.
    pub applied: Vec<TextEdit>,
}

/// The range an accept of `item` replaces at the primary caret: the item's
/// edit range (its replace range with `replace`, when it has one), stretched
/// by whatever was typed between `requested_at` and `caret`; or, for an item
/// without an edit, the typed word from `anchor` to `caret`.
///
/// An edit range that no longer holds the caret on its own line -- the user
/// deleted back past its start -- falls back to the typed word too. `None`
/// when even that does not hold (the caret left the anchored span).
pub(crate) fn edit_range(
    item: &CompletionItem,
    anchor: LineCol,
    requested_at: LineCol,
    caret: LineCol,
    replace: bool,
) -> Option<Range> {
    let typed_word = accept_range(anchor, caret)?;
    let base = match (replace, item.replace_range, &item.edit) {
        (true, Some(range), _) => range,
        (_, _, Some(edit)) => edit.range,
        _ => return Some(typed_word),
    };
    let one_line = base.start.line == caret.line
        && base.end.line == caret.line
        && requested_at.line == caret.line;
    if !one_line || base.start.col > caret.col {
        return Some(typed_word);
    }
    let end = i64::from(base.end.col) + i64::from(caret.col) - i64::from(requested_at.col);
    let end = u32::try_from(end).unwrap_or(0).max(caret.col);
    Some(Range {
        start: base.start,
        end: LineCol::new(caret.line, end),
    })
}

/// Whether two edits touch the same text: they overlap, or insert at the same
/// spot (whose order nothing decides).
fn collide(a: &TextEdit, b: &TextEdit) -> bool {
    if a.range.is_empty() && b.range.is_empty() {
        return a.range.start == b.range.start;
    }
    a.range.start < b.range.end && b.range.start < a.range.end
        || a.range.is_empty() && a.range.start == b.range.start
        || b.range.is_empty() && b.range.start == a.range.start
}

/// `extra` with every edit that collides with one of `edits` (or with an
/// earlier one of its own) left out, sorted. LSP promises additional edits
/// never overlap the main edit; one that does is dropped rather than letting
/// it fail the whole accept.
fn compatible(edits: &[TextEdit], extra: &[TextEdit]) -> Vec<TextEdit> {
    let mut kept: Vec<TextEdit> = Vec::with_capacity(extra.len());
    for edit in extra {
        if !edits.iter().chain(&kept).any(|other| collide(edit, other)) {
            kept.push(edit.clone());
        }
    }
    kept.sort_by_key(|edit| (edit.range.start, edit.range.end));
    kept
}

/// `extra`, in the coordinates before `applied` applied, moved to the
/// coordinates after it -- dropping any that collide with it.
pub(crate) fn after_applied(applied: &[TextEdit], extra: &[TextEdit]) -> Vec<TextEdit> {
    compatible(applied, extra)
        .into_iter()
        .map(|edit| TextEdit {
            range: Range {
                start: map_position(edit.range.start, applied),
                end: map_position(edit.range.end, applied),
            },
            new_text: edit.new_text,
        })
        .collect()
}

/// `multi` with an accepted item's additional edits applied in the same atomic
/// change, its cursors moved past them (an import above the caret pushes it
/// down a line). `extra` is in the same base coordinates as `multi`'s change.
pub(crate) fn with_additional_edits(multi: MultiEdit, extra: &[TextEdit]) -> MultiEdit {
    let MultiEdit {
        mut change,
        mut cursors,
    } = multi;
    let kept = compatible(&change.edits, extra);
    if kept.is_empty() {
        return MultiEdit { change, cursors };
    }
    let shifted = after_applied(&change.edits, &kept);
    for selection in &mut cursors.selections {
        *selection = map_selection(*selection, &shifted);
    }
    change.edits.extend(kept);
    change
        .edits
        .sort_by_key(|edit| (edit.range.start, edit.range.end));
    MultiEdit { change, cursors }
}

/// The trigger typing `c` asks for completions with, if it asks at all: one of
/// the server's own `known` trigger characters, or -- before any server
/// said which -- `.` and the second `:` of `::` (`second_colon`), sent as an
/// ordinary invocation since no server named them.
pub(crate) fn trigger_for(
    c: char,
    known: &[char],
    second_colon: bool,
) -> Option<CompletionTrigger> {
    if known.contains(&c) {
        return Some(CompletionTrigger::Character(c));
    }
    let fallback = c == TRIGGER_DOT || (c == TRIGGER_COLON && second_colon);
    (known.is_empty() && fallback).then_some(CompletionTrigger::Invoked)
}

/// Whether typing `c` over `item` accepts it first: one of its commit
/// characters (else the server's list-wide `defaults`) that is not an
/// identifier character -- a word character always narrows the filter.
pub(crate) fn commits(item: &CompletionItem, c: char, defaults: &[char]) -> bool {
    !is_word_char(c) && item.commits_on(c, defaults)
}

/// How an open completion popup filters and tracks the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CompletionMode {
    /// Ordinary language-server completion: fuzzy-filter the anchor-to-caret prefix.
    Filtered,
    /// Spelling corrections: show every suggestion while the caret stays fixed at
    /// the end of the diagnosed word.
    Spelling { caret: LineCol },
}

/// Whether `c` is an identifier character for auto-trigger purposes: the
/// editor's word-boundary vocabulary.
pub(crate) use karet_core::is_word_char;

/// Whether any syntax-error line range in `errors` (inclusive `(start, end)`
/// rows) covers `line` — the "no outright errors" auto-trigger gate.
pub(crate) fn line_has_syntax_error(errors: &[(u32, u32)], line: u32) -> bool {
    errors
        .iter()
        .any(|&(start, end)| start <= line && line <= end)
}

pub(crate) use karet_editor::editing::caret_after_insert;
pub(crate) use karet_widgets::completion::accept_range;
pub(crate) use karet_widgets::completion::caret_still_anchored;

#[cfg(test)]
mod tests {
    use karet_core::Range;

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

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn span(start: LineCol, end: LineCol) -> Range {
        Range { start, end }
    }

    fn insert(pos: LineCol, text: &str) -> TextEdit {
        TextEdit {
            range: span(pos, pos),
            new_text: text.to_owned(),
        }
    }

    fn with_edit(range: Range, replace: Option<Range>) -> CompletionItem {
        CompletionItem {
            edit: Some(TextEdit {
                range,
                new_text: "push".to_owned(),
            }),
            replace_range: replace,
            ..CompletionItem::new("push", karet_core::CompletionKind::Method)
        }
    }

    /// Falsified by an accept that ignores the server's range (replacing only
    /// the typed word), or one that forgets what was typed after the request.
    #[test]
    fn an_edit_range_stretches_by_what_was_typed_since_the_request() {
        // `v.pu|` asked at col 4 with the server's range covering `pu`;
        // the user then typed `s` (caret at 5).
        let item = with_edit(span(at(0, 2), at(0, 4)), None);
        assert_eq!(
            edit_range(&item, at(0, 2), at(0, 4), at(0, 5), false),
            Some(span(at(0, 2), at(0, 5)))
        );
        // A server range starting before the typed word (`?.` rewriting `.`)
        // is honoured as given.
        let item = with_edit(span(at(0, 1), at(0, 4)), None);
        assert_eq!(
            edit_range(&item, at(0, 2), at(0, 4), at(0, 4), false),
            Some(span(at(0, 1), at(0, 4)))
        );
        // Deleting back after the request shrinks it again.
        let item = with_edit(span(at(0, 2), at(0, 4)), None);
        assert_eq!(
            edit_range(&item, at(0, 2), at(0, 4), at(0, 3), false),
            Some(span(at(0, 2), at(0, 3)))
        );
    }

    /// Insert mode keeps the word after the caret; replace mode swallows it.
    #[test]
    fn replace_takes_the_replace_range_and_insert_the_insert_range() {
        // `pu|sh_back` asked at col 2.
        let item = with_edit(span(at(0, 0), at(0, 2)), Some(span(at(0, 0), at(0, 9))));
        assert_eq!(
            edit_range(&item, at(0, 0), at(0, 2), at(0, 2), false),
            Some(span(at(0, 0), at(0, 2)))
        );
        assert_eq!(
            edit_range(&item, at(0, 0), at(0, 2), at(0, 2), true),
            Some(span(at(0, 0), at(0, 9)))
        );
        // Replace on an item with only one range uses that range.
        let item = with_edit(span(at(0, 0), at(0, 2)), None);
        assert_eq!(
            edit_range(&item, at(0, 0), at(0, 2), at(0, 2), true),
            Some(span(at(0, 0), at(0, 2)))
        );
    }

    /// Without an edit, or with one the caret no longer sits in, the accept
    /// replaces the typed word; with the caret off the anchored span, nothing.
    #[test]
    fn without_a_usable_edit_the_typed_word_is_replaced() {
        let plain = CompletionItem::new("push", karet_core::CompletionKind::Method);
        assert_eq!(
            edit_range(&plain, at(0, 2), at(0, 4), at(0, 5), false),
            Some(span(at(0, 2), at(0, 5)))
        );
        let elsewhere = with_edit(span(at(3, 0), at(3, 2)), None);
        assert_eq!(
            edit_range(&elsewhere, at(0, 2), at(0, 4), at(0, 4), false),
            Some(span(at(0, 2), at(0, 4)))
        );
        // Backspaced past the server range's start (but not the anchor).
        let late = with_edit(span(at(0, 4), at(0, 6)), None);
        assert_eq!(
            edit_range(&late, at(0, 2), at(0, 6), at(0, 3), false),
            Some(span(at(0, 2), at(0, 3)))
        );
        assert_eq!(
            edit_range(&plain, at(0, 2), at(0, 4), at(1, 0), false),
            None
        );
    }

    /// Falsified by an import that lands without moving the caret down past
    /// it, or by an additional edit overlapping the main one being kept.
    #[test]
    fn additional_edits_join_the_accept_and_push_the_caret() {
        let main = TextEdit {
            range: span(at(2, 4), at(2, 6)),
            new_text: "HashMap".to_owned(),
        };
        let multi = MultiEdit {
            change: karet_core::Change::new(7, vec![main.clone()]),
            cursors: karet_core::CursorState {
                selections: vec![karet_core::Selection::caret(at(2, 11))],
                primary: 0,
            },
        };
        let import = insert(at(0, 0), "use std::collections::HashMap;\n");
        let clash = TextEdit {
            range: span(at(2, 5), at(2, 8)),
            new_text: "x".to_owned(),
        };
        let out = with_additional_edits(multi, &[clash, import.clone()]);
        assert_eq!(out.change.base_version, 7);
        assert_eq!(out.change.edits, vec![import, main]);
        assert_eq!(out.cursors.selections[0].head, at(3, 11));
    }

    /// Deferred additional edits are moved past the accept that already
    /// applied: one after it on the same line shifts by the inserted text.
    #[test]
    fn deferred_edits_map_through_the_applied_accept() {
        let applied = vec![TextEdit {
            range: span(at(1, 0), at(1, 2)),
            new_text: "HashMap".to_owned(),
        }];
        let after = insert(at(1, 4), ";");
        let above = insert(at(0, 0), "use x;\n");
        let inside = insert(at(1, 1), "!");
        let moved = after_applied(&applied, &[after, above.clone(), inside]);
        assert_eq!(moved, vec![above, insert(at(1, 9), ";")]);
    }

    /// Two insertions at one spot have no order to apply in; the second is
    /// dropped rather than failing the batch.
    #[test]
    fn colliding_insertions_keep_only_the_first() {
        let main = insert(at(0, 0), "a");
        let other = insert(at(0, 0), "b");
        assert!(after_applied(&[main], &[other]).is_empty());
    }

    #[test]
    fn a_servers_trigger_characters_replace_the_fallback_set() {
        assert_eq!(
            trigger_for('.', &[], false),
            Some(CompletionTrigger::Invoked)
        );
        assert_eq!(
            trigger_for(':', &[], true),
            Some(CompletionTrigger::Invoked)
        );
        assert_eq!(trigger_for(':', &[], false), None);
        assert_eq!(trigger_for('<', &[], false), None);
        // A server that named its own: only those, sent as what they are.
        let known = ['<', ':'];
        assert_eq!(
            trigger_for('<', &known, false),
            Some(CompletionTrigger::Character('<'))
        );
        assert_eq!(
            trigger_for(':', &known, false),
            Some(CompletionTrigger::Character(':'))
        );
        assert_eq!(trigger_for('.', &known, false), None);
    }

    #[test]
    fn commit_characters_never_include_word_characters() {
        let item = CompletionItem {
            commit_characters: Some(vec!['(', 'a']),
            ..CompletionItem::new("f", karet_core::CompletionKind::Function)
        };
        assert!(commits(&item, '(', &[]));
        assert!(
            !commits(&item, 'a', &[]),
            "a word character narrows instead"
        );
        let plain = CompletionItem::new("x", karet_core::CompletionKind::Variable);
        assert!(commits(&plain, ';', &[';']));
        assert!(!commits(&plain, ';', &[]));
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
