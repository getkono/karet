//! Comment toggling across every selection at once.
//!
//! Unlike the per-caret edits in the parent module, a comment toggle decides one
//! direction for *all* the lines the selections touch (comment them all, or
//! uncomment them all), so it builds a single batch over the whole cursor set and
//! returns the cursor set it leaves behind.

use karet_core::Change;
use karet_core::CursorState;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Selection;
use karet_core::TextEdit;
use karet_text::TextBuffer;

use super::last_selected_line;

/// An edit that spans every selection, plus the cursor set it leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiEdit {
    /// The change to submit (one atomic, non-overlapping batch).
    pub change: Change,
    /// The selections after the change applies, in the edited document's coordinates.
    pub cursors: CursorState,
}

/// How [`toggle_line_comment`] marks a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineComment<'a> {
    /// A line-comment prefix such as `//` or `#`.
    Prefix(&'a str),
    /// Wrap each line in a block comment, for languages without a line comment
    /// (`<!-- line -->` in HTML, `/* line */` in CSS).
    Wrap {
        /// The opening delimiter (e.g. `<!--`).
        open: &'a str,
        /// The closing delimiter (e.g. `-->`).
        close: &'a str,
    },
}

/// One edit confined to a single line, in the original document's char columns:
/// remove `remove` chars at `col`, then insert `text` there.
struct LineEdit {
    line: u32,
    col: u32,
    remove: u32,
    text: String,
    /// For a pure insertion, how far a position exactly at `col` moves: the full
    /// insertion width to push it past the text, `0` to keep it before.
    at_col: u32,
}

impl LineEdit {
    fn insert(at: LineCol, text: String, at_col: u32) -> Self {
        Self {
            line: at.line,
            col: at.col,
            remove: 0,
            text,
            at_col,
        }
    }

    fn delete(at: LineCol, remove: u32) -> Self {
        Self {
            line: at.line,
            col: at.col,
            remove,
            text: String::new(),
            at_col: 0,
        }
    }
}

/// Toggle a line comment on every line the selections touch.
///
/// When every non-blank touched line is already commented, the comment marker (and
/// one space after it) is removed from each; otherwise each non-blank line gets
/// `token` inserted at the *minimum* indent of those lines, so the markers align.
/// Blank lines are left alone unless every touched line is blank. `None` when there
/// is nothing to do (an empty token).
#[must_use]
pub fn toggle_line_comment(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    token: LineComment<'_>,
) -> Option<MultiEdit> {
    let (prefix, suffix) = match token {
        LineComment::Prefix(prefix) => (prefix, ""),
        LineComment::Wrap { open, close } => (open, close),
    };
    if prefix.is_empty() {
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
    let texts: Vec<(u32, Vec<char>)> = lines
        .into_iter()
        .filter_map(|l| buffer.line(l as usize).map(|t| (l, t.chars().collect())))
        .collect();
    let non_blank: Vec<&(u32, Vec<char>)> = texts.iter().filter(|(_, t)| !is_blank(t)).collect();
    let targets: Vec<&(u32, Vec<char>)> = if non_blank.is_empty() {
        texts.iter().collect()
    } else {
        non_blank
    };
    let prefix: Vec<char> = prefix.chars().collect();
    let suffix: Vec<char> = suffix.chars().collect();
    let uncomment = targets
        .iter()
        .all(|(_, t)| !is_blank(t) && is_line_commented(t, &prefix, &suffix));

    let mut edits = Vec::new();
    if uncomment {
        for (line, text) in &targets {
            edits.extend(uncomment_line(*line, text, &prefix, &suffix));
        }
    } else {
        let indent = targets.iter().map(|(_, t)| leading(t)).min().unwrap_or(0);
        for (line, text) in &targets {
            edits.extend(comment_line(*line, text, indent, &prefix, &suffix));
        }
    }
    finish(edits, cursors, base)
}

/// Toggle a block comment around each selection.
///
/// A non-empty selection is wrapped as `open selection close`, or unwrapped when its
/// trimmed text already starts with `open` and ends with `close`. A bare caret acts
/// on its line's content the same way; on a blank line it inserts `open  close` and
/// lands between the delimiters. A selection touching an earlier one's target is
/// skipped so the batch never overlaps. `None` when there is nothing to do.
#[must_use]
pub fn toggle_block_comment(
    buffer: &TextBuffer,
    cursors: &CursorState,
    base: u64,
    open: &str,
    close: &str,
) -> Option<MultiEdit> {
    if open.is_empty() || close.is_empty() {
        return None;
    }
    let open: Vec<char> = open.chars().collect();
    let close: Vec<char> = close.chars().collect();
    let mut edits = Vec::new();
    let mut previous: Option<Range> = None;
    for selection in &cursors.selections {
        let target = block_target(buffer, *selection);
        if previous.is_some_and(|p| target.start <= p.end) {
            continue;
        }
        previous = Some(target);
        edits.extend(toggle_block(buffer, target, &open, &close));
    }
    finish(edits, cursors, base)
}

/// What a block toggle acts on for `selection`: the selection itself, or for a bare
/// caret its line's content (an empty range at the caret on a blank line).
fn block_target(buffer: &TextBuffer, selection: Selection) -> Range {
    let range = selection.range();
    if !range.is_empty() {
        return range;
    }
    let caret = selection.head;
    let text: Vec<char> = buffer
        .line(caret.line as usize)
        .map(|t| t.chars().collect())
        .unwrap_or_default();
    if is_blank(&text) {
        return range;
    }
    Range {
        start: LineCol::new(caret.line, leading(&text)),
        end: LineCol::new(caret.line, content_end(&text)),
    }
}

/// The edits that wrap or unwrap `target` in a block comment.
fn toggle_block(
    buffer: &TextBuffer,
    target: Range,
    open: &[char],
    close: &[char],
) -> Vec<LineEdit> {
    let (open_w, close_w) = (width(open), width(close));
    if target.is_empty() {
        let text: String = open.iter().chain(&[' ', ' ']).chain(close).collect();
        return vec![LineEdit::insert(target.start, text, open_w + 1)];
    }
    if let Some((start, end)) = trimmed(buffer, target) {
        let first = line_chars(buffer, start.line);
        let last = line_chars(buffer, end.line);
        let starts = first
            .get(start.col as usize..)
            .is_some_and(|t| t.starts_with(open));
        let ends = last
            .get(..end.col as usize)
            .is_some_and(|t| t.ends_with(close));
        let long_enough = start.line < end.line || end.col - start.col >= open_w + close_w;
        if starts && ends && long_enough {
            let inner_start = start.col + open_w;
            let close_at = end.col - close_w;
            // Room for a padding space between the delimiters on a shared line.
            let open_pad = (start.line < end.line || inner_start < close_at)
                && first.get(inner_start as usize) == Some(&' ');
            let open_end = inner_start + u32::from(open_pad);
            let close_pad = close_at > 0
                && last.get(close_at as usize - 1) == Some(&' ')
                && (start.line < end.line || close_at > open_end);
            let close_from = close_at - u32::from(close_pad);
            return vec![
                LineEdit::delete(start, open_w + u32::from(open_pad)),
                LineEdit::delete(LineCol::new(end.line, close_from), end.col - close_from),
            ];
        }
    }
    let open_text: String = open.iter().chain(&[' ']).collect();
    let close_text: String = std::iter::once(&' ').chain(close).collect();
    vec![
        LineEdit::insert(target.start, open_text, open_w + 1),
        LineEdit::insert(target.end, close_text, 0),
    ]
}

/// `range` shrunk past whitespace (line breaks included) at both ends, or `None`
/// when it holds only whitespace.
fn trimmed(buffer: &TextBuffer, range: Range) -> Option<(LineCol, LineCol)> {
    let mut start = None;
    for line in range.start.line..=range.end.line {
        let text = line_chars(buffer, line);
        let from = if line == range.start.line {
            range.start.col
        } else {
            0
        };
        let to = if line == range.end.line {
            range.end.col
        } else {
            width(&text)
        };
        if let Some(col) =
            (from..to).find(|c| text.get(*c as usize).is_some_and(|ch| !ch.is_whitespace()))
        {
            start = Some(LineCol::new(line, col));
            break;
        }
    }
    let start = start?;
    for line in (start.line..=range.end.line).rev() {
        let text = line_chars(buffer, line);
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == range.end.line {
            range.end.col
        } else {
            width(&text)
        };
        if let Some(col) = (from..to)
            .rev()
            .find(|c| text.get(*c as usize).is_some_and(|ch| !ch.is_whitespace()))
        {
            return Some((start, LineCol::new(line, col + 1)));
        }
    }
    None
}

/// Whether `text`'s content is already commented with `prefix` (and `suffix`).
fn is_line_commented(text: &[char], prefix: &[char], suffix: &[char]) -> bool {
    let body = text
        .get(leading(text) as usize..content_end(text) as usize)
        .unwrap_or(&[]);
    body.starts_with(prefix)
        && (suffix.is_empty()
            || (body.ends_with(suffix) && body.len() >= prefix.len() + suffix.len()))
}

/// Remove the marker(s) from a commented line, with one padding space each.
fn uncomment_line(line: u32, text: &[char], prefix: &[char], suffix: &[char]) -> Vec<LineEdit> {
    let start = leading(text);
    let end = content_end(text);
    let inner_start = start + width(prefix);
    let close_at = end - width(suffix);
    let open_pad = text.get(inner_start as usize) == Some(&' ') && inner_start < close_at;
    let open_end = inner_start + u32::from(open_pad);
    let mut edits = vec![LineEdit::delete(
        LineCol::new(line, start),
        open_end - start,
    )];
    if !suffix.is_empty() {
        let close_pad = close_at > open_end && text.get(close_at as usize - 1) == Some(&' ');
        let from = close_at - u32::from(close_pad);
        edits.push(LineEdit::delete(LineCol::new(line, from), end - from));
    }
    edits
}

/// Insert the marker(s) on one line, the prefix at column `indent`.
fn comment_line(
    line: u32,
    text: &[char],
    indent: u32,
    prefix: &[char],
    suffix: &[char],
) -> Vec<LineEdit> {
    let at = LineCol::new(line, indent);
    if suffix.is_empty() {
        let text: String = prefix.iter().chain(&[' ']).collect();
        let w = width(prefix) + 1;
        return vec![LineEdit::insert(at, text, w)];
    }
    let end = content_end(text);
    if end <= indent {
        let text: String = prefix.iter().chain(&[' ', ' ']).chain(suffix).collect();
        return vec![LineEdit::insert(at, text, width(prefix) + 1)];
    }
    vec![
        LineEdit::insert(at, prefix.iter().chain(&[' ']).collect(), width(prefix) + 1),
        LineEdit::insert(
            LineCol::new(line, end),
            std::iter::once(&' ').chain(suffix).collect(),
            0,
        ),
    ]
}

/// Assemble the batch and carry every selection through it.
fn finish(mut edits: Vec<LineEdit>, cursors: &CursorState, base: u64) -> Option<MultiEdit> {
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|e| (e.line, e.col));
    let selections = cursors
        .selections
        .iter()
        .map(|s| Selection {
            anchor: map(s.anchor, &edits),
            head: map(s.head, &edits),
        })
        .collect();
    let text_edits = edits
        .into_iter()
        .map(|e| TextEdit {
            range: Range {
                start: LineCol::new(e.line, e.col),
                end: LineCol::new(e.line, e.col + e.remove),
            },
            new_text: e.text,
        })
        .collect();
    Some(MultiEdit {
        change: Change::new(base, text_edits),
        cursors: CursorState {
            selections,
            primary: cursors.primary,
        },
    })
}

/// Where `pos` (original coordinates) lands once `edits` apply. Every edit is
/// confined to one line, so only edits on `pos`'s own line move it.
fn map(pos: LineCol, edits: &[LineEdit]) -> LineCol {
    let mut col = i64::from(pos.col);
    for e in edits.iter().filter(|e| e.line == pos.line) {
        let inserted = i64::from(width_str(&e.text));
        let end = e.col + e.remove;
        if e.remove == 0 && pos.col == e.col {
            col += i64::from(e.at_col);
        } else if pos.col >= end && pos.col > e.col {
            col += inserted - i64::from(e.remove);
        } else if pos.col > e.col {
            // Inside a removed run: collapse onto its start.
            col -= i64::from(pos.col - e.col);
        }
    }
    LineCol::new(pos.line, u32::try_from(col.max(0)).unwrap_or(u32::MAX))
}

fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .map(|t| t.chars().collect())
        .unwrap_or_default()
}

fn is_blank(text: &[char]) -> bool {
    text.iter().all(|c| c.is_whitespace())
}

/// The number of leading whitespace chars.
fn leading(text: &[char]) -> u32 {
    width(&text[..text.iter().take_while(|c| c.is_whitespace()).count()])
}

/// The column just past the last non-whitespace char (`0` on a blank line).
fn content_end(text: &[char]) -> u32 {
    let trailing = text.iter().rev().take_while(|c| c.is_whitespace()).count();
    width(&text[..text.len() - trailing])
}

fn width(chars: &[char]) -> u32 {
    u32::try_from(chars.len()).unwrap_or(u32::MAX)
}

fn width_str(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}
