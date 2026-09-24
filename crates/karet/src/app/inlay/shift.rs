//! Carrying a hint set across an edit until its replacement arrives.
//!
//! A held set is positioned against the text it was answered for. Left where
//! it was, every edit above a hint leaves it annotating whatever now sits at
//! its old column -- a `: u32` stranded in the middle of the next identifier
//! while the server re-infers. So the set moves with the text: each hint is
//! shifted by the line and column delta of the edits before it, and a hint
//! whose anchor an edit consumed is dropped rather than guessed at.
//!
//! Pure functions over neutral edits, so the arithmetic is tested without an
//! app.

use karet_core::InlayHint;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_editor::editing::caret_after_insert;

/// Where `pos` lands once `edits` are applied, or `None` when an edit replaced
/// the text around it.
///
/// `edits` are one atomic, non-overlapping batch against the same base text,
/// in any order. They are applied last-first: an edit only moves text after
/// it, so every earlier edit's range is still valid in the base coordinates
/// when its turn comes, and one rule per edit is exact.
///
/// The rule, for an edit replacing `start..end`:
///
/// - `pos` at or after `end` moves with the text after the edit -- including
///   `pos == end`, which is typing at the hint's own anchor: the caret sits
///   before the hint, so what is typed lands before it and pushes it along.
/// - `pos` strictly inside `start..end` is gone with the text it annotated.
/// - `pos` at or before `start` is untouched.
#[must_use]
pub(crate) fn shift_position(pos: LineCol, edits: &[TextEdit]) -> Option<LineCol> {
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    let mut pos = pos;
    for edit in ordered {
        let Range { start, end } = edit.range;
        if pos >= end {
            let new_end = caret_after_insert(start, &edit.new_text);
            pos = if pos.line == end.line {
                LineCol::new(new_end.line, new_end.col + (pos.col - end.col))
            } else {
                // A line count cannot go negative here: `end.line` is at most
                // `pos.line`, and `new_end.line` at least `start.line`.
                LineCol::new(pos.line - end.line + new_end.line, pos.col)
            };
        } else if pos > start {
            return None;
        }
    }
    Some(pos)
}

/// Carry every hint in `hints` across `edits`, dropping those an edit consumed.
pub(crate) fn shift_hints(hints: &mut Vec<InlayHint>, edits: &[TextEdit]) {
    hints.retain_mut(|hint| match shift_position(hint.position, edits) {
        Some(position) => {
            hint.position = position;
            true
        },
        None => false,
    });
}

/// The one edit that turns `old` into `new`: everything between their common
/// prefix and common suffix.
///
/// For a change whose edits were not seen -- an undo, a formatter, a reload
/// -- this is the smallest single replacement that explains it. Hints outside
/// it shift exactly; hints inside it are dropped, which is right for the text
/// that really changed and merely conservative for anything between two
/// separate changes. `None` when the texts are equal.
#[must_use]
pub(crate) fn diff_edit(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    // Equal bytes up to `prefix`, so a boundary in one is one in the other.
    while !old.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) {
        suffix -= 1;
    }
    Some(TextEdit {
        range: Range {
            start: line_col_at(old, prefix),
            end: line_col_at(old, old.len() - suffix),
        },
        new_text: new.get(prefix..new.len() - suffix)?.to_owned(),
    })
}

/// The line and character column of byte offset `at` in `text`.
fn line_col_at(text: &str, at: usize) -> LineCol {
    let before = text.get(..at).unwrap_or(text);
    let line = before.matches('\n').count();
    let col = before
        .rsplit('\n')
        .next()
        .map_or(0, |tail| tail.chars().count());
    LineCol::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(col).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use karet_core::InlayHintKind;

    use super::*;

    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: LineCol::new(start.0, start.1),
                end: LineCol::new(end.0, end.1),
            },
            new_text: text.to_owned(),
        }
    }

    fn hint_at(line: u32, col: u32) -> InlayHint {
        InlayHint {
            position: LineCol::new(line, col),
            label: ": u32".to_owned(),
            kind: InlayHintKind::Type,
            padding_left: false,
            padding_right: false,
        }
    }

    #[test]
    fn inserted_lines_above_move_a_hint_down() {
        let edits = [edit((0, 0), (0, 0), "use a;\nuse b;\n")];
        assert_eq!(
            shift_position(LineCol::new(3, 5), &edits),
            Some(LineCol::new(5, 5))
        );
    }

    #[test]
    fn typing_before_a_hint_on_its_line_shifts_its_column() {
        let edits = [edit((2, 4), (2, 4), "mut ")];
        assert_eq!(
            shift_position(LineCol::new(2, 5), &edits),
            Some(LineCol::new(2, 9))
        );
        // Typing at the anchor itself lands before the hint.
        assert_eq!(
            shift_position(LineCol::new(2, 4), &edits),
            Some(LineCol::new(2, 8))
        );
    }

    #[test]
    fn an_edit_after_a_hint_leaves_it_alone() {
        let edits = [edit((2, 9), (4, 0), "")];
        assert_eq!(
            shift_position(LineCol::new(2, 5), &edits),
            Some(LineCol::new(2, 5))
        );
    }

    #[test]
    fn deleting_the_text_around_a_hint_drops_it() {
        let mut hints = vec![hint_at(1, 5), hint_at(3, 2)];
        // Delete line 1 whole: the first hint's anchor goes with it.
        shift_hints(&mut hints, &[edit((1, 0), (2, 0), "")]);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].position, LineCol::new(2, 2));
    }

    #[test]
    fn joining_a_line_carries_the_column_onto_the_line_above() {
        // Backspace at the start of line 1, joining it onto "let x" (5 chars).
        let edits = [edit((0, 5), (1, 0), "")];
        assert_eq!(
            shift_position(LineCol::new(1, 7), &edits),
            Some(LineCol::new(0, 12))
        );
    }

    #[test]
    fn a_multi_caret_batch_shifts_by_every_edit_before_the_hint() {
        // Two carets on the hint's line, both before it, in either order.
        let edits = [edit((0, 8), (0, 8), "b"), edit((0, 2), (0, 2), "a")];
        assert_eq!(
            shift_position(LineCol::new(0, 10), &edits),
            Some(LineCol::new(0, 12))
        );
    }

    #[test]
    fn diff_edit_finds_the_one_changed_span() {
        let old = "fn a() -> u32 {\n    0\n}\n";
        let new = "fn a() -> u64 {\n    0\n}\n";
        assert_eq!(diff_edit(old, new), Some(edit((0, 11), (0, 13), "64")));
        assert_eq!(diff_edit(old, old), None);
    }

    #[test]
    fn diff_edit_counts_columns_in_characters_not_bytes() {
        // An undo restoring an inserted line under an emoji: the emoji is four
        // bytes but one column, and the edit must not split it.
        let old = "😀a\nb\n";
        let new = "😀a\nx\nb\n";
        let Some(found) = diff_edit(old, new) else {
            unreachable!("the texts differ");
        };
        assert_eq!(
            shift_position(LineCol::new(1, 1), std::slice::from_ref(&found)),
            Some(LineCol::new(2, 1))
        );
        assert_eq!(
            shift_position(LineCol::new(0, 2), std::slice::from_ref(&found)),
            Some(LineCol::new(0, 2))
        );
    }

    #[test]
    fn diff_edit_never_splits_a_character_both_sides_share_bytes_of() {
        // 'é' (C3 A9) and 'ê' (C3 AA) share their first byte, so a byte-wise
        // prefix stops inside the character.
        assert_eq!(diff_edit("aé", "aê"), Some(edit((0, 1), (0, 2), "ê")));
        // 'é' (C3 A9) and 'ǩ' (C7 A9) share their last byte, so a byte-wise
        // suffix does.
        assert_eq!(diff_edit("aé", "aǩ"), Some(edit((0, 1), (0, 2), "ǩ")));
    }
}
