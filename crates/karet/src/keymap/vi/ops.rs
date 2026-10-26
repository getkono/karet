//! The edits the operators make — delete, change, indent, case, join,
//! replace, paste — built against the document as it stands, with the
//! cursor position each leaves behind (already in post-edit coordinates).

use karet_core::LineCol;
use karet_core::Range;
use karet_core::TextEdit;

use super::object::Span;
use super::register::Register;
use super::text::Doc;
use super::text::end_of_insert;

/// A batch of non-overlapping edits against the current document, and the
/// cursor it leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Planned {
    /// The edits, in document order.
    pub(super) edits: Vec<TextEdit>,
    /// Where the cursor lands once they apply.
    pub(super) cursor: LineCol,
}

fn edit(range: Range, new_text: impl Into<String>) -> TextEdit {
    TextEdit {
        range,
        new_text: new_text.into(),
    }
}

fn col(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The text `span` covers, as a register.
pub(super) fn yank(doc: &Doc, span: Span) -> Register {
    match span {
        Span::Chars(range) => Register {
            text: doc.text(range),
            linewise: false,
        },
        Span::Lines(first, last) => Register {
            text: doc.lines_text(first, last),
            linewise: true,
        },
    }
}

/// The range that removes whole lines `first..=last`, break included: the
/// break after them, or — for the document's last lines — the one before.
fn lines_range(doc: &Doc, first: u32, last: u32) -> Range {
    let last = last.min(doc.last_line());
    if last < doc.last_line() {
        Range {
            start: LineCol::new(first, 0),
            end: LineCol::new(last + 1, 0),
        }
    } else if first > 0 {
        Range {
            start: doc.line_end(first - 1),
            end: doc.line_end(last),
        }
    } else {
        Range {
            start: LineCol::new(0, 0),
            end: doc.line_end(last),
        }
    }
}

/// Delete `span`. The cursor stays where the text was, on a character.
pub(super) fn delete(doc: &Doc, span: Span) -> Planned {
    match span {
        Span::Chars(range) => {
            let (start, end) = (doc.clamp(range.start), doc.clamp(range.end));
            let joined = start.col + doc.len(end.line).saturating_sub(end.col);
            Planned {
                edits: vec![edit(Range { start, end }, "")],
                cursor: LineCol::new(start.line, start.col.min(joined.saturating_sub(1))),
            }
        },
        Span::Lines(first, last) => {
            let last = last.min(doc.last_line());
            let range = lines_range(doc, first, last);
            let cursor = if last < doc.last_line() {
                LineCol::new(first, doc.first_non_blank(last + 1).col)
            } else if first > 0 {
                doc.first_non_blank(first - 1)
            } else {
                LineCol::new(0, 0)
            };
            Planned {
                edits: vec![edit(range, "")],
                cursor: clamp_on(doc, cursor, last, first),
            }
        },
    }
}

/// Keep a first-non-blank cursor on a character of a blank target line.
fn clamp_on(doc: &Doc, cursor: LineCol, last: u32, first: u32) -> LineCol {
    let source = if last < doc.last_line() && cursor.line == first {
        last + 1
    } else {
        cursor.line
    };
    LineCol::new(
        cursor.line,
        cursor.col.min(doc.len(source).saturating_sub(1)),
    )
}

/// Delete `span` for a change, leaving the cursor where typing begins. Whole
/// lines keep the first line's indentation, as `cc` does.
pub(super) fn change(doc: &Doc, span: Span) -> Planned {
    match span {
        Span::Chars(range) => Planned {
            edits: vec![edit(range, "")],
            cursor: range.start,
        },
        Span::Lines(first, last) => {
            let last = last.min(doc.last_line());
            let indent = doc.indent_of(first);
            let cursor = LineCol::new(first, col(indent.chars().count()));
            Planned {
                edits: vec![edit(
                    Range {
                        start: LineCol::new(first, 0),
                        end: doc.line_end(last),
                    },
                    indent,
                )],
                cursor,
            }
        },
    }
}

/// Indent (or, with `outdent`, dedent) lines `first..=last` by `count`
/// levels of `unit`. Blank lines are left alone when indenting.
pub(super) fn shift(
    doc: &Doc,
    first: u32,
    last: u32,
    unit: &str,
    count: u32,
    outdent: bool,
) -> Planned {
    let width = unit.chars().filter(|c| *c == ' ').count().max(1);
    let mut edits = Vec::new();
    let mut cursor_col = 0;
    for line in first..=last.min(doc.last_line()) {
        let indent = doc.indent_of(line);
        let new_indent = if outdent {
            let mut rest: &str = &indent;
            for _ in 0..count.max(1) {
                rest = if let Some(tail) = rest.strip_prefix('\t') {
                    tail
                } else {
                    let spaces = rest.chars().take(width).take_while(|c| *c == ' ').count();
                    rest.get(spaces..).unwrap_or_default()
                };
            }
            rest.to_string()
        } else if doc.is_blank_line(line) {
            indent.clone()
        } else {
            unit.repeat(count.max(1) as usize) + &indent
        };
        if line == first {
            cursor_col = new_indent.chars().count();
        }
        if new_indent != indent {
            edits.push(edit(
                Range {
                    start: LineCol::new(line, 0),
                    end: LineCol::new(line, col(indent.chars().count())),
                },
                new_indent,
            ));
        }
    }
    // The first line's length once its indentation is replaced.
    let first_len =
        (doc.len(first) as usize + cursor_col).saturating_sub(doc.indent_of(first).chars().count());
    Planned {
        edits,
        cursor: LineCol::new(first, col(cursor_col.min(first_len.saturating_sub(1)))),
    }
}

/// How a case operator rewrites each character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Case {
    /// `gu`, helix `` ` ``.
    Lower,
    /// `gU`, helix `` Alt+` ``.
    Upper,
    /// `g~`, `~`.
    Toggle,
}

/// Rewrite the case of the text in `range`.
pub(super) fn case(doc: &Doc, range: Range, case: Case) -> Option<TextEdit> {
    let text = doc.text(range);
    let mapped: String = text
        .chars()
        .flat_map(|c| -> Vec<char> {
            match case {
                Case::Lower => c.to_lowercase().collect(),
                Case::Upper => c.to_uppercase().collect(),
                Case::Toggle if c.is_lowercase() => c.to_uppercase().collect(),
                Case::Toggle => c.to_lowercase().collect(),
            }
        })
        .collect();
    (mapped != text).then(|| edit(range, mapped))
}

/// The character range whole lines cover (from the first's start to the
/// last's end, without the final break).
pub(super) fn lines_chars(doc: &Doc, first: u32, last: u32) -> Range {
    Range {
        start: LineCol::new(first, 0),
        end: doc.line_end(last.min(doc.last_line())),
    }
}

/// Join lines `first..=last` into one (`J`; `gJ` without `spaces`). Each
/// break and the next line's indentation become one space, except before a
/// `)`, after a blank, or where the next line is empty. The cursor lands at
/// the last join.
pub(super) fn join(doc: &Doc, first: u32, last: u32, spaces: bool) -> Option<Planned> {
    let last = last.min(doc.last_line());
    if last <= first {
        return None;
    }
    let mut edits = Vec::new();
    let mut joined_len = doc.len(first);
    let mut cursor_col = 0;
    for line in first..last {
        let next = line + 1;
        let next_chars = doc.chars(next);
        let lead = if spaces {
            next_chars.iter().take_while(|c| c.is_whitespace()).count()
        } else {
            0
        };
        let rest_empty = lead >= next_chars.len();
        let current = doc.chars(line);
        let ends_blank = current.last().is_none_or(|c| c.is_whitespace());
        let sep = if !spaces || rest_empty || ends_blank || next_chars.get(lead) == Some(&')') {
            ""
        } else {
            " "
        };
        edits.push(edit(
            Range {
                start: doc.line_end(line),
                end: LineCol::new(next, col(lead)),
            },
            sep,
        ));
        cursor_col = joined_len;
        joined_len += col(sep.len()) + col(next_chars.len() - lead);
    }
    Some(Planned {
        edits,
        cursor: LineCol::new(first, cursor_col.min(joined_len.saturating_sub(1))),
    })
}

/// Replace every character in `range` with `with`, keeping line breaks.
pub(super) fn replace_chars(doc: &Doc, range: Range, with: char) -> Option<TextEdit> {
    let text = doc.text(range);
    if text.is_empty() {
        return None;
    }
    let mapped: String = text
        .chars()
        .map(|c| if c == '\n' { c } else { with })
        .collect();
    Some(edit(range, mapped))
}

/// Where a paste goes relative to the cursor or selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Put {
    /// After the cursor / below the line (`p`).
    After,
    /// Before the cursor / above the line (`P`).
    Before,
}

/// Paste `reg` `count` times at `cursor`. Whole lines go below (or above)
/// the cursor's line with the cursor on the first's first non-blank; text
/// goes after (or before) the cursor with the cursor on its last character
/// (on its first, for text spanning lines).
pub(super) fn paste(doc: &Doc, cursor: LineCol, reg: &Register, put: Put, count: u32) -> Planned {
    let times = count.max(1) as usize;
    if reg.linewise {
        let mut body = reg.text.clone();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let body = body.repeat(times);
        let (at, text, first_line) = match put {
            Put::Before => (LineCol::new(cursor.line, 0), body, cursor.line),
            Put::After if cursor.line < doc.last_line() => {
                (LineCol::new(cursor.line + 1, 0), body, cursor.line + 1)
            },
            Put::After => {
                let trimmed = body.strip_suffix('\n').unwrap_or(&body).to_string();
                (
                    doc.line_end(cursor.line),
                    format!("\n{trimmed}"),
                    cursor.line + 1,
                )
            },
        };
        let first_text: String = text
            .trim_start_matches('\n')
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let lead = first_text.chars().take_while(|c| c.is_whitespace()).count();
        let cursor = LineCol::new(
            first_line,
            col(lead.min(first_text.chars().count().saturating_sub(1))),
        );
        return Planned {
            edits: vec![edit(Range { start: at, end: at }, text)],
            cursor,
        };
    }
    let text = reg.text.repeat(times);
    let at = match put {
        Put::Before => cursor,
        Put::After => doc.after(cursor).min(doc.line_end(cursor.line)),
    };
    let at = if doc.len(cursor.line) == 0 {
        LineCol::new(cursor.line, 0)
    } else {
        at
    };
    let end = end_of_insert(at, &text);
    let cursor = if text.contains('\n') {
        at
    } else {
        LineCol::new(end.line, end.col.saturating_sub(1))
    };
    Planned {
        edits: vec![edit(Range { start: at, end: at }, text)],
        cursor,
    }
}

/// Increment (or decrement) the number at or after `p` on its line by
/// `delta` (`Ctrl+A`/`Ctrl+X`), leaving the cursor on its last digit.
pub(super) fn increment(doc: &Doc, p: LineCol, delta: i64) -> Option<Planned> {
    let chars = doc.chars(p.line);
    let at = p.col as usize;
    // The digit run under the cursor, or the first after it.
    let mut start = (at..chars.len()).find(|&i| chars.get(i).is_some_and(char::is_ascii_digit))?;
    if chars.get(at).is_some_and(char::is_ascii_digit) {
        start = at;
        while start > 0 && chars.get(start - 1).is_some_and(char::is_ascii_digit) {
            start -= 1;
        }
    }
    let mut end = start;
    while chars.get(end).is_some_and(char::is_ascii_digit) {
        end += 1;
    }
    let negative = start > 0 && chars.get(start - 1) == Some(&'-');
    let from = if negative { start - 1 } else { start };
    let digits: String = chars.get(start..end)?.iter().collect();
    let value: i64 = digits.parse().ok()?;
    let value = if negative { -value } else { value };
    let next = value.checked_add(delta)?.to_string();
    let range = Range {
        start: LineCol::new(p.line, col(from)),
        end: LineCol::new(p.line, col(end)),
    };
    let cursor = LineCol::new(p.line, col(from + next.chars().count() - 1));
    Some(Planned {
        edits: vec![edit(range, next)],
        cursor,
    })
}
