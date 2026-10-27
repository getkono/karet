use karet_core::HighlightSpan;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::TokenId;
use karet_text::TextBuffer;
pub(crate) use karet_text::words::word_at;
use karet_theme::Rgba;
use karet_theme::Theme;
use ratatui::style::Style;

use super::hint::LineHints;
use crate::Fold;
use crate::cluster_bounds;
use crate::cluster_widths;

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

/// Every occurrence of `needle` in the buffer, in document order, under the same
/// matching rules as [`find_next`]. Empty for an empty needle.
pub(super) fn find_all(buffer: &TextBuffer, needle: &str, whole_word: bool) -> Vec<Range> {
    if needle.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = needle.split('\n').collect();
    (0..=last_line(buffer))
        .flat_map(|l| matches_on_line(buffer, l, &parts, whole_word))
        .collect()
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

/// Chars read past a row's right edge, so the grapheme cluster that straddles
/// the edge is segmented whole — a flag or a ZWJ emoji sequence is several
/// chars — and keeps the width it has when the full line is measured.
pub(super) const GRAPHEME_LOOKAHEAD: u32 = 32;

/// The chars of unwrapped line `l` that a row starting at column `start` with
/// `room` cells can show, plus [`GRAPHEME_LOOKAHEAD`], and their
/// [`cluster_widths`].
///
/// The window starts at `start + 2 × room` chars and doubles until the chars
/// past `start` (the lookahead aside) cover `room` cells, or the line ends. It
/// is sized by cells rather than chars because a cluster can be many chars to
/// one glyph: a ZWJ family emoji is five chars in two cells, so a fixed char
/// budget would stop painting well short of the right edge. A tab counts one
/// cell and hints none, so the estimate never overshoots what the walk paints.
///
/// The doubling stops at `room ×` [`GRAPHEME_LOOKAHEAD`] chars past `start`:
/// one cell's cluster is taken to span at most the lookahead, the same bound
/// the right edge already relies on. Without it a line of only zero-width chars
/// (a million U+200B) never covers a cell, so every pass would re-copy and
/// re-segment it from char 0 until the window reached the line end.
pub(super) fn row_window(
    buffer: &TextBuffer,
    l: u32,
    start: u32,
    room: u32,
) -> (Vec<char>, Vec<u32>) {
    let most = room.saturating_mul(GRAPHEME_LOOKAHEAD).max(1);
    let mut visible = room.saturating_mul(2).clamp(1, most);
    loop {
        let cap = start
            .saturating_add(visible)
            .saturating_add(GRAPHEME_LOOKAHEAD);
        let chars = line_chars_prefix(buffer, l, cap);
        let widths = cluster_widths(&chars);
        let measured = chars.len().saturating_sub(GRAPHEME_LOOKAHEAD as usize);
        let covered: u32 = chars
            .iter()
            .zip(&widths)
            .take(measured)
            .skip(start as usize)
            .map(|(&ch, &w)| if ch == '\t' { 1 } else { w })
            .fold(0, u32::saturating_add);
        if chars.len() < cap as usize || covered >= room || cap == u32::MAX || visible >= most {
            return (chars, widths);
        }
        visible = visible.saturating_mul(2).min(most);
    }
}

/// The `char`s of line `line` in `buffer` (empty past the end).
pub(super) fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect()
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
