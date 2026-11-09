//! The find bar's pure match arithmetic: which match is current, which ones a
//! Search in Selection scope counts, how that scope follows a replacement, and
//! the edits a replacement plans — split from the find/search module to respect
//! the per-file code-line ceiling.

use karet_core::BytePos;
use karet_core::Decoration;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_core::ThemeRole;
use karet_editor::editing;
use karet_search::SearchQuery;

/// The match the caret is in or the first one after it, wrapping to the first
/// when every match lies behind it.
pub(super) fn match_at_or_after(decos: &[Decoration], caret: LineCol) -> usize {
    decos
        .iter()
        .position(|deco| deco.range.start >= caret || deco.range.end > caret)
        .unwrap_or(0)
}

/// Paint the match at `current` with the current-match role and every other
/// one with the plain match role, so exactly one match reads as current.
pub(super) fn mark_current(decos: &mut [Decoration], current: usize) {
    for (i, deco) in decos.iter_mut().enumerate() {
        deco.role = Some(if i == current {
            ThemeRole::SearchMatchCurrent
        } else {
            ThemeRole::SearchMatch
        });
    }
}

/// Whether a match over `range` counts under the Search in Selection `scope`:
/// every match without one, otherwise only a match wholly inside it. An
/// empty-width match (a regex `^` or `$`) at `scope.end` sits on the boundary
/// with the text after the scope, so it counts only for an empty scope or
/// where `scope.end` is `buffer_end` mid-line, with no text after it to belong
/// to. A `buffer_end` at column 0 is the empty line after a trailing newline:
/// a scope ending there is the line before it, whose own `$` already counted
/// before the newline, so the empty line's match lies outside it.
pub(super) fn in_scope(scope: Option<Range>, range: Range, buffer_end: LineCol) -> bool {
    scope.is_none_or(|scope| {
        scope.start <= range.start
            && range.end <= scope.end
            && (range.start < scope.end
                || scope.start == scope.end
                || (scope.end == buffer_end && scope.end.col > 0))
    })
}

/// `scope` carried through `edits` (sorted, non-overlapping), so a replacement
/// inside the scope grows or shrinks it rather than leaving it on stale text.
/// Text inserted exactly at `scope.start` lands inside the scope: the start
/// stays before it rather than being pushed past it.
pub(super) fn map_scope(scope: Option<Range>, edits: &[TextEdit]) -> Option<Range> {
    scope.map(|scope| {
        let moves_start: Vec<TextEdit> = edits
            .iter()
            .filter(|edit| !(edit.range.is_empty() && edit.range.start == scope.start))
            .cloned()
            .collect();
        Range {
            start: editing::map_position(scope.start, &moves_start),
            end: editing::map_position(scope.end, edits),
        }
    })
}

/// One edit per match of `query` in `text`, replacing it with `replacement`:
/// regex captures (`$1`, `${name}`) expanded for a regex query, then, with
/// `preserve`, the case of the matched text carried onto the result.
pub(super) fn planned_replacements(
    text: &str,
    buffer: &karet_text::TextBuffer,
    query: &SearchQuery,
    replacement: &str,
    preserve: bool,
) -> Vec<TextEdit> {
    karet_search::plan_replacements(text, query, replacement)
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let new_text = if preserve {
                karet_search::preserve_case(text.get(r.start..r.end).unwrap_or_default(), &r.text)
            } else {
                r.text
            };
            TextEdit {
                range: Range {
                    start: buffer.byte_to_line_col(BytePos(r.start)),
                    end: buffer.byte_to_line_col(BytePos(r.end)),
                },
                new_text,
            }
        })
        .collect()
}
