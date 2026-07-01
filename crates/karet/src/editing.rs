//! Pure editing logic: build a [`Change`] (and the resulting caret position) from
//! the current caret/selection and buffer, for each non-modal editing operation.
//!
//! These functions own *what* an edit is, independent of the app, the session, or
//! any async machinery — so they are exhaustively unit-testable. The app turns the
//! returned [`Change`] into a `karet_session::Command::ApplyChange` and moves the
//! caret to the returned position optimistically.

use karet_core::Change;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;
use karet_text::TextBuffer;

/// One indent level. (EditorConfig-driven width is a later refinement.)
pub const INDENT: &str = "    ";

/// An edit plus the caret position it should leave behind.
pub struct Edit {
    /// The change to submit.
    pub change: Change,
    /// Where the caret lands after the change applies.
    pub caret: LineCol,
}

/// The caret position after inserting `text` starting at `start`.
fn advance(start: LineCol, text: &str) -> LineCol {
    let mut pos = start;
    for ch in text.chars() {
        if ch == '\n' {
            pos.line += 1;
            pos.col = 0;
        } else {
            pos.col += 1;
        }
    }
    pos
}

/// The number of `char`s on `line` (excluding the trailing break).
fn line_chars(buffer: &TextBuffer, line: u32) -> u32 {
    buffer
        .line(line as usize)
        .map_or(0, |l| u32::try_from(l.chars().count()).unwrap_or(u32::MAX))
}

/// The leading whitespace (spaces/tabs) of `line`, for auto-indent.
fn leading_ws(buffer: &TextBuffer, line: u32) -> String {
    buffer.line(line as usize).map_or_else(String::new, |l| {
        l.chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    })
}

fn one_edit(range: Range, new_text: String, base: u64) -> Change {
    Change::new(base, vec![TextEdit { range, new_text }])
}

/// Replace `range` with `text`.
fn replace(range: Range, text: String, base: u64) -> Edit {
    let caret = advance(range.start, &text);
    Edit {
        change: one_edit(range, text, base),
        caret,
    }
}

/// The selection if it is non-empty, else `None`.
fn non_empty(selection: Option<Range>) -> Option<Range> {
    selection.filter(|r| !r.is_empty())
}

/// Insert `text` at the caret, replacing any selection. Used for typing, paste,
/// and tab-as-spaces.
#[must_use]
pub fn insert(caret: LineCol, selection: Option<Range>, base: u64, text: &str) -> Edit {
    let range = non_empty(selection).unwrap_or(Range {
        start: caret,
        end: caret,
    });
    replace(range, text.to_string(), base)
}

/// Insert a newline, replacing any selection and copying the current line's leading
/// whitespace (auto-indent).
#[must_use]
pub fn newline(caret: LineCol, selection: Option<Range>, buffer: &TextBuffer, base: u64) -> Edit {
    let range = non_empty(selection).unwrap_or(Range {
        start: caret,
        end: caret,
    });
    let indent = leading_ws(buffer, range.start.line);
    let mut text = String::with_capacity(indent.len() + 1);
    text.push('\n');
    text.push_str(&indent);
    replace(range, text, base)
}

/// Delete backward: the selection if any, else the grapheme/char before the caret,
/// joining with the previous line at column 0. `None` at the very start of the buffer.
#[must_use]
pub fn backspace(
    caret: LineCol,
    selection: Option<Range>,
    buffer: &TextBuffer,
    base: u64,
) -> Option<Edit> {
    if let Some(range) = non_empty(selection) {
        return Some(Edit {
            change: one_edit(range, String::new(), base),
            caret: range.start,
        });
    }
    if caret.col > 0 {
        let start = LineCol::new(caret.line, caret.col - 1);
        return Some(delete_between(start, caret, base));
    }
    if caret.line > 0 {
        let prev = caret.line - 1;
        let start = LineCol::new(prev, line_chars(buffer, prev));
        return Some(delete_between(start, caret, base));
    }
    None
}

/// Delete forward: the selection if any, else the char after the caret, joining the
/// next line at end of line. `None` at the very end of the buffer.
#[must_use]
pub fn delete_forward(
    caret: LineCol,
    selection: Option<Range>,
    buffer: &TextBuffer,
    base: u64,
) -> Option<Edit> {
    if let Some(range) = non_empty(selection) {
        return Some(Edit {
            change: one_edit(range, String::new(), base),
            caret: range.start,
        });
    }
    if caret.col < line_chars(buffer, caret.line) {
        let end = LineCol::new(caret.line, caret.col + 1);
        return Some(delete_between(caret, end, base));
    }
    if (caret.line as usize) + 1 < buffer.line_count() {
        let end = LineCol::new(caret.line + 1, 0);
        return Some(delete_between(caret, end, base));
    }
    None
}

/// Delete `[start, end)`, leaving the caret at `start`.
fn delete_between(start: LineCol, end: LineCol, base: u64) -> Edit {
    Edit {
        change: one_edit(Range { start, end }, String::new(), base),
        caret: start,
    }
}

/// Indent: insert one level at the caret with no selection, or at the start of every
/// line the selection touches.
#[must_use]
pub fn indent(caret: LineCol, selection: Option<Range>, base: u64) -> Edit {
    match non_empty(selection) {
        Some(range) => {
            let last = last_selected_line(range);
            let edits = (range.start.line..=last)
                .map(|line| TextEdit {
                    range: Range {
                        start: LineCol::new(line, 0),
                        end: LineCol::new(line, 0),
                    },
                    new_text: INDENT.to_string(),
                })
                .collect();
            let width = u32::try_from(INDENT.chars().count()).unwrap_or(0);
            Edit {
                change: Change::new(base, edits),
                caret: LineCol::new(caret.line, caret.col + width),
            }
        },
        None => insert(caret, None, base, INDENT),
    }
}

/// Dedent the caret's line: remove up to one indent level of leading whitespace.
#[must_use]
pub fn dedent(caret: LineCol, buffer: &TextBuffer, base: u64) -> Option<Edit> {
    let line = buffer.line(caret.line as usize)?;
    let width = INDENT.chars().count();
    let remove = if line.starts_with('\t') {
        1
    } else {
        line.chars().take(width).take_while(|c| *c == ' ').count()
    };
    if remove == 0 {
        return None;
    }
    let remove_u32 = u32::try_from(remove).unwrap_or(0);
    let range = Range {
        start: LineCol::new(caret.line, 0),
        end: LineCol::new(caret.line, remove_u32),
    };
    Some(Edit {
        change: one_edit(range, String::new(), base),
        caret: LineCol::new(caret.line, caret.col.saturating_sub(remove_u32)),
    })
}

/// The last line a selection range actually covers (a selection ending at column 0
/// of a line does not include that line).
fn last_selected_line(range: Range) -> u32 {
    if range.end.col == 0 && range.end.line > range.start.line {
        range.end.line - 1
    } else {
        range.end.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn sel(s: (u32, u32), e: (u32, u32)) -> Option<Range> {
        Some(Range {
            start: at(s.0, s.1),
            end: at(e.0, e.1),
        })
    }

    #[test]
    fn insert_char_at_caret() {
        let e = insert(at(0, 3), None, 5, "x");
        assert_eq!(e.change.base_version, 5);
        assert_eq!(e.change.edits.len(), 1);
        assert_eq!(e.change.edits[0].new_text, "x");
        assert_eq!(e.change.edits[0].range.start, at(0, 3));
        assert_eq!(e.change.edits[0].range.end, at(0, 3));
        assert_eq!(e.caret, at(0, 4));
    }

    #[test]
    fn insert_replaces_selection() {
        let e = insert(at(0, 5), sel((0, 2), (0, 5)), 0, "Z");
        assert_eq!(e.change.edits[0].range.start, at(0, 2));
        assert_eq!(e.change.edits[0].range.end, at(0, 5));
        assert_eq!(e.caret, at(0, 3));
    }

    #[test]
    fn newline_copies_indent() {
        let buffer = TextBuffer::from_text("    code");
        let e = newline(at(0, 8), None, &buffer, 0);
        assert_eq!(e.change.edits[0].new_text, "\n    ");
        assert_eq!(e.caret, at(1, 4));
    }

    #[test]
    fn backspace_deletes_prev_char() {
        let buffer = TextBuffer::from_text("abc");
        let e = backspace(at(0, 2), None, &buffer, 0).expect("edit");
        assert_eq!(e.change.edits[0].range.start, at(0, 1));
        assert_eq!(e.change.edits[0].range.end, at(0, 2));
        assert_eq!(e.change.edits[0].new_text, "");
        assert_eq!(e.caret, at(0, 1));
    }

    #[test]
    fn backspace_joins_lines_at_column_zero() {
        let buffer = TextBuffer::from_text("ab\ncd");
        let e = backspace(at(1, 0), None, &buffer, 0).expect("edit");
        // Deletes the newline: from end of line 0 (col 2) to start of line 1.
        assert_eq!(e.change.edits[0].range.start, at(0, 2));
        assert_eq!(e.change.edits[0].range.end, at(1, 0));
        assert_eq!(e.caret, at(0, 2));
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let buffer = TextBuffer::from_text("abc");
        assert!(backspace(at(0, 0), None, &buffer, 0).is_none());
    }

    #[test]
    fn backspace_prefers_selection() {
        let buffer = TextBuffer::from_text("abcdef");
        let e = backspace(at(0, 4), sel((0, 1), (0, 4)), &buffer, 0).expect("edit");
        assert_eq!(e.change.edits[0].range.start, at(0, 1));
        assert_eq!(e.caret, at(0, 1));
    }

    #[test]
    fn delete_forward_joins_next_line() {
        let buffer = TextBuffer::from_text("ab\ncd");
        let e = delete_forward(at(0, 2), None, &buffer, 0).expect("edit");
        assert_eq!(e.change.edits[0].range.start, at(0, 2));
        assert_eq!(e.change.edits[0].range.end, at(1, 0));
        assert_eq!(e.caret, at(0, 2));
    }

    #[test]
    fn delete_forward_at_end_is_noop() {
        let buffer = TextBuffer::from_text("ab");
        assert!(delete_forward(at(0, 2), None, &buffer, 0).is_none());
    }

    #[test]
    fn indent_inserts_at_caret_without_selection() {
        let e = indent(at(0, 2), None, 0);
        assert_eq!(e.change.edits.len(), 1);
        assert_eq!(e.change.edits[0].new_text, INDENT);
        assert_eq!(e.caret, at(0, 6));
    }

    #[test]
    fn indent_indents_each_selected_line() {
        let e = indent(at(2, 1), sel((0, 0), (2, 3)), 0);
        assert_eq!(e.change.edits.len(), 3); // lines 0,1,2
        assert!(e.change.edits.iter().all(|ed| ed.range.start.col == 0));
        assert_eq!(e.caret, at(2, 5));
    }

    #[test]
    fn dedent_removes_leading_spaces() {
        let buffer = TextBuffer::from_text("      code"); // 6 spaces
        let e = dedent(at(0, 8), &buffer, 0).expect("edit");
        // Removes one level (4 spaces).
        assert_eq!(e.change.edits[0].range.start, at(0, 0));
        assert_eq!(e.change.edits[0].range.end, at(0, 4));
        assert_eq!(e.caret, at(0, 4));
    }

    #[test]
    fn dedent_unindented_line_is_noop() {
        let buffer = TextBuffer::from_text("code");
        assert!(dedent(at(0, 2), &buffer, 0).is_none());
    }

    #[test]
    fn edits_round_trip_through_the_buffer() {
        // Build an edit and apply it to a real buffer to confirm it does what it says.
        let mut buffer = TextBuffer::from_text("hello");
        let e = insert(at(0, 5), None, 0, " world");
        assert!(buffer.apply_simple(&e.change).is_ok());
        assert_eq!(buffer.line(0).as_deref(), Some("hello world"));
        assert_eq!(e.caret, at(0, 11));
    }
}
