//! The soft-wrap model: where every byte of a buffer lands on screen.
//!
//! One layout serves both halves of the widget. The renderer walks these rows to
//! decide which glyph goes in which cell; the hit test walks the same rows to turn
//! a cell back into a byte offset. They cannot disagree, because there is only one
//! answer to disagree about.
//!
//! # The wrap rule
//!
//! Taken from the editor, so a commit message breaks where a document would: fill
//! a row to the viewport width, then back up to the last whitespace inside it and
//! break there. **The break whitespace stays on the row before the break**, which
//! is what makes every source column map to exactly one display cell — a rule
//! `Paragraph`'s own wrapping does not follow, since it drops the whitespace and
//! leaves those columns with nowhere to be. A word wider than the whole viewport
//! has no whitespace to back up to and is split at the hard width.
//!
//! # Tabs and control characters
//!
//! Every control character, tab included, occupies exactly one cell and paints as
//! a space. A tab-stop model would buy a commit message nothing, and the
//! alternative — writing a raw `\t` into a cell and letting the terminal expand it
//! — desynchronises every cell after it from this layout, which is the whole class
//! of bug this module exists to prevent.

use unicode_width::UnicodeWidthChar;

/// One wrapped display row: the byte range of the buffer it shows.
///
/// The range excludes the `\n` that ended the logical line, so a row's text is
/// exactly what is painted on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapRow {
    /// Byte offset of the row's first character.
    pub start: usize,
    /// Byte offset just past the row's last character.
    pub end: usize,
}

/// The display width of `character` in cells, never less than one.
///
/// A glyph the renderer emits into its own cell always occupies that cell, so a
/// zero-width or combining character counts as one — see the [module docs](self).
#[must_use]
pub fn glyph_width(character: char) -> usize {
    if character.is_control() {
        return 1;
    }
    character.width().unwrap_or(0).max(1)
}

/// What `character` paints as: itself, or a space when it is a control character.
#[must_use]
pub fn glyph_symbol(character: char) -> char {
    if character.is_control() {
        ' '
    } else {
        character
    }
}

/// Wrap `text` to `width` cells, one [`WrapRow`] per display row.
///
/// Never empty: an empty buffer is one empty row, and a trailing `\n` opens a
/// final empty row, exactly as an editor shows them.
#[must_use]
pub fn wrap_rows(text: &str, width: u16) -> Vec<WrapRow> {
    let width = usize::from(width.max(1));
    let mut rows = Vec::new();
    let mut line_start = 0usize;
    loop {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |offset| line_start + offset);
        wrap_line(&text[line_start..line_end], line_start, width, &mut rows);
        if line_end == text.len() {
            return rows;
        }
        line_start = line_end + 1;
    }
}

/// Wrap one logical line (no `\n`) that starts at byte `offset`.
fn wrap_line(line: &str, offset: usize, width: usize, rows: &mut Vec<WrapRow>) {
    if line.is_empty() {
        rows.push(WrapRow {
            start: offset,
            end: offset,
        });
        return;
    }
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let len = chars.len();
    let byte_at = |index: usize| offset + chars.get(index).map_or(line.len(), |(at, _)| *at);
    let mut start = 0usize;
    while start < len {
        // Fill to the hard width first: how many characters could fit at all.
        let mut hard_end = start;
        let mut used = 0usize;
        while hard_end < len {
            let advance = glyph_width(chars[hard_end].1);
            if hard_end > start && used + advance > width {
                break;
            }
            used += advance;
            hard_end += 1;
            if used >= width {
                break;
            }
        }
        // Then back up to the last whitespace, keeping it on this row.
        let end = if hard_end < len {
            chars[start..hard_end]
                .iter()
                .rposition(|(_, character)| character.is_whitespace())
                .map_or(hard_end, |index| start + index + 1)
        } else {
            hard_end
        };
        let end = end.max(start + 1).min(len);
        rows.push(WrapRow {
            start: byte_at(start),
            end: byte_at(end),
        });
        start = end;
    }
}

/// The display cell `(row, column)` the caret sits in for a viewport `width`
/// cells wide.
///
/// `cursor` is an arbitrary byte offset and never panics: past the end of `text`
/// it reports the end of the buffer, and inside a multi-byte character it reports
/// the next character boundary at or after it.
///
/// A caret at the end of an exactly-full row has no cell left on that row, so it
/// reports the row below at column 0 — which is where the renderer paints it, and
/// the only reason a returned row may be one past the last [`WrapRow`].
#[must_use]
pub fn caret_cell(text: &str, cursor: usize, width: u16) -> (u16, u16) {
    let cursor = char_boundary_at_or_after(text, cursor);
    let rows = wrap_rows(text, width);
    // The last row that opens at or before the cursor. A cursor on a `\n` belongs
    // to no row's range — it lands at the end of the row the newline closed.
    let index = rows
        .iter()
        .rposition(|row| row.start <= cursor)
        .unwrap_or(0);
    let row = rows
        .get(index)
        .copied()
        .unwrap_or(WrapRow { start: 0, end: 0 });
    // The cursor never precedes the row that opens at or before it, and never
    // passes that row's end: the next row opens after any `\n` between them.
    let column: usize = text
        .get(row.start..cursor)
        .unwrap_or_default()
        .chars()
        .map(glyph_width)
        .sum();
    if column >= usize::from(width.max(1)) {
        return (clamp_u16(index + 1), 0);
    }
    (clamp_u16(index), clamp_u16(column))
}

/// The wrapped display row holding the caret at byte `cursor`.
#[must_use]
pub fn cursor_row(text: &str, cursor: usize, width: u16) -> u16 {
    caret_cell(text, cursor, width).0
}

/// The byte offset of the character shown at display cell (`row`, `column`) of a
/// buffer wrapped to `width` cells.
///
/// A row past the end of the text maps to the end of the buffer. A column past the
/// end of a row maps to that row's last offset, so clicking in the empty space
/// after a line puts the cursor at its end — and on a soft-wrapped row that offset
/// skips the break whitespace, so the click stays on the row it was made on
/// instead of jumping to the start of the next one.
#[must_use]
pub fn byte_at_row_col(text: &str, row: usize, column: usize, width: u16) -> usize {
    let rows = wrap_rows(text, width);
    let Some(wrapped) = rows.get(row) else {
        return text.len();
    };
    let mut cell = 0usize;
    for (offset, character) in text
        .get(wrapped.start..wrapped.end)
        .unwrap_or_default()
        .char_indices()
    {
        let advance = glyph_width(character);
        if column < cell + advance {
            return wrapped.start + offset;
        }
        cell += advance;
    }
    // Past the row's content. A soft-wrapped row continues into the next one, so
    // stop before the whitespace it broke at rather than landing on the next row.
    let hard = row + 1 >= rows.len()
        || rows
            .get(row + 1)
            .is_some_and(|next| next.start > wrapped.end);
    if hard {
        return wrapped.end;
    }
    let trimmed = text
        .get(wrapped.start..wrapped.end)
        .unwrap_or_default()
        .trim_end();
    wrapped.start + trimmed.len()
}

/// The first character boundary at or after `at`, clamped to the end of `text`.
fn char_boundary_at_or_after(text: &str, at: usize) -> usize {
    if at >= text.len() {
        return text.len();
    }
    let mut at = at;
    while at < text.len() && !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// Saturate a row or column index into the cell coordinates ratatui uses.
fn clamp_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text each wrapped row shows.
    fn painted(text: &str, width: u16) -> Vec<&str> {
        wrap_rows(text, width)
            .into_iter()
            .map(|row| &text[row.start..row.end])
            .collect()
    }

    #[test]
    fn a_row_breaks_at_the_last_whitespace_and_keeps_it() {
        // The break whitespace stays on the row before the break, so every
        // source column has exactly one cell to be in.
        assert_eq!(
            painted("the quick brown fox", 8),
            ["the ", "quick ", "brown ", "fox"]
        );
        assert_eq!(painted("a b", 8), ["a b"], "no break needed");
    }

    #[test]
    fn a_word_wider_than_the_viewport_splits_at_the_hard_width() {
        assert_eq!(painted("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        // There is whitespace to back up to, but not inside the over-long word.
        assert_eq!(painted("hi abcdefghij", 4), ["hi ", "abcd", "efgh", "ij"]);
        // A zero width never stalls the walk.
        assert_eq!(painted("ab", 0), ["a", "b"]);
    }

    #[test]
    fn rows_cover_the_buffer_and_a_trailing_newline_opens_one() {
        assert_eq!(painted("", 5), [""], "an empty buffer is still one row");
        assert_eq!(painted("a\n", 5), ["a", ""]);
        assert_eq!(painted("a\n\nb", 5), ["a", "", "b"]);
        // Every byte outside a newline belongs to exactly one row.
        let text = "the quick brown fox\njumps";
        let rows = wrap_rows(text, 8);
        for (previous, row) in rows.iter().zip(rows.iter().skip(1)) {
            assert!(
                row.start == previous.end || row.start == previous.end + 1,
                "gap between {previous:?} and {row:?}"
            );
        }
        assert_eq!(rows.first().map(|row| row.start), Some(0));
        assert_eq!(rows.last().map(|row| row.end), Some(text.len()));
    }

    #[test]
    fn a_wide_glyph_never_straddles_the_row_boundary() {
        // Width 3 cannot hold two two-cell glyphs.
        assert_eq!(painted("界界", 3), ["界", "界"]);
        assert_eq!(painted("界界", 5), ["界界"]);
        assert_eq!(painted("a界b", 2), ["a", "界", "b"]);
    }

    #[test]
    fn a_control_character_is_one_cell_and_paints_as_a_space() {
        assert_eq!(glyph_width('\t'), 1);
        assert_eq!(glyph_symbol('\t'), ' ');
        assert_eq!(glyph_width('\u{7}'), 1);
        assert_eq!(glyph_symbol('a'), 'a');
        // A tab costs one cell in the layout, so the row after it stays aligned
        // with what the renderer paints — and being whitespace, it is also a
        // break the wrap can back up to.
        assert_eq!(painted("a\tbcd", 3), ["a\t", "bcd"]);
    }

    #[test]
    fn every_cursor_round_trips_between_its_byte_and_its_cell() {
        // A fixture with everything that has ever desynchronised the two halves:
        // a wide glyph, a combining mark, a tab, and an unbreakable word.
        let text = "hi 界a\u{301}\tworld\nabcdefghijklmnopqrst\n";
        for width in [1u16, 3, 8, 40] {
            let rows = wrap_rows(text, width);
            for cursor in (0..=text.len()).filter(|at| text.is_char_boundary(*at)) {
                let (row, column) = caret_cell(text, cursor, width);
                // One position has no cell of its own: the end of a row that is
                // exactly full *and* ends at a newline. Its caret overflows onto
                // the row below, which already belongs to the next logical line,
                // so the two share a cell and only the later one maps back. Every
                // other position round-trips.
                let index = rows
                    .iter()
                    .rposition(|row| row.start <= cursor)
                    .unwrap_or_default();
                let overflowed_a_line_end = usize::from(row) > index
                    && rows
                        .get(index + 1)
                        .is_some_and(|next| next.start > rows.get(index).map_or(0, |row| row.end));
                if overflowed_a_line_end {
                    continue;
                }
                let back = byte_at_row_col(text, usize::from(row), usize::from(column), width);
                assert_eq!(back, cursor, "width {width}, cursor {cursor}");
            }
        }
    }

    #[test]
    fn cursor_row_wraps_on_display_cells_and_reserves_the_caret_cell() {
        assert_eq!(cursor_row("subject\nbody", 8, 40), 1);
        // A cursor at the end of an exactly-full row: the caret is pushed down.
        assert_eq!(cursor_row("abcdefghij", 10, 5), 2);
        assert_eq!(cursor_row("abcdefghij", 9, 5), 1);
        assert_eq!(cursor_row("", 0, 5), 0);
        // Wide glyphs consume two cells, so they wrap twice as fast.
        assert_eq!(cursor_row("界界", 6, 3), 1);
        assert_eq!(cursor_row("界界", 6, 5), 0);
        // A zero width never stalls the walk.
        assert_eq!(cursor_row("abc", 3, 0), 3);
    }
    #[test]
    fn cursor_row_charges_a_full_line_only_the_rows_it_fills() {
        // Regression. The old floor-division model summed `segment.width() /
        // width` over every `\n`-segment of the prefix and added the newline
        // count, which over-counts by one for every *completed* preceding line
        // whose display width is a positive exact multiple of `width`: such a
        // line consumes `floor((w - 1) / width)` extra rows, not `floor(w /
        // width)`. It returned 2 and 4 for the first two cases below, so the
        // caret follow-scrolled one row too far — a commit panel of inner width
        // 50 with a 50-column subject line scrolled past the caret. The values
        // pinned here are the rows ratatui actually paints, and the ones
        // `byte_at_row_col`/`place_cursor` already mapped back.
        assert_eq!(cursor_row("subject\nbody", 12, 7), 1); // old: 2
        assert_eq!(cursor_row("aaaaa\nbbbbb\ncc", 14, 5), 2); // old: 4
        assert_eq!(cursor_row("abcdefghij\nxy", 13, 5), 2); // old: 3
        assert_eq!(cursor_row("abcde\nx", 7, 5), 1); // old: 2
        assert_eq!(cursor_row("abcd\n", 5, 4), 1); // old: 2
    }
    #[test]
    fn cursor_row_tolerates_a_cursor_off_a_char_boundary() {
        // Byte 2 is interior to the two-byte "é"; slicing the prefix panicked.
        assert_eq!(cursor_row("héllo", 2, 10), 0);
        // An interior offset reports the next boundary's row, not its own.
        let text = "hé\nllo";
        assert_eq!(cursor_row(text, 2, 10), cursor_row(text, 3, 10));
        assert_eq!(cursor_row("héllo", 99, 10), 0, "past the end walks it all");
    }
    #[test]
    fn hit_testing_maps_rows_and_wide_characters_back_to_byte_offsets() {
        let text = "subject\nbody";
        assert_eq!(byte_at_row_col(text, 0, 3, 20), 3);
        assert_eq!(byte_at_row_col(text, 0, 40, 20), 7, "past the line end");
        assert_eq!(byte_at_row_col(text, 1, 2, 20), 10);
        assert_eq!(byte_at_row_col(text, 9, 0, 20), text.len(), "past the end");
        // "a界b": the wide glyph covers cells 1 and 2.
        assert_eq!(byte_at_row_col("a界b", 0, 0, 10), 0);
        assert_eq!(byte_at_row_col("a界b", 0, 1, 10), 1);
        assert_eq!(byte_at_row_col("a界b", 0, 2, 10), 1);
        assert_eq!(byte_at_row_col("a界b", 0, 3, 10), 4);
        // Width 2 cannot fit "a界" — the wide glyph starts row 1.
        assert_eq!(byte_at_row_col("a界b", 1, 0, 2), 1);
    }
}
