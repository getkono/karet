//! Indent (Tab) and dedent (Shift+Tab) across every selection at once.
//!
//! Like the comment toggles, these build one batch over the whole cursor set and
//! carry every selection through it, so a selection survives being indented or
//! dedented, and every caret takes part.

use karet_core::CursorState;
use karet_core::LineCol;
use karet_text::TextBuffer;

use super::MultiEdit;
use super::comment::LineEdit;
use super::comment::finish;
use super::last_selected_line;

/// Indent across every selection (Tab).
///
/// A non-empty selection indents every line it touches by one `indentation`
/// level, inserted at column 0; lines with no characters are left alone so no
/// trailing whitespace is created, and the selection keeps covering the same
/// text (an end at column 0 stays at column 0, so a whole-line selection still
/// starts at the line start). A bare caret instead inserts indentation at the
/// caret: with space indentation, just enough spaces to reach the next multiple
/// of the indent width (tabs before the caret count to the next multiple of
/// `tab_width`), and otherwise `indentation` itself. A caret on a line a
/// selection already indents moves with that line rather than inserting again.
///
/// `None` when `indentation` is empty or there is nothing to indent.
#[must_use]
pub fn indent(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    indentation: &str,
    tab_width: u32,
) -> Option<MultiEdit> {
    if indentation.is_empty() {
        return None;
    }
    let mut lines: Vec<u32> = cursors
        .selections
        .iter()
        .map(|s| s.range())
        .filter(|r| !r.is_empty())
        .flat_map(|r| r.start.line..=last_selected_line(r))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let mut edits: Vec<LineEdit> = lines
        .iter()
        .filter(|&&line| buffer.line(line as usize).is_some_and(|t| !t.is_empty()))
        .map(|&line| LineEdit::insert(LineCol::new(line, 0), indentation.to_string(), 0))
        .collect();
    for selection in &cursors.selections {
        let caret = selection.head;
        if !selection.range().is_empty() || lines.binary_search(&caret.line).is_ok() {
            continue;
        }
        let text = caret_indent(buffer, caret, indentation, tab_width);
        let width = char_width(&text);
        edits.push(LineEdit::insert(caret, text, width));
    }
    finish(edits, cursors, base)
}

/// Dedent across every selection (Shift+Tab): every line any selection or caret
/// touches loses up to one indent level of leading whitespace.
///
/// A line starting with a hard tab loses that tab (or the whole `indentation`
/// string when it is a tab-led mix the line starts with). A line indented with
/// spaces moves back to the previous multiple of the indent width, so a line
/// off the grid snaps onto it. Selections and carets move with their text; one
/// inside the removed whitespace lands at column 0.
///
/// `None` when no touched line has leading whitespace to remove.
#[must_use]
pub fn dedent(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    indentation: &str,
    tab_width: u32,
) -> Option<MultiEdit> {
    let unit = visual_width(indentation, tab_width);
    if unit == 0 {
        return None;
    }
    let mut lines: Vec<u32> = cursors
        .selections
        .iter()
        .flat_map(|s| {
            let range = s.range();
            range.start.line..=last_selected_line(range)
        })
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let edits = lines
        .into_iter()
        .filter_map(|line| {
            let text = buffer.line(line as usize)?;
            let remove = dedent_width(&text, indentation, unit);
            (remove > 0).then(|| LineEdit::delete(LineCol::new(line, 0), remove))
        })
        .collect();
    finish(edits, cursors, base)
}

/// The text a bare caret's Tab inserts: spaces to the next indent stop when
/// `indentation` is all spaces, else `indentation` itself.
fn caret_indent(buffer: &TextBuffer, caret: LineCol, indentation: &str, tab_width: u32) -> String {
    if !indentation.chars().all(|c| c == ' ') {
        return indentation.to_string();
    }
    let unit = char_width(indentation);
    let before: String = buffer
        .line(caret.line as usize)
        .map(|t| t.chars().take(caret.col as usize).collect())
        .unwrap_or_default();
    let column = visual_width(&before, tab_width);
    " ".repeat((unit - column % unit) as usize)
}

/// How many leading chars of `text` one dedent removes.
fn dedent_width(text: &str, indentation: &str, unit: u32) -> u32 {
    if indentation.starts_with('\t') && text.starts_with(indentation) {
        return char_width(indentation);
    }
    if text.starts_with('\t') {
        return 1;
    }
    let spaces = u32::try_from(text.chars().take_while(|c| *c == ' ').count()).unwrap_or(0);
    if spaces == 0 {
        0
    } else {
        (spaces - 1) % unit + 1
    }
}

/// The display width of `text` with hard tabs advancing to the next multiple of
/// `tab_width` (at least 1).
fn visual_width(text: &str, tab_width: u32) -> u32 {
    let tab_width = tab_width.max(1);
    text.chars().fold(0, |column, c| {
        if c == '\t' {
            column + tab_width - column % tab_width
        } else {
            column + 1
        }
    })
}

fn char_width(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use karet_core::Selection;

    use super::*;

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn cursors(selections: &[(LineCol, LineCol)]) -> CursorState {
        CursorState {
            selections: selections
                .iter()
                .map(|&(anchor, head)| Selection { anchor, head })
                .collect(),
            primary: 0,
        }
    }

    /// Build an edit over `text` with `build`, apply it, and return the edited
    /// text and the cursor set it leaves (unchanged text and none for `None`).
    fn run(
        text: &str,
        build: impl FnOnce(&TextBuffer) -> Option<MultiEdit>,
    ) -> (String, Vec<(LineCol, LineCol)>) {
        let mut buffer = TextBuffer::from_text(text);
        let Some(edit) = build(&buffer) else {
            return (buffer.text(), Vec::new());
        };
        assert!(buffer.apply_simple(&edit.change).is_ok());
        let selections = edit
            .cursors
            .selections
            .iter()
            .map(|s| (s.anchor, s.head))
            .collect();
        (buffer.text(), selections)
    }

    #[test]
    fn bare_caret_pads_spaces_to_the_next_stop() {
        let set = cursors(&[(at(0, 2), at(0, 2))]);
        let (text, sel) = run("ab\nx", |b| indent(b, &set, 0, "    ", 4));
        assert_eq!(text, "ab  \nx");
        assert_eq!(sel, vec![(at(0, 4), at(0, 4))]);
    }

    #[test]
    fn bare_caret_on_a_stop_inserts_a_whole_level() {
        let set = cursors(&[(at(0, 4), at(0, 4))]);
        let (text, sel) = run("    x", |b| indent(b, &set, 0, "    ", 4));
        assert_eq!(text, "        x");
        assert_eq!(sel, vec![(at(0, 8), at(0, 8))]);
    }

    #[test]
    fn tab_stop_counts_hard_tabs_by_display_width() {
        // A tab displayed 8 wide puts the caret at column 9; the next 4-stop is 12.
        let set = cursors(&[(at(0, 2), at(0, 2))]);
        let (text, _) = run("\tab", |b| indent(b, &set, 0, "    ", 8));
        assert_eq!(text, "\ta   b");
    }

    #[test]
    fn hard_tab_indentation_inserts_itself_at_the_caret() {
        let set = cursors(&[(at(0, 1), at(0, 1))]);
        let (text, sel) = run("ab", |b| indent(b, &set, 0, "\t", 4));
        assert_eq!(text, "a\tb");
        assert_eq!(sel, vec![(at(0, 2), at(0, 2))]);
    }

    #[test]
    fn every_caret_indents() {
        let set = cursors(&[(at(0, 0), at(0, 0)), (at(1, 1), at(1, 1))]);
        let (text, sel) = run("a\nbc", |b| indent(b, &set, 0, "  ", 4));
        assert_eq!(text, "  a\nb c");
        assert_eq!(sel, vec![(at(0, 2), at(0, 2)), (at(1, 2), at(1, 2))]);
    }

    #[test]
    fn selection_indents_its_lines_and_is_preserved() {
        let set = cursors(&[(at(0, 1), at(2, 3))]);
        let (text, sel) = run("one\ntwo\nthree", |b| indent(b, &set, 0, "    ", 4));
        assert_eq!(text, "    one\n    two\n    three");
        assert_eq!(sel, vec![(at(0, 5), at(2, 7))]);
    }

    #[test]
    fn single_line_selection_indents_the_line_rather_than_replacing_it() {
        let set = cursors(&[(at(0, 4), at(0, 5))]);
        let (text, sel) = run("let x", |b| indent(b, &set, 0, "  ", 4));
        assert_eq!(text, "  let x");
        assert_eq!(sel, vec![(at(0, 6), at(0, 7))]);
    }

    #[test]
    fn whole_line_selection_keeps_its_column_zero_ends() {
        // A selection ending at column 0 does not touch that line, and one
        // starting at column 0 keeps the inserted indentation inside it.
        let set = cursors(&[(at(2, 0), at(0, 0))]);
        let (text, sel) = run("a\nb\nc", |b| indent(b, &set, 0, "\t", 4));
        assert_eq!(text, "\ta\n\tb\nc");
        assert_eq!(sel, vec![(at(2, 0), at(0, 0))]);
    }

    #[test]
    fn empty_lines_inside_a_selection_stay_empty() {
        let set = cursors(&[(at(0, 0), at(2, 1))]);
        let (text, _) = run("a\n\nb", |b| indent(b, &set, 0, "  ", 4));
        assert_eq!(text, "  a\n\n  b");
    }

    #[test]
    fn caret_on_a_line_a_selection_indents_moves_with_it() {
        let set = cursors(&[(at(0, 0), at(1, 1)), (at(1, 2), at(1, 2))]);
        let (text, sel) = run("ab\ncd", |b| indent(b, &set, 0, "  ", 4));
        assert_eq!(text, "  ab\n  cd");
        assert_eq!(sel, vec![(at(0, 0), at(1, 3)), (at(1, 4), at(1, 4))]);
    }

    #[test]
    fn empty_indentation_is_a_no_op() {
        let buffer = TextBuffer::from_text("a");
        assert!(indent(&buffer, &cursors(&[(at(0, 0), at(0, 0))]), 0, "", 4).is_none());
        assert!(dedent(&buffer, &cursors(&[(at(0, 0), at(0, 0))]), 0, "", 4).is_none());
    }

    #[test]
    fn dedent_removes_one_level_and_moves_the_caret() {
        let set = cursors(&[(at(0, 10), at(0, 10))]);
        let (text, sel) = run("        code", |b| dedent(b, &set, 0, "    ", 4));
        assert_eq!(text, "    code");
        assert_eq!(sel, vec![(at(0, 6), at(0, 6))]);
    }

    #[test]
    fn dedent_snaps_off_grid_spaces_to_the_previous_stop() {
        let set = cursors(&[(at(0, 0), at(0, 0))]);
        let (text, _) = run("      code", |b| dedent(b, &set, 0, "    ", 4));
        assert_eq!(text, "    code");
    }

    #[test]
    fn dedent_selection_keeps_it_and_collapses_ends_inside_removed_indent() {
        let set = cursors(&[(at(0, 2), at(2, 3))]);
        let (text, sel) = run("    one\n\ttwo\nthree", |b| dedent(b, &set, 0, "    ", 4));
        assert_eq!(text, "one\ntwo\nthree");
        assert_eq!(sel, vec![(at(0, 0), at(2, 3))]);
    }

    #[test]
    fn dedent_every_caret_line_once() {
        let set = cursors(&[
            (at(0, 5), at(0, 5)),
            (at(0, 7), at(0, 7)),
            (at(1, 3), at(1, 3)),
        ]);
        let (text, sel) = run("    a b\n  c", |b| dedent(b, &set, 0, "    ", 4));
        assert_eq!(text, "a b\nc");
        assert_eq!(
            sel,
            vec![
                (at(0, 1), at(0, 1)),
                (at(0, 3), at(0, 3)),
                (at(1, 1), at(1, 1))
            ]
        );
    }

    #[test]
    fn dedent_of_unindented_lines_is_a_no_op() {
        let buffer = TextBuffer::from_text("code\nmore");
        let set = cursors(&[(at(0, 0), at(1, 2))]);
        assert!(dedent(&buffer, &set, 0, "    ", 4).is_none());
    }

    #[test]
    fn mixed_tab_and_space_indentation_round_trips() {
        let set = cursors(&[(at(0, 0), at(0, 4))]);
        let (text, sel) = run("code", |b| indent(b, &set, 0, "\t  ", 4));
        assert_eq!(text, "\t  code");
        let (anchor, head) = sel[0];
        let set = cursors(&[(anchor, head)]);
        let (text, sel) = run("\t  code", |b| dedent(b, &set, 0, "\t  ", 4));
        assert_eq!(text, "code");
        assert_eq!(sel, vec![(at(0, 0), at(0, 4))]);
    }
}
