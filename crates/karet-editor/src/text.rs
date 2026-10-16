use unicode_segmentation::UnicodeSegmentation;

use super::hint::LineHints;

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
/// single-cluster span when `pos` is not on a word character. Mirrors the span a
/// double-click selects; reused by the app's click handling.
#[must_use]
pub fn word_bounds(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    word_at(buffer, pos).unwrap_or_else(|| {
        let chars = line_chars(buffer, pos.line);
        let col = pos.col.min(chars.len() as u32);
        let start = cluster_bounds(&chars)
            .into_iter()
            .rev()
            .find(|&bound| bound <= col)
            .unwrap_or(0);
        let end = next_cluster(&chars, start);
        (LineCol::new(pos.line, start), LineCol::new(pos.line, end))
    })
}

/// The `(start, end)` of the word touching `pos` on its line — the word `pos` is
/// inside, or ends or starts at — or `None` when no word character touches it.
/// Walks whole grapheme clusters, so a letter's combining marks stay in its word.
pub(super) fn word_at(buffer: &TextBuffer, pos: LineCol) -> Option<(LineCol, LineCol)> {
    let chars = line_chars(buffer, pos.line);
    let n = chars.len() as u32;
    let col = pos.col.min(n);
    let runs = clusters(&chars);
    let word = |i: usize| runs.get(i).is_some_and(|c| is_word_char(c.2));
    let bound = |i: usize| runs.get(i).map_or(n, |c| c.0);
    let at = runs.partition_point(|c| c.1 <= col);
    let mut start = at;
    while start > 0 && word(start - 1) {
        start -= 1;
    }
    let mut end = at;
    while end < runs.len() && word(end) {
        end += 1;
    }
    let (start, end) = (bound(start), bound(end));
    (start < end).then(|| (LineCol::new(pos.line, start), LineCol::new(pos.line, end)))
}

/// The `(start, end)` a double-click at `pos` selects: the run of characters
/// sharing the [`karet_core::WordClass`] of the character under `pos` — a word,
/// a punctuation run, or a whitespace run. The character under `pos` decides
/// alone, so a double-click on the `.` of `foo.bar` selects the `.`, never the
/// word beside it.
///
/// Past the end of a line the last character decides, so a double-click right
/// of `foo bar` selects `bar`. An empty line yields the empty span at its start.
#[must_use]
pub fn class_run_at(buffer: &TextBuffer, pos: LineCol) -> (LineCol, LineCol) {
    let (start, end) = class_run(&line_chars(buffer, pos.line), pos.col);
    (LineCol::new(pos.line, start), LineCol::new(pos.line, end))
}

/// The `(start, end)` of the identifier (alphanumeric + `_`) under the character
/// at `pos`, or `None` when that character is punctuation or whitespace, or `pos`
/// is past the end of its line. Unlike [`word_bounds`], a word that merely ends
/// at `pos` does not count: this answers what the pointer is *on*.
#[must_use]
pub fn identifier_at(buffer: &TextBuffer, pos: LineCol) -> Option<(LineCol, LineCol)> {
    let chars = line_chars(buffer, pos.line);
    clusters(&chars)
        .into_iter()
        .find(|c| c.1 > pos.col)
        .filter(|c| c.0 <= pos.col && is_word_char(c.2))?;
    let (start, end) = class_run(&chars, pos.col);
    Some((LineCol::new(pos.line, start), LineCol::new(pos.line, end)))
}

/// The run of one [`karet_core::WordClass`] in `chars` around column `col`: the
/// grapheme cluster holding `col` decides, or the last one when `col` is past
/// the end. `(0, 0)` for an empty line.
fn class_run(chars: &[char], col: u32) -> (u32, u32) {
    let runs = clusters(chars);
    let Some(last) = runs.len().checked_sub(1) else {
        return (0, 0);
    };
    let at = runs.partition_point(|c| c.1 <= col).min(last);
    let class_of = |i: usize| runs.get(i).map(|c| karet_core::word_class(c.2));
    let class = class_of(at);
    let mut start = at;
    while start > 0 && class_of(start - 1) == class {
        start -= 1;
    }
    let mut end = at + 1;
    while end < runs.len() && class_of(end) == class {
        end += 1;
    }
    let col_of = |i: usize| runs.get(i).map_or(chars.len() as u32, |c| c.0);
    (col_of(start), col_of(end))
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
    let chars = line_chars(buffer, pos.line);
    let runs = clusters(&chars);
    let first = |i: usize| runs.get(i).map_or(' ', |c| c.2);
    let mut i = runs.partition_point(|c| c.0 < pos.col);
    while i > 0 && first(i - 1).is_whitespace() {
        i -= 1;
    }
    if i > 0 {
        let word = is_word_char(first(i - 1));
        while i > 0 && !first(i - 1).is_whitespace() && is_word_char(first(i - 1)) == word {
            i -= 1;
        }
    }
    LineCol::new(pos.line, runs.get(i).map_or(chars.len() as u32, |c| c.0))
}

/// The end of the word after `pos`, wrapping to the next line's start at end of line.
/// Skips leading whitespace, then a single word/punctuation run.
#[must_use]
pub fn next_word_boundary(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let chars = line_chars(buffer, pos.line);
    let n = chars.len() as u32;
    if pos.col >= n {
        return if pos.line < last_line(buffer) {
            LineCol::new(pos.line + 1, 0)
        } else {
            pos
        };
    }
    let runs = clusters(&chars);
    let first = |i: usize| runs.get(i).map_or(' ', |c| c.2);
    let mut i = runs.partition_point(|c| c.1 <= pos.col);
    while i < runs.len() && first(i).is_whitespace() {
        i += 1;
    }
    if i < runs.len() {
        let word = is_word_char(first(i));
        while i < runs.len() && !first(i).is_whitespace() && is_word_char(first(i)) == word {
            i += 1;
        }
    }
    LineCol::new(pos.line, runs.get(i).map_or(n, |c| c.0))
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

/// The length (in `char`s) of line `line` in `buffer` (0 past the end).
///
/// Read off the rope's line index rather than by copying the line out, so it
/// costs the same on a minified one-line file as on a short line.
pub(super) fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    if line as usize >= buffer.line_count() {
        return 0;
    }
    buffer.clamp(LineCol::new(line, u32::MAX)).col
}

/// The cache key of an [`ExtentCache`]: the buffer content (its edit version,
/// line count and byte length) and the geometry the extents were measured at
/// (viewport width, tab width).
type ExtentKey = (u64, usize, usize, u32, u16);

/// Each line's horizontal extent with no inlay hints, measured once per
/// buffer content and geometry.
///
/// The horizontal scroll clamp needs the widest line in the document, and
/// measuring that copies and walks every line. Remembering each line's answer
/// makes repeated horizontal scrolls — and every frame's visible-line
/// measurements — cost a lookup per line until the next edit. Lines that carry
/// inlay hints are measured live by [`line_extent`], since hints change between
/// frames without the buffer changing.
#[derive(Clone, Debug, Default)]
pub(super) struct ExtentCache {
    key: Option<ExtentKey>,
    extents: Vec<Option<u32>>,
}

impl ExtentCache {
    /// Line `line`'s hint-free [`horizontal_extent`] at `width` and
    /// `tab_width`, measured on first request after the buffer or geometry
    /// changed (0 past the end).
    ///
    /// [`horizontal_extent`]: super::visual::horizontal_extent
    pub(super) fn plain(
        &mut self,
        buffer: &TextBuffer,
        line: u32,
        width: u32,
        tab_width: u16,
    ) -> u32 {
        let key = (
            buffer.version(),
            buffer.line_count(),
            buffer.len_bytes(),
            width,
            tab_width,
        );
        if self.key != Some(key) {
            self.key = Some(key);
            self.extents.clear();
            self.extents.resize(buffer.line_count(), None);
        }
        let Some(slot) = self.extents.get_mut(line as usize) else {
            return 0;
        };
        *slot.get_or_insert_with(|| {
            super::visual::horizontal_extent(
                &line_chars(buffer, line),
                width,
                tab_width,
                LineHints::default(),
            )
        })
    }
}

/// Line `line`'s horizontal extent with `hints` painted on it: cached in
/// `cache` when the line has none, measured live otherwise.
pub(super) fn line_extent(
    cache: &mut ExtentCache,
    buffer: &TextBuffer,
    line: u32,
    width: u32,
    tab_width: u16,
    hints: LineHints<'_>,
) -> u32 {
    if hints.is_empty() {
        cache.plain(buffer, line, width, tab_width)
    } else {
        super::visual::horizontal_extent(&line_chars(buffer, line), width, tab_width, hints)
    }
}

/// The first `cap` `char`s of line `line` in `buffer` — all of them when the
/// line is shorter, none past the end.
///
/// Only that prefix is copied out of the rope, so a renderer that needs the
/// columns up to the right edge of the viewport pays for those and not for
/// the rest of a megabyte-long line.
pub(super) fn line_chars_prefix(buffer: &TextBuffer, line: u32, cap: u32) -> Vec<char> {
    let Some(span) = buffer.line_to_byte_range(line as usize) else {
        return Vec::new();
    };
    let end = buffer
        .line_col_to_byte(LineCol::new(line, cap))
        .unwrap_or(span.end);
    buffer
        .slice(karet_core::Span {
            start: span.start,
            end,
        })
        .unwrap_or_default()
        .chars()
        .collect()
}

/// The `char`s of line `line` in `buffer` (empty past the end).
pub(super) fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect()
}

/// The column of every extended grapheme cluster boundary in `chars`, ascending:
/// `0`, each later cluster's start, and `chars.len()`. These are the only columns
/// a caret rests on, so a step or a delete never splits a user-perceived
/// character (a combining mark, a ZWJ emoji sequence, a flag).
#[must_use]
pub fn cluster_bounds(chars: &[char]) -> Vec<u32> {
    let text: String = chars.iter().collect();
    let mut bounds = Vec::with_capacity(chars.len().saturating_add(1));
    let mut col = 0_u32;
    bounds.push(col);
    for cluster in text.graphemes(true) {
        col = col.saturating_add(cluster.chars().count() as u32);
        bounds.push(col);
    }
    bounds
}

/// The cluster boundary strictly before `col` in `chars` (`0` at the start).
pub(super) fn previous_cluster(chars: &[char], col: u32) -> u32 {
    cluster_bounds(chars)
        .into_iter()
        .rev()
        .find(|&bound| bound < col)
        .unwrap_or(0)
}

/// The cluster boundary strictly after `col` in `chars` (the line's length at
/// its end).
pub(super) fn next_cluster(chars: &[char], col: u32) -> u32 {
    cluster_bounds(chars)
        .into_iter()
        .find(|&bound| bound > col)
        .unwrap_or(chars.len() as u32)
}

/// The column one grapheme cluster left of `pos` on its line.
pub(crate) fn previous_cluster_col(buffer: &TextBuffer, pos: LineCol) -> u32 {
    previous_cluster(&line_chars(buffer, pos.line), pos.col)
}

/// The column one grapheme cluster right of `pos` on its line.
pub(crate) fn next_cluster_col(buffer: &TextBuffer, pos: LineCol) -> u32 {
    next_cluster(&line_chars(buffer, pos.line), pos.col)
}

/// The clusters of `chars` as `(start, end)` column spans, with the first
/// `char` of each, which decides its word class: a base letter carries its
/// combining marks into a word, and an emoji sequence stays one symbol.
pub(super) fn clusters(chars: &[char]) -> Vec<(u32, u32, char)> {
    cluster_bounds(chars)
        .windows(2)
        .filter_map(|pair| {
            let (start, end) = (pair[0], pair[1]);
            Some((start, end, *chars.get(start as usize)?))
        })
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
///
/// `hl` is byte-ordered and non-overlapping (the [`Highlights`] invariant), so
/// the covering span is found by bisection. A linear scan here made painting a
/// row cost its characters times its spans, which on a minified line with
/// tens of thousands of tokens is quadratic.
pub(super) fn token_at(abs: usize, hl: &[HighlightSpan]) -> Option<TokenId> {
    let first = hl.partition_point(|s| s.span.end.0 <= abs);
    hl.get(first)
        .filter(|s| s.span.start.0 <= abs)
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

/// The `(anchor, head)` span covering all of `line` and the line break ending
/// it: from the line's start to the next line's start, so a triple-click
/// selection deletes or copies the whole line. The last line has no break, so
/// its span ends at its own end.
#[must_use]
pub fn line_span(buffer: &TextBuffer, line: u32) -> (LineCol, LineCol) {
    let head = if line < last_line(buffer) {
        LineCol::new(line + 1, 0)
    } else {
        LineCol::new(line, line_len(buffer, line))
    };
    (LineCol::new(line, 0), head)
}
