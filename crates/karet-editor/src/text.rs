use super::*;

/// Whether the cell at line `l`, column `col` lies within any of `selections`.
pub(super) fn in_any(selections: &[Range], l: u32, col: u32) -> bool {
    selections.iter().any(|r| col_in_range(l, col, *r))
}

/// Whether the line break ending line `l` (of `line_len` chars) is inside any of
/// `selections`: the break occupies the slot one past the last character, so a
/// selection covers it exactly when it covers that column. An empty line inside
/// a selection is the case with `line_len == 0`.
pub(super) fn newline_in_any(selections: &[Range], l: u32, line_len: u32) -> bool {
    in_any(selections, l, line_len)
}

/// Whether `c` is part of a word, on the shared [`karet_core::word_class`] model.
pub(super) fn is_word_char(c: char) -> bool {
    karet_core::is_word_char(c)
}

/// The `(start, end)` of the word (alphanumeric + `_`) around `pos` on its line, or a
/// single-character span when `pos` is not on a word character. Mirrors the span a
/// double-click selects; reused by the app's click handling.
#[must_use]
pub fn word_bounds(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    word_at(buffer, pos).unwrap_or_else(|| {
        let col = pos.col.min(line_len(buffer, pos.line));
        let end = (col + 1).min(line_len(buffer, pos.line));
        (LineCol::new(pos.line, col), LineCol::new(pos.line, end))
    })
}

/// The `(start, end)` of the word touching `pos` on its line — the word `pos` is
/// inside, or ends or starts at — or `None` when no word character touches it.
pub(super) fn word_at(buffer: &TextBuffer, pos: LineCol) -> Option<(LineCol, LineCol)> {
    let chars = line_chars(buffer, pos.line);
    let n = chars.len() as u32;
    let col = pos.col.min(n);
    let word = |i: u32| chars.get(i as usize).copied().is_some_and(is_word_char);
    let mut start = col;
    while start > 0 && word(start - 1) {
        start -= 1;
    }
    let mut end = col;
    while end < n && word(end) {
        end += 1;
    }
    (start < end).then(|| (LineCol::new(pos.line, start), LineCol::new(pos.line, end)))
}

/// The text within `range`, read line by line so the document is never copied
/// whole. Lines are joined with `\n`, the buffer's normalized line break.
pub(super) fn range_text(buffer: &TextBuffer, range: Range) -> String {
    (range.start.line..=range.end.line)
        .map(|l| {
            let chars = line_chars(buffer, l);
            let n = chars.len();
            let lo = if l == range.start.line {
                (range.start.col as usize).min(n)
            } else {
                0
            };
            let hi = if l == range.end.line {
                (range.end.col as usize).clamp(lo, n)
            } else {
                n
            };
            chars
                .get(lo..hi)
                .unwrap_or_default()
                .iter()
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first occurrence of `needle` starting at or after `from`, wrapping past the
/// end of the buffer back to its start, that `taken` does not reject. With
/// `whole_word`, a match must not continue a word on either side.
///
/// Reads the buffer a line at a time, so a search never copies the document. The
/// line holding `from` is visited twice: first for matches at or after `from`, then,
/// after the wrap, for those before it. `None` once every occurrence is rejected.
pub(super) fn find_next(
    buffer: &TextBuffer,
    needle: &str,
    from: LineCol,
    whole_word: bool,
    taken: impl Fn(Range) -> bool,
) -> Option<Range> {
    if needle.is_empty() {
        return None;
    }
    let parts: Vec<&str> = needle.split('\n').collect();
    let lines = (from.line..=last_line(buffer)).chain(0..=from.line);
    lines.enumerate().find_map(|(i, l)| {
        matches_on_line(buffer, l, &parts, whole_word)
            .into_iter()
            .filter(|r| match (i, l == from.line) {
                (0, _) => r.start >= from,
                (_, true) => r.start < from,
                _ => true,
            })
            .find(|r| !taken(*r))
    })
}

/// Every occurrence of the needle `parts` (its text split at line breaks) that starts
/// on line `line`, left to right and non-overlapping.
fn matches_on_line(buffer: &TextBuffer, line: u32, parts: &[&str], whole_word: bool) -> Vec<Range> {
    let Some(text) = buffer.line(line as usize) else {
        return Vec::new();
    };
    let col_of = |s: &str, byte: usize| s.get(..byte).map_or(0, |h| h.chars().count() as u32);
    let bounded = |before: Option<char>, after: Option<char>| {
        !whole_word || !(before.is_some_and(is_word_char) || after.is_some_and(is_word_char))
    };
    match parts {
        [] => Vec::new(),
        [needle] => text
            .match_indices(needle)
            .filter(|(b, m)| {
                let before = text.get(..*b).and_then(|h| h.chars().next_back());
                let after = text.get(b + m.len()..).and_then(|t| t.chars().next());
                bounded(before, after)
            })
            .map(|(b, m)| {
                let start = col_of(&text, b);
                Range {
                    start: LineCol::new(line, start),
                    end: LineCol::new(line, start + m.chars().count() as u32),
                }
            })
            .collect(),
        [first, middle @ .., last] => {
            let Some(head) = text.strip_suffix(first) else {
                return Vec::new();
            };
            let end_line = line + 1 + middle.len() as u32;
            let middle_matches = middle
                .iter()
                .zip(line + 1..)
                .all(|(part, l)| buffer.line(l as usize).as_deref() == Some(*part));
            let Some(tail) = buffer.line(end_line as usize) else {
                return Vec::new();
            };
            let after = tail.strip_prefix(last).map(|t| t.chars().next());
            match after {
                Some(after) if middle_matches && bounded(head.chars().next_back(), after) => {
                    vec![Range {
                        start: LineCol::new(line, head.chars().count() as u32),
                        end: LineCol::new(end_line, last.chars().count() as u32),
                    }]
                },
                _ => Vec::new(),
            }
        },
    }
}

/// The start of the word before `pos`, wrapping to the previous line's end when at
/// column 0. Skips trailing whitespace, then a single word/punctuation run.
#[must_use]
pub fn previous_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    if pos.col == 0 {
        return if pos.line > 0 {
            let line = pos.line - 1;
            LineCol::new(line, line_len(buffer, line))
        } else {
            pos
        };
    }
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let mut i = (pos.col as usize).min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i > 0 {
        let word = is_word_char(chars[i - 1]);
        while i > 0 && !chars[i - 1].is_whitespace() && is_word_char(chars[i - 1]) == word {
            i -= 1;
        }
    }
    LineCol::new(pos.line, i as u32)
}

/// The end of the word after `pos`, wrapping to the next line's start at end of line.
/// Skips leading whitespace, then a single word/punctuation run.
#[must_use]
pub fn next_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let n = chars.len();
    if pos.col as usize >= n {
        return if pos.line < last_line(buffer) {
            LineCol::new(pos.line + 1, 0)
        } else {
            pos
        };
    }
    let mut i = pos.col as usize;
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    if i < n {
        let word = is_word_char(chars[i]);
        while i < n && !chars[i].is_whitespace() && is_word_char(chars[i]) == word {
            i += 1;
        }
    }
    LineCol::new(pos.line, i as u32)
}

/// Whether line `l` is hidden inside the interior of a collapsed fold in `folds`.
pub(super) fn hidden_in(folds: &[Fold], l: u32) -> bool {
    folds
        .iter()
        .any(|f| f.collapsed && l > f.start && l <= f.end)
}

/// The index of the last line in `buffer` (0 for an empty buffer).
pub(super) fn last_line(buffer: &TextBuffer) -> u32 {
    (buffer.line_count().max(1) - 1) as u32
}

/// The length (in `char`s) of line `line` in `buffer`.
pub(super) fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    buffer
        .line(line as usize)
        .map_or(0, |s| s.chars().count() as u32)
}

/// The `char`s of line `line` in `buffer` (empty past the end).
pub(super) fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect()
}

/// The number of decimal digits needed to print `n`.
pub(super) fn digit_count(n: u32) -> usize {
    if n < 10 { 1 } else { (n.ilog10() + 1) as usize }
}

/// Whether line `l` falls within the line span of `range`.
pub(super) fn line_in_range(l: u32, range: Range) -> bool {
    l >= range.start.line && l <= range.end.line
}

/// Whether column `col` on line `l` falls within `range`.
pub(super) fn col_in_range(l: u32, col: u32, range: Range) -> bool {
    if !line_in_range(l, range) {
        return false;
    }
    let lo = if l == range.start.line {
        range.start.col
    } else {
        0
    };
    let hi = if l == range.end.line {
        range.end.col
    } else {
        u32::MAX
    };
    col >= lo && col < hi
}

/// The semantic token covering absolute byte `abs`, if any highlight span claims it.
pub(super) fn token_at(abs: usize, hl: &[HighlightSpan]) -> Option<TokenId> {
    hl.iter()
        .find(|s| s.span.start.0 <= abs && abs < s.span.end.0)
        .map(|s| s.token)
}

/// The style (foreground + emphasis) for the char at absolute byte `abs`. Markup
/// tokens carry bold/italic, so color alone is not enough.
pub(super) fn token_style(
    abs: usize,
    hl: &[HighlightSpan],
    theme: &Theme,
    default_fg: Rgba,
) -> Style {
    match token_at(abs, hl) {
        Some(token) => theme.token_style(token),
        None => Style::default().fg(default_fg.to_ratatui()),
    }
}

/// The text within `range`, sliced from `source` (the buffer's full text) using
/// byte offsets derived from `buffer`. `None` if the range cannot be resolved.
#[must_use]
pub fn selection_text(buffer: &TextBuffer, source: &str, range: Range) -> Option<String> {
    let start = buffer.line_col_to_byte(range.start).ok()?.0;
    let end = buffer.line_col_to_byte(range.end).ok()?.0;
    source.get(start..end).map(str::to_string)
}

/// The `(anchor, head)` span covering all of `line`.
#[must_use]
pub fn line_span(buffer: &TextBuffer, line: u32) -> (LineCol, LineCol) {
    let len = buffer
        .line(line as usize)
        .map_or(0, |s| s.chars().count() as u32);
    (LineCol::new(line, 0), LineCol::new(line, len))
}
