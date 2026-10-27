//! The word model over a [`TextBuffer`]: which columns a word, a double-click
//! run, or a word motion spans on a line.
//!
//! Columns are `char` columns (the [`LineCol`] convention), and every answer
//! falls on an extended grapheme cluster boundary: a cluster's first `char`
//! decides its [`karet_core::WordClass`], so a letter carries its combining
//! marks into its word and an emoji sequence stays one symbol. A word is a run
//! of [`karet_core::is_word_char`] clusters (alphanumeric and `_`).

use karet_core::LineCol;
use karet_core::is_word_char;
use karet_core::word_class;
use unicode_segmentation::UnicodeSegmentation;

use crate::TextBuffer;

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

/// The `(start, end)` of the word touching `pos` on its line: the word `pos` is
/// inside, or ends or starts at. `None` when no word character touches it, or
/// `pos` is past the last line. A column past the end of the line reads as the
/// line's end.
#[must_use]
pub fn word_at(buffer: &TextBuffer, pos: LineCol) -> Option<(LineCol, LineCol)> {
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

/// The `(start, end)` of the word around `pos` on its line, as [`word_at`]
/// finds it, or the single grapheme cluster at `pos` when no word touches it
/// (an empty span on an empty line).
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
/// is past the end of its line. Unlike [`word_at`], a word that merely ends at
/// `pos` does not count: this answers what the pointer is *on*.
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

/// The run of one [`karet_core::WordClass`] in `chars` around column `col`: the
/// grapheme cluster holding `col` decides, or the last one when `col` is past
/// the end. `(0, 0)` for an empty line.
fn class_run(chars: &[char], col: u32) -> (u32, u32) {
    let runs = clusters(chars);
    let Some(last) = runs.len().checked_sub(1) else {
        return (0, 0);
    };
    let at = runs.partition_point(|c| c.1 <= col).min(last);
    let class_of = |i: usize| runs.get(i).map(|c| word_class(c.2));
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

/// The clusters of `chars` as `(start, end)` column spans, with the first
/// `char` of each, which decides its word class.
fn clusters(chars: &[char]) -> Vec<(u32, u32, char)> {
    cluster_bounds(chars)
        .windows(2)
        .filter_map(|pair| {
            let (start, end) = (*pair.first()?, *pair.get(1)?);
            Some((start, end, *chars.get(start as usize)?))
        })
        .collect()
}

/// The cluster boundary strictly after `col` in `chars` (the line's length at
/// its end).
fn next_cluster(chars: &[char], col: u32) -> u32 {
    cluster_bounds(chars)
        .into_iter()
        .find(|&bound| bound > col)
        .unwrap_or(chars.len() as u32)
}

/// The `char`s of line `line` in `buffer` (empty past the end).
fn line_chars(buffer: &TextBuffer, line: u32) -> Vec<char> {
    buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect()
}

/// The index of the last line in `buffer` (0 for an empty buffer).
fn last_line(buffer: &TextBuffer) -> u32 {
    (buffer.line_count().max(1) - 1) as u32
}

/// The length (in `char`s) of line `line` in `buffer` (0 past the end), read
/// off the rope's line index rather than by copying the line out.
fn line_len(buffer: &TextBuffer, line: u32) -> u32 {
    if line as usize >= buffer.line_count() {
        return 0;
    }
    buffer.clamp(LineCol::new(line, u32::MAX)).col
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `e` + COMBINING ACUTE ACCENT: one cluster of two chars.
    const E_ACUTE: &str = "e\u{301}";
    /// MAN ZWJ WOMAN ZWJ GIRL: one cluster of five chars.
    const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    /// The flags of Japan and the United States: two clusters of two regional
    /// indicators each.
    const FLAGS: &str = "\u{1F1EF}\u{1F1F5}\u{1F1FA}\u{1F1F8}";

    fn at(line: u32, col: u32) -> LineCol {
        LineCol::new(line, col)
    }

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    /// The `(start, end)` columns of the run a double-click at `col` on line 0 selects.
    fn run_cols(text: &str, col: u32) -> (u32, u32) {
        let (start, end) = class_run_at(&TextBuffer::from_text(text), at(0, col));
        (start.col, end.col)
    }

    #[test]
    fn cluster_bounds_mark_only_user_perceived_character_edges() {
        assert_eq!(cluster_bounds(&chars("")), vec![0]);
        assert_eq!(
            cluster_bounds(&chars(&format!("{E_ACUTE}x"))),
            vec![0, 2, 3]
        );
        assert_eq!(
            cluster_bounds(&chars(&format!("a{FAMILY}b"))),
            vec![0, 1, 6, 7]
        );
        assert_eq!(cluster_bounds(&chars(FLAGS)), vec![0, 2, 4]);
        assert_eq!(cluster_bounds(&chars("日本")), vec![0, 1, 2]);
    }

    #[test]
    fn word_at_finds_the_word_the_position_is_in_or_touches() {
        let buffer = TextBuffer::from_text("let x = foo(bar);\n");
        let cols = |col| word_at(&buffer, at(0, col)).map(|(s, e)| (s.col, e.col));
        // Inside, at the start of, and just past the end of `foo`.
        assert_eq!(cols(9), Some((8, 11)));
        assert_eq!(cols(8), Some((8, 11)));
        assert_eq!(cols(11), Some((8, 11)));
        // Between two non-word characters nothing touches.
        assert_eq!(cols(6), None, "between space and `=`");
        assert_eq!(cols(17), None, "after the `;` at the line end");
        // A column past the line end reads as the end; a line past the last is empty.
        let tail = TextBuffer::from_text("abc");
        assert_eq!(word_at(&tail, at(0, 99)), Some((at(0, 0), at(0, 3))));
        assert_eq!(word_at(&tail, at(5, 0)), None);
        assert_eq!(word_at(&TextBuffer::new(), at(0, 0)), None);
    }

    #[test]
    fn word_at_keeps_combining_marks_and_never_splits_a_cluster() {
        // `café` spelled with a combining accent, then ` x`.
        let buffer = TextBuffer::from_text(&format!("caf{E_ACUTE} x"));
        // Just past the accent still touches the word, which spans both chars of `é`.
        assert_eq!(word_at(&buffer, at(0, 5)), Some((at(0, 0), at(0, 5))));
        // A position inside the cluster (between `e` and its mark) is in the word too.
        assert_eq!(word_at(&buffer, at(0, 4)), Some((at(0, 0), at(0, 5))));
        // An emoji sequence is a symbol, not part of a word beside it.
        let emoji = TextBuffer::from_text(&format!("{FAMILY}ab"));
        assert_eq!(word_at(&emoji, at(0, 2)), None);
        assert_eq!(word_at(&emoji, at(0, 5)), Some((at(0, 5), at(0, 7))));
    }

    #[test]
    fn word_bounds_spans_the_word_under_pos() {
        let buffer = TextBuffer::from_text("foo bar");
        assert_eq!(word_bounds(&buffer, at(0, 5)), (at(0, 4), at(0, 7)));
    }

    #[test]
    fn word_bounds_falls_back_to_the_cluster_off_a_word() {
        let buffer = TextBuffer::from_text(&format!("ab {FAMILY} cd\n"));
        // Off a word, a double-click falls back to the whole cluster.
        assert_eq!(word_bounds(&buffer, at(0, 3)), (at(0, 3), at(0, 8)));
        assert_eq!(word_bounds(&buffer, at(0, 5)), (at(0, 3), at(0, 8)));
        // An empty line yields the empty span at its start.
        let empty = TextBuffer::from_text("\n");
        assert_eq!(word_bounds(&empty, at(0, 4)), (at(0, 0), at(0, 0)));
    }

    #[test]
    fn class_run_at_selects_the_run_under_the_character_alone() {
        // A word, wherever in it the pointer is.
        assert_eq!(run_cols("foo.bar", 0), (0, 3));
        assert_eq!(run_cols("foo.bar", 2), (0, 3));
        // Punctuation right after a word is its own run, not the word before it.
        assert_eq!(run_cols("foo.bar", 3), (3, 4));
        assert_eq!(run_cols("a::b", 2), (1, 3));
        // Whitespace between words is a run of its own.
        assert_eq!(run_cols("foo  bar", 3), (3, 5));
        assert_eq!(run_cols("foo bar", 3), (3, 4));
        // Past the end of the line, the last character decides.
        assert_eq!(run_cols("foo bar", 9), (4, 7));
        assert_eq!(run_cols("foo;", 4), (3, 4));
        // An empty line has nothing to select.
        assert_eq!(run_cols("", 0), (0, 0));
    }

    #[test]
    fn identifier_at_answers_only_on_a_word_character() {
        let buffer = TextBuffer::from_text("let x = foo(bar);");
        let cols = |col| identifier_at(&buffer, at(0, col)).map(|(s, e)| (s.col, e.col));
        assert_eq!(cols(9), Some((8, 11)));
        assert_eq!(cols(4), Some((4, 5)));
        // `(` directly after `foo` is not on `foo`, unlike `word_at`.
        assert_eq!(cols(11), None);
        assert_eq!(cols(3), None, "whitespace");
        assert_eq!(cols(17), None, "past the end of the line");
        assert_eq!(identifier_at(&buffer, at(4, 0)), None, "past the last line");
    }

    #[test]
    fn a_combining_mark_stays_inside_its_word() {
        // `café` spelled with a combining accent: `caf`, `e`, U+0301.
        let buffer = TextBuffer::from_text(&format!("caf{E_ACUTE} x\n"));
        assert_eq!(next_word_boundary(&buffer, at(0, 0)), at(0, 5));
        assert_eq!(previous_word_boundary(&buffer, at(0, 5)), at(0, 0));
        assert_eq!(word_bounds(&buffer, at(0, 2)), (at(0, 0), at(0, 5)));
        assert_eq!(identifier_at(&buffer, at(0, 4)), Some((at(0, 0), at(0, 5))));
        assert_eq!(class_run_at(&buffer, at(0, 1)), (at(0, 0), at(0, 5)));
    }

    #[test]
    fn word_motions_treat_an_emoji_sequence_as_one_symbol() {
        let buffer = TextBuffer::from_text(&format!("ab {FAMILY} cd\n"));
        assert_eq!(next_word_boundary(&buffer, at(0, 2)), at(0, 8));
        assert_eq!(previous_word_boundary(&buffer, at(0, 8)), at(0, 3));
        assert_eq!(class_run_at(&buffer, at(0, 5)), (at(0, 3), at(0, 8)));
        assert_eq!(identifier_at(&buffer, at(0, 4)), None);
    }

    #[test]
    fn word_boundaries_cover_whitespace_punctuation_and_lines() {
        let buffer = TextBuffer::from_text("foo  bar.baz\nnext");
        assert_eq!(previous_word_boundary(&buffer, at(0, 8)), at(0, 5));
        assert_eq!(previous_word_boundary(&buffer, at(1, 0)), at(0, 12));
        assert_eq!(next_word_boundary(&buffer, at(0, 3)), at(0, 8));
        assert_eq!(next_word_boundary(&buffer, at(0, 8)), at(0, 9));
        assert_eq!(next_word_boundary(&buffer, at(0, 12)), at(1, 0));
        // The buffer's two ends stay put.
        assert_eq!(previous_word_boundary(&buffer, at(0, 0)), at(0, 0));
        assert_eq!(next_word_boundary(&buffer, at(1, 4)), at(1, 4));
    }

    #[test]
    fn a_word_left_skips_the_whitespace_before_the_caret_first() {
        let buffer = TextBuffer::from_text("foo  bar\n  baz");
        // Back over the two spaces, then over `foo`.
        assert_eq!(previous_word_boundary(&buffer, at(0, 5)), at(0, 0));
        // Only whitespace before the caret: the line start.
        assert_eq!(previous_word_boundary(&buffer, at(1, 2)), at(1, 0));
    }

    #[test]
    fn line_span_includes_the_line_break_except_on_the_last_line() {
        let buffer = TextBuffer::from_text("alpha\nbravo");
        assert_eq!(line_span(&buffer, 0), (at(0, 0), at(1, 0)));
        assert_eq!(line_span(&buffer, 1), (at(1, 0), at(1, 5)));
        // A trailing newline leaves an empty last line, which the line before ends at.
        let buffer = TextBuffer::from_text("alpha\n");
        assert_eq!(line_span(&buffer, 0), (at(0, 0), at(1, 0)));
        assert_eq!(line_span(&buffer, 1), (at(1, 0), at(1, 0)));
    }
}
