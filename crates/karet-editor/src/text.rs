use super::*;

/// Whether the cell at line `l`, column `col` lies within any of `selections`.
pub(super) fn in_any(selections: &[Range], l: u32, col: u32) -> bool {
    selections.iter().any(|r| col_in_range(l, col, *r))
}

/// Whether `c` is part of a word (alphanumeric or underscore), for word motions.
pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The `(start, end)` of the word (alphanumeric + `_`) around `pos` on its line, or a
/// single-character span when `pos` is not on a word character. Mirrors the span a
/// double-click selects; reused by the app's click handling.
#[must_use]
pub fn word_bounds(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    let chars: Vec<char> = buffer
        .line(pos.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let n = chars.len() as u32;
    let col = pos.col.min(n);
    let mut start = col;
    while start > 0 && is_word_char(chars[start as usize - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < n && is_word_char(chars[end as usize]) {
        end += 1;
    }
    if start == end {
        (
            LineCol::new(pos.line, col),
            LineCol::new(pos.line, (col + 1).min(n)),
        )
    } else {
        (LineCol::new(pos.line, start), LineCol::new(pos.line, end))
    }
}

/// The text within `range`, or `None` if either end can't be resolved to a byte.
pub(super) fn slice_text(buffer: &TextBuffer, range: Range) -> Option<String> {
    let start = buffer.line_col_to_byte(range.start).ok()?.0;
    let end = buffer.line_col_to_byte(range.end).ok()?.0;
    buffer.text().get(start..end).map(str::to_string)
}

/// The byte offset of the next occurrence of `needle` in `hay` at or after `from`,
/// wrapping around to the start of the buffer.
pub(super) fn find_next(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let from = from.min(hay.len());
    hay.get(from..)
        .and_then(|tail| tail.find(needle).map(|i| from + i))
        .or_else(|| hay.get(..from).and_then(|head| head.find(needle)))
}

/// The start of the word before `pos`, wrapping to the previous line's end when at
/// column 0. Skips trailing whitespace, then a single word/punctuation run.
pub(super) fn prev_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
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
pub(super) fn next_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
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
        Some(token) => Style::default()
            .fg(theme.color(token).to_ratatui())
            .add_modifier(theme.emphasis(token).to_ratatui()),
        None => Style::default().fg(default_fg.to_ratatui()),
    }
}
