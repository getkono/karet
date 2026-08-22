//! Line-based markdown editing helpers: list continuation, ordered-list
//! renumbering, task toggling, and emphasis toggles.
//!
//! Everything here is pure text-in/data-out — the caller (an editor) turns the
//! answers into its own edit/undo machinery. Columns are **character**
//! indices, matching editor caret coordinates; returned line text never
//! includes the trailing newline.

/// The marker of a list item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListMarker {
    /// An unordered bullet: `-`, `*`, or `+`.
    Bullet(char),
    /// An ordered marker: the number plus its `.` or `)` delimiter.
    Ordered {
        /// The item number as written.
        number: u64,
        /// The delimiter following the number.
        delimiter: char,
    },
}

impl ListMarker {
    /// The marker text an item *following* this one carries (`- `, `7. `).
    #[must_use]
    pub fn next(&self) -> String {
        match self {
            Self::Bullet(c) => format!("{c} "),
            Self::Ordered { number, delimiter } => format!("{}{delimiter} ", number + 1),
        }
    }
}

/// What a line says about the list item it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListContext {
    /// The leading whitespace, verbatim.
    pub indent: String,
    /// The marker.
    pub marker: ListMarker,
    /// The task state, when the item carries a `[ ]`/`[x]` checkbox.
    pub task: Option<bool>,
    /// Character column where the item's content starts (after marker, space,
    /// and any checkbox).
    pub content_col: usize,
    /// Whether there is no content after the marker (an empty item).
    pub content_empty: bool,
}

/// Parse `line` as a list-item opener, if it is one.
#[must_use]
pub fn list_context(line: &str) -> Option<ListContext> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    let indent: String = chars[..i].iter().collect();
    let marker = match chars.get(i) {
        Some(&c @ ('-' | '*' | '+')) => {
            i += 1;
            ListMarker::Bullet(c)
        },
        Some(c) if c.is_ascii_digit() => {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            // Ten digits is already past what renumbering should touch.
            if i - start > 9 {
                return None;
            }
            let number: u64 = chars[start..i]
                .iter()
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            let delimiter = match chars.get(i) {
                Some(&d @ ('.' | ')')) => {
                    i += 1;
                    d
                },
                _ => return None,
            };
            ListMarker::Ordered { number, delimiter }
        },
        _ => return None,
    };
    // The marker must be followed by a space (or end the line — an item being
    // typed right now).
    match chars.get(i) {
        Some(' ') => i += 1,
        None => {},
        Some(_) => return None,
    }
    // An optional GFM task checkbox, itself followed by a space or line end.
    let task = match (chars.get(i), chars.get(i + 1), chars.get(i + 2)) {
        (Some('['), Some(' '), Some(']')) => Some(false),
        (Some('['), Some('x' | 'X'), Some(']')) => Some(true),
        _ => None,
    };
    if task.is_some() {
        i += 3;
        if chars.get(i) == Some(&' ') {
            i += 1;
        }
    }
    let content_empty = chars[i..].iter().all(|c| c.is_whitespace());
    Some(ListContext {
        indent,
        marker,
        task,
        content_col: i,
        content_empty,
    })
}

/// Whether character `line` (0-based) of `text` lies inside a fenced code
/// block — where list/emphasis editing must stay inert.
#[must_use]
pub fn in_fenced_code_block(text: &str, line: usize) -> bool {
    let mut fence: Option<(char, usize)> = None;
    for (i, l) in text.lines().enumerate() {
        if i >= line {
            break;
        }
        let trimmed = l.trim_start();
        let (c, len) = if trimmed.starts_with("```") {
            ('`', trimmed.chars().take_while(|&c| c == '`').count())
        } else if trimmed.starts_with("~~~") {
            ('~', trimmed.chars().take_while(|&c| c == '~').count())
        } else {
            continue;
        };
        match fence {
            // A closing fence must match the opener's character and length.
            Some((open, open_len)) if c == open && len >= open_len => fence = None,
            Some(_) => {},
            None => fence = Some((c, len)),
        }
    }
    fence.is_some()
}

/// What pressing Enter at the end of a list line should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListContinuation {
    /// Start the next item: insert a newline plus this text (indent + marker,
    /// with an unchecked checkbox when the item was a task).
    Continue {
        /// The text following the inserted newline.
        insert: String,
    },
    /// The item is empty: end the list by deleting the marker — everything
    /// before `marker_end` on the line.
    EndList {
        /// Character column one past the marker (and checkbox, if any).
        marker_end: usize,
    },
}

/// The list continuation for Enter pressed at character column `col` of line
/// `line`, or `None` when ordinary newline behavior should apply.
///
/// Only fires at or past the item's content start (Enter inside the indent or
/// marker splits the line like anywhere else), and never inside a fenced code
/// block.
#[must_use]
pub fn continue_list(text: &str, line: usize, col: usize) -> Option<ListContinuation> {
    if in_fenced_code_block(text, line) {
        return None;
    }
    let ctx = list_context(text.lines().nth(line)?)?;
    if col < ctx.content_col {
        return None;
    }
    if ctx.content_empty {
        return Some(ListContinuation::EndList {
            marker_end: ctx.content_col,
        });
    }
    let mut insert = ctx.indent.clone();
    insert.push_str(&ctx.marker.next());
    if ctx.task.is_some() {
        insert.push_str("[ ] ");
    }
    Some(ListContinuation::Continue { insert })
}

/// A whole-line replacement at `line`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineRewrite {
    /// The 0-based line to replace.
    pub line: usize,
    /// Its new text, without the newline.
    pub text: String,
}

/// Renumber the run of same-indent ordered items around `line` so numbers
/// ascend from the run's first item. Lines that already carry the right
/// number produce no rewrite.
#[must_use]
pub fn renumber_ordered(text: &str, line: usize) -> Vec<LineRewrite> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(anchor) = lines.get(line).copied().and_then(list_context) else {
        return Vec::new();
    };
    let ListMarker::Ordered { .. } = anchor.marker else {
        return Vec::new();
    };
    let member = |l: &str| {
        list_context(l)
            .filter(|c| c.indent == anchor.indent && matches!(c.marker, ListMarker::Ordered { .. }))
    };
    // Walk to the run's first item: contiguous lines that are members, or
    // more-indented continuation lines (nested content stays inside the run).
    let deeper = |l: &str| {
        let ws = l.chars().take_while(|c| c.is_whitespace()).count();
        !l.trim().is_empty() && ws > anchor.indent.chars().count()
    };
    let mut start = line;
    while start > 0 {
        let prev = lines[start - 1];
        if member(prev).is_some() || deeper(prev) {
            start -= 1;
        } else {
            break;
        }
    }
    while start < line && member(lines[start]).is_none() {
        start += 1;
    }
    let mut rewrites = Vec::new();
    let mut expected: Option<u64> = None;
    let mut i = start;
    while i < lines.len() {
        let l = lines[i];
        if let Some(ctx) = member(l) {
            let ListMarker::Ordered { number, .. } = ctx.marker else {
                break;
            };
            let n = expected.unwrap_or(number);
            if number != n {
                let rest: String = l.chars().skip(ctx.indent.chars().count()).collect();
                let after_marker: String = rest
                    .chars()
                    .skip_while(|c| c.is_ascii_digit())
                    .collect::<String>();
                rewrites.push(LineRewrite {
                    line: i,
                    text: format!("{}{n}{after_marker}", ctx.indent),
                });
            }
            expected = Some(n + 1);
            i += 1;
        } else if deeper(l) && expected.is_some() {
            i += 1; // nested content between siblings
        } else {
            break;
        }
    }
    rewrites
}

/// Toggle the task checkbox on `line`: `[ ]` ⇄ `[x]`. `None` when the line
/// is not a task item.
#[must_use]
pub fn toggle_task(line: &str) -> Option<String> {
    let ctx = list_context(line)?;
    let done = ctx.task?;
    let box_start = ctx.content_col
        - if line.chars().nth(ctx.content_col.saturating_sub(1)) == Some(' ') {
            4
        } else {
            3
        };
    let mut chars: Vec<char> = line.chars().collect();
    chars[box_start + 1] = if done { ' ' } else { 'x' };
    Some(chars.into_iter().collect())
}

/// An emphasis toggle's outcome: the new line text plus where the selection
/// lands (character columns), so the caller can keep the same text selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineToggle {
    /// The new text of the line.
    pub text: String,
    /// New selection start column.
    pub start: usize,
    /// New selection end column.
    pub end: usize,
}

/// Toggle `marker` (e.g. `**`, `*`, `~~`, `` ` ``) around `start..end` of
/// `line` (character columns). An empty selection expands to the word under
/// the caret first; `None` when there is nothing to toggle (no word, or the
/// span cannot hold the marker).
///
/// Toggling is symmetric: a span already wrapped in `marker` (inside or
/// immediately outside the selection) unwraps instead.
#[must_use]
pub fn toggle_surround(line: &str, start: usize, end: usize, marker: &str) -> Option<InlineToggle> {
    let chars: Vec<char> = line.chars().collect();
    let m: Vec<char> = marker.chars().collect();
    let ml = m.len();
    let (mut s, mut e) = (start.min(chars.len()), end.min(chars.len()));
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    if s == e {
        // Expand to the word under the caret.
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let mut ws = s;
        while ws > 0 && chars.get(ws - 1).copied().is_some_and(is_word) {
            ws -= 1;
        }
        let mut we = s;
        while chars.get(we).copied().is_some_and(is_word) {
            we += 1;
        }
        if ws == we {
            return None;
        }
        (s, e) = (ws, we);
    }
    let has = |from: usize| {
        chars
            .get(from..from + ml)
            .is_some_and(|w| w == m.as_slice())
    };
    // Unwrap when the marker sits just inside the span…
    if e - s >= 2 * ml && has(s) && has(e - ml) {
        let mut text: String = chars[..s].iter().collect();
        text.extend(&chars[s + ml..e - ml]);
        text.extend(&chars[e..]);
        return Some(InlineToggle {
            text,
            start: s,
            end: e - 2 * ml,
        });
    }
    // …or immediately outside it.
    if s >= ml && has(s - ml) && has(e) {
        let mut text: String = chars[..s - ml].iter().collect();
        text.extend(&chars[s..e]);
        text.extend(&chars[e + ml..]);
        return Some(InlineToggle {
            text,
            start: s - ml,
            end: e - ml,
        });
    }
    // Otherwise wrap.
    let mut text: String = chars[..s].iter().collect();
    text.push_str(marker);
    text.extend(&chars[s..e]);
    text.push_str(marker);
    text.extend(&chars[e..]);
    Some(InlineToggle {
        text,
        start: s + ml,
        end: e + ml,
    })
}

#[cfg(test)]
mod tests;
