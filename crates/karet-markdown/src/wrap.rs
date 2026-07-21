//! Soft-wrapping the render model into painted lines.
//!
//! The output is renderer-agnostic: each line is a run of [`TextSpan`]s carrying a
//! semantic [`TokenId`], which a consumer resolves to a color (and bold/italic) through
//! `karet-theme`. Widths are measured in terminal columns, not bytes or `char`s.

#[cfg(test)]
mod tests;

use karet_core::StandardToken;
use karet_core::TokenId;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use crate::Alignment;
use crate::Block;
use crate::Inline;
use crate::ListItem;
use crate::MarkdownDocument;
use crate::Row;

/// A styled run of text within a wrapped line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSpan {
    /// The text of the run.
    pub text: String,
    /// The semantic class to color it with, or `None` for default foreground.
    pub token: Option<TokenId>,
    /// Hyperlink target carried by this run, or `None` for ordinary text.
    ///
    /// The target remains renderer-neutral. A presentation layer decides whether and
    /// how to expose or activate it after applying its own trust policy.
    pub link: Option<String>,
}

/// One wrapped, painted line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrappedLine {
    /// The styled runs, left to right.
    pub spans: Vec<TextSpan>,
}

impl WrappedLine {
    /// The line's plain text, with styling discarded.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// The line's display width in terminal columns.
    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }
}

/// Ties a source line to the wrapped line it produced.
///
/// One anchor is emitted per *top-level* block, at the block's first wrapped line. Lines
/// inside a block (a soft-wrapped paragraph, the body of a code fence) are not anchored
/// individually; [`WrappedDocument`]'s projections interpolate between anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// The 0-based line in the markdown source.
    pub source_line: usize,
    /// The 0-based index into [`WrappedDocument::lines`].
    pub wrapped_line: usize,
}

/// A width-wrapped document, ready to be painted line by line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrappedDocument {
    /// The painted lines, top to bottom.
    pub lines: Vec<WrappedLine>,
    /// Source-line anchors, one per top-level block, ascending on both axes.
    pub anchors: Vec<Anchor>,
}

impl WrappedDocument {
    /// The wrapped line that best corresponds to 0-based `source_line`.
    ///
    /// Clamped to the last line; `0` for an empty document.
    #[must_use]
    pub fn wrapped_line_for_source(&self, source_line: usize) -> usize {
        project(
            &self.anchors,
            source_line,
            |a| a.source_line,
            |a| a.wrapped_line,
            self.lines.len().saturating_sub(1),
        )
    }

    /// The 0-based source line that best corresponds to `wrapped_line`.
    ///
    /// Unclamped at the top end — the source's length is not known here, so a caller that
    /// needs a valid line index clamps it against its own buffer.
    #[must_use]
    pub fn source_line_for_wrapped(&self, wrapped_line: usize) -> usize {
        project(
            &self.anchors,
            wrapped_line,
            |a| a.wrapped_line,
            |a| a.source_line,
            usize::MAX,
        )
    }
}

/// Project `input` from one anchor axis onto the other, interpolating proportionally
/// between the two anchors that bracket it, and clamping the result to `limit`.
///
/// Total by construction: an empty `anchors`, or an `input` below the first anchor,
/// projects to `0`; past the last anchor the final block extends one-for-one.
fn project(
    anchors: &[Anchor],
    input: usize,
    from: impl Fn(&Anchor) -> usize,
    to: impl Fn(&Anchor) -> usize,
    limit: usize,
) -> usize {
    let above = anchors.partition_point(|a| from(a) <= input);
    let Some(lo) = above.checked_sub(1).and_then(|i| anchors.get(i)) else {
        return 0;
    };
    let offset = input.saturating_sub(from(lo));
    let projected = match anchors.get(above) {
        // Bracketed by two anchors: scale the offset by the ratio of the two spans. The
        // `max(1)` divisor is unreachable (anchors are strictly ascending on the source
        // axis) but keeps the division total.
        Some(hi) => {
            let span_from = from(hi).saturating_sub(from(lo)).max(1);
            let span_to = to(hi).saturating_sub(to(lo));
            to(lo).saturating_add(offset.saturating_mul(span_to) / span_from)
        },
        // Past the last anchor: no `hi` to scale against, so extend one-for-one.
        None => to(lo).saturating_add(offset),
    };
    projected.min(limit)
}

/// The bullet used for unordered list items.
const BULLET: &str = "• ";
/// The rule drawn for a thematic break, and for a table's horizontal borders.
const RULE: char = '─';
/// The vertical border between a table's columns.
const BAR: char = '│';
/// The gutter drawn to the left of a block quote.
const QUOTE_GUTTER: &str = "▌ ";

/// Wrap `doc` to `width` terminal columns.
pub(crate) fn wrap(doc: &MarkdownDocument, width: u16) -> WrappedDocument {
    // A zero-width viewport would make every wrap loop spin; one column always
    // terminates, and the caller sees (unhelpful but finite) output.
    let width = usize::from(width).max(1);
    let mut lines = Vec::new();
    let mut anchors = Vec::new();
    // The top level is wrapped here rather than through `wrap_blocks` so each block can
    // be anchored to its source line. The anchor is stamped *after* the separator, so it
    // points at the block's first real line rather than the blank one above it.
    for (index, block) in doc.blocks.iter().enumerate() {
        if index > 0 {
            lines.push(WrappedLine::default());
        }
        if let Some(source_line) = doc.block_line(index) {
            anchors.push(Anchor {
                source_line,
                wrapped_line: lines.len(),
            });
        }
        wrap_block(block, width, &[], &mut lines);
    }
    // A trailing blank line is an artifact of the between-blocks separator.
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    WrappedDocument { lines, anchors }
}

/// Wrap a sequence of blocks, each prefixed by `prefix` (a quote gutter, a list
/// indent), separated by a blank line.
fn wrap_blocks(blocks: &[Block], width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    for (index, block) in blocks.iter().enumerate() {
        // A nested list hugs the item that introduces it: a blank line between `- one`
        // and its sub-list would read as a break between two unrelated lists.
        let separated = index > 0 && !matches!(block, Block::List { .. });
        if separated {
            out.push(prefixed_line(prefix, Vec::new()));
        }
        wrap_block(block, width, prefix, out);
    }
}

fn wrap_block(block: &Block, width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    let indent = prefix_width(prefix);
    let inner = width.saturating_sub(indent).max(1);

    match block {
        Block::Heading { level, content } => {
            let marker = TextSpan {
                text: format!("{} ", "#".repeat(usize::from(*level).min(6))),
                token: Some(StandardToken::MarkupHeading.id()),
                link: None,
            };
            let mut runs = vec![marker];
            flatten(content, Some(StandardToken::MarkupHeading.id()), &mut runs);
            wrap_runs(&runs, inner, prefix, out);
        },
        Block::Paragraph(content) => {
            let mut runs = Vec::new();
            flatten(content, None, &mut runs);
            wrap_runs(&runs, inner, prefix, out);
        },
        Block::CodeBlock { lang, code } => {
            for line in code_lines(lang.as_deref(), code) {
                out.push(prefixed_line(prefix, line));
            }
        },
        Block::List { start, items } => {
            let markers = list_markers(*start, items);
            // Every marker is padded to the widest, so `9.` and `10.` share one text
            // column and the items' content lines up.
            let marker_width = markers.iter().map(|m| m.width()).max().unwrap_or(0);
            for (item, marker) in items.iter().zip(&markers) {
                // The marker occupies the first line; continuation lines align under it.
                let mut marked = prefix.to_vec();
                marked.push(TextSpan {
                    text: marker.clone() + &" ".repeat(marker_width - marker.width()),
                    token: Some(StandardToken::MarkupListMarker.id()),
                    link: None,
                });
                let mut continuation = prefix.to_vec();
                continuation.push(TextSpan {
                    text: " ".repeat(marker_width),
                    token: None,
                    link: None,
                });

                let first_line = out.len();
                wrap_blocks(&item.blocks, width, &continuation, out);
                // Swap the continuation indent on the item's first line for the marker.
                if let Some(first) = out.get_mut(first_line) {
                    replace_prefix(first, prefix.len(), &marked);
                }
            }
        },
        Block::Quote(blocks) => {
            let mut gutter = prefix.to_vec();
            gutter.push(TextSpan {
                text: QUOTE_GUTTER.to_owned(),
                token: Some(StandardToken::MarkupQuote.id()),
                link: None,
            });
            wrap_blocks(blocks, width, &gutter, out);
        },
        Block::Table {
            header,
            alignments,
            rows,
        } => wrap_table(header, alignments, rows, inner, prefix, out),
        Block::Rule => out.push(prefixed_line(
            prefix,
            vec![TextSpan {
                text: RULE.to_string().repeat(inner),
                token: Some(StandardToken::MarkupListMarker.id()),
                link: None,
            }],
        )),
    }
}

/// Every cell of `row`, flattened to styled runs and padded out to `columns` cells.
///
/// `token` seeds the flatten, so a header row can render bold without overriding the
/// tokens an inline sets for itself (a code span stays raw).
fn row_runs(row: &Row, columns: usize, token: Option<TokenId>) -> Vec<Vec<TextSpan>> {
    let mut cells: Vec<Vec<TextSpan>> = row
        .iter()
        .map(|cell| {
            let mut runs = Vec::new();
            flatten(cell, token, &mut runs);
            runs
        })
        .collect();
    cells.resize_with(columns, Vec::new);
    cells
}

/// The single-line display width of a cell's runs.
fn runs_width(runs: &[TextSpan]) -> usize {
    runs.iter().map(|s| s.text.width()).sum()
}

/// The total width of `natural` once no column exceeds `cap`.
fn capped_total(natural: &[usize], cap: usize) -> usize {
    natural.iter().map(|&n| n.min(cap)).sum()
}

/// The content width of each column, given `width` columns for the whole table.
///
/// Every column gets at least one cell column, so a table always renders — a viewport too
/// narrow to hold the grid overflows rather than collapsing. Where the natural widths do
/// not fit, the *widest* columns shrink first (a cap is lowered until the row fits), so a
/// narrow column keeps its content intact instead of being squeezed alongside a prose
/// column that dwarfs it.
fn column_widths<'a>(
    rows: impl Iterator<Item = &'a [Vec<TextSpan>]>,
    columns: usize,
    width: usize,
) -> Vec<usize> {
    let mut natural = vec![1usize; columns];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            if let Some(slot) = natural.get_mut(column) {
                *slot = (*slot).max(runs_width(cell));
            }
        }
    }
    // Chrome per column: a left border and a space either side of the content, plus the
    // table's closing border.
    let chrome = columns.saturating_mul(3).saturating_add(1);
    let budget = width.saturating_sub(chrome).max(columns);
    if capped_total(&natural, usize::MAX) <= budget {
        return natural;
    }

    // The largest per-column cap that fits. A cap of 1 always fits (`budget >= columns`),
    // so the search has a feasible floor to fall back on.
    let mut low = 1usize;
    let mut high = natural.iter().copied().max().unwrap_or(1);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if capped_total(&natural, mid) <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    let mut widths: Vec<usize> = natural.iter().map(|&n| n.min(low)).collect();
    // The cap leaves the budget under-spent by less than one column's worth; hand the
    // remainder to the capped columns so the grid fills the viewport.
    let mut spare = budget.saturating_sub(widths.iter().sum());
    for (width, natural) in widths.iter_mut().zip(&natural) {
        if spare == 0 {
            break;
        }
        if *width < *natural {
            *width += 1;
            spare -= 1;
        }
    }
    widths
}

/// A horizontal table border: `left`, then a `RULE` run per column, joined by `mid`.
fn table_border(widths: &[usize], left: char, mid: char, right: char) -> Vec<TextSpan> {
    let mut text = left.to_string();
    for (index, &width) in widths.iter().enumerate() {
        if index > 0 {
            text.push(mid);
        }
        // The content width plus the space padding either side of it.
        text.extend(std::iter::repeat_n(RULE, width.saturating_add(2)));
    }
    text.push(right);
    vec![TextSpan {
        text,
        token: Some(StandardToken::MarkupListMarker.id()),
        link: None,
    }]
}

/// The padding to either side of a cell line `extra` columns narrower than its column.
fn cell_padding(alignment: Alignment, extra: usize) -> (usize, usize) {
    match alignment {
        Alignment::None | Alignment::Left => (0, extra),
        Alignment::Center => (extra / 2, extra - extra / 2),
        Alignment::Right => (extra, 0),
    }
}

/// Paint one table row, soft-wrapping each cell inside its column. A row is as tall as
/// its tallest cell; the shorter cells are blank-padded to match.
fn table_row(
    cells: &[Vec<TextSpan>],
    widths: &[usize],
    alignments: &[Alignment],
    prefix: &[TextSpan],
    out: &mut Vec<WrappedLine>,
) {
    let wrapped: Vec<Vec<WrappedLine>> = cells
        .iter()
        .zip(widths)
        .map(|(runs, &width)| {
            let mut lines = Vec::new();
            wrap_cell(runs, width, &mut lines);
            lines
        })
        .collect();
    let height = wrapped.iter().map(Vec::len).max().unwrap_or(0);

    for row in 0..height {
        let mut spans = vec![bar()];
        for (column, &width) in widths.iter().enumerate() {
            let line = wrapped.get(column).and_then(|lines| lines.get(row));
            let used = line.map_or(0, WrappedLine::width);
            let alignment = alignments.get(column).copied().unwrap_or_default();
            let (left, right) = cell_padding(alignment, width.saturating_sub(used));
            spans.push(space(left.saturating_add(1)));
            if let Some(line) = line {
                spans.extend(line.spans.iter().cloned());
            }
            spans.push(space(right.saturating_add(1)));
            spans.push(bar());
        }
        out.push(prefixed_line(prefix, spans));
    }
}

/// A vertical table border.
fn bar() -> TextSpan {
    TextSpan {
        text: BAR.to_string(),
        token: Some(StandardToken::MarkupListMarker.id()),
        link: None,
    }
}

/// An unstyled run of `count` spaces.
fn space(count: usize) -> TextSpan {
    TextSpan {
        text: " ".repeat(count),
        token: None,
        link: None,
    }
}

/// Paint a table as a box-drawn grid, including a horizontal rule between each row.
fn wrap_table(
    header: &Row,
    alignments: &[Alignment],
    rows: &[Row],
    width: usize,
    prefix: &[TextSpan],
    out: &mut Vec<WrappedLine>,
) {
    let columns = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0));
    if columns == 0 {
        return; // a table with no columns has nothing to draw
    }
    // A header cell renders bold unless one of its inlines claims a token of its own.
    let header_cells = row_runs(header, columns, Some(StandardToken::MarkupBold.id()));
    let body_cells: Vec<Vec<Vec<TextSpan>>> = rows
        .iter()
        .map(|row| row_runs(row, columns, None))
        .collect();

    let measured =
        std::iter::once(header_cells.as_slice()).chain(body_cells.iter().map(Vec::as_slice));
    let widths = column_widths(measured, columns, width);

    out.push(prefixed_line(prefix, table_border(&widths, '┌', '┬', '┐')));
    table_row(&header_cells, &widths, alignments, prefix, out);
    out.push(prefixed_line(prefix, table_border(&widths, '├', '┼', '┤')));
    for (index, row) in body_cells.iter().enumerate() {
        table_row(row, &widths, alignments, prefix, out);
        if index + 1 < body_cells.len() {
            out.push(prefixed_line(prefix, table_border(&widths, '├', '┼', '┤')));
        }
    }
    out.push(prefixed_line(prefix, table_border(&widths, '└', '┴', '┘')));
}

/// The marker for each list item: `1. `, `2. `, … from `start` for an ordered list, or a
/// bullet for each item of an unordered one.
///
/// A task item's checkbox stands in for the bullet — the two mark the same thing, and
/// GitHub draws only the box. An *ordered* task item keeps its ordinal and puts the
/// checkbox after it, because the number carries meaning the box does not.
fn list_markers(start: Option<u64>, items: &[ListItem]) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| match (start, item.task) {
            (None, None) => BULLET.to_owned(),
            (None, Some(checked)) => crate::task_marker(checked).to_owned(),
            (Some(start), task) => {
                // A list long enough to overflow `u64` cannot be typed; saturating keeps
                // the arithmetic total, at the cost of repeating the final ordinal.
                let ordinal = start.saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
                let box_ = task.map(crate::task_marker).unwrap_or_default();
                format!("{ordinal}. {box_}")
            },
        })
        .collect()
}

/// Replace the first `old_len` spans of `line` with `new`.
fn replace_prefix(line: &mut WrappedLine, old_len: usize, new: &[TextSpan]) {
    if line.spans.len() < old_len + 1 {
        return;
    }
    line.spans.splice(..=old_len, new.iter().cloned());
}

/// The display width of a prefix.
fn prefix_width(prefix: &[TextSpan]) -> usize {
    prefix.iter().map(|s| s.text.width()).sum()
}

/// A line consisting of `prefix` followed by `spans`.
fn prefixed_line(prefix: &[TextSpan], spans: Vec<TextSpan>) -> WrappedLine {
    let mut all = prefix.to_vec();
    all.extend(spans);
    WrappedLine { spans: all }
}

/// Flatten inlines into styled runs, inheriting `token` where an inline sets none.
fn flatten(inlines: &[Inline], token: Option<TokenId>, out: &mut Vec<TextSpan>) {
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push(TextSpan {
                text: text.clone(),
                token,
                link: None,
            }),
            Inline::Code(text) => out.push(TextSpan {
                text: text.clone(),
                token: Some(StandardToken::MarkupRaw.id()),
                link: None,
            }),
            // Emphasis inside a heading stays a heading: the outer token wins, because a
            // theme colors headings as a unit.
            Inline::Emphasis(children) => flatten(
                children,
                token.or(Some(StandardToken::MarkupItalic.id())),
                out,
            ),
            Inline::Strong(children) => {
                flatten(
                    children,
                    token.or(Some(StandardToken::MarkupBold.id())),
                    out,
                );
            },
            Inline::Link { text, href } => out.push(TextSpan {
                text: text.clone(),
                token: Some(StandardToken::MarkupLink.id()),
                link: Some(href.clone()),
            }),
        }
    }
}

/// Greedily wrap `runs` to `width` columns, preserving each run's token.
///
/// Breaks at whitespace; a single word longer than the line is emitted whole and allowed
/// to overflow rather than being cut mid-word. An embedded `\n` (a hard break) ends the
/// line.
fn wrap_runs(runs: &[TextSpan], width: usize, prefix: &[TextSpan], out: &mut Vec<WrappedLine>) {
    wrap_runs_inner(runs, width, prefix, false, out);
}

/// As [`wrap_runs`], but breaking an over-long word across lines rather than letting it
/// overflow — a table cell must stay inside its column or the grid stops lining up.
fn wrap_cell(runs: &[TextSpan], width: usize, out: &mut Vec<WrappedLine>) {
    wrap_runs_inner(runs, width, &[], true, out);
}

/// Split `word` into chunks at most `width` columns wide.
///
/// Splits between `char`s, so a word made of multi-`char` grapheme clusters can be cut
/// mid-cluster; the alternative — overflowing the column — breaks the table grid, which is
/// the worse of the two. A single `char` wider than `width` is emitted alone.
fn split_to_width(word: &str, width: usize) -> Vec<&str> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut used = 0usize;
    for (index, ch) in word.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if used > 0 && used + ch_width > width {
            chunks.push(&word[start..index]);
            start = index;
            used = 0;
        }
        used += ch_width;
    }
    if start < word.len() {
        chunks.push(&word[start..]);
    }
    chunks
}

fn wrap_runs_inner(
    runs: &[TextSpan],
    width: usize,
    prefix: &[TextSpan],
    break_words: bool,
    out: &mut Vec<WrappedLine>,
) {
    let mut line: Vec<TextSpan> = Vec::new();
    let mut used = 0usize;

    // The space that separated the last word from the one pushed to the next line must
    // not linger at the end of this one.
    let flush = |line: &mut Vec<TextSpan>, used: &mut usize, out: &mut Vec<WrappedLine>| {
        trim_trailing_space(line);
        out.push(prefixed_line(prefix, std::mem::take(line)));
        *used = 0;
    };

    for run in runs {
        for (index, segment) in run.text.split('\n').enumerate() {
            if index > 0 {
                flush(&mut line, &mut used, out); // a hard break
            }
            for word in words(segment) {
                let is_space = word.chars().all(char::is_whitespace);
                let pieces = if break_words && !is_space && word.width() > width {
                    split_to_width(word, width)
                } else {
                    vec![word]
                };
                for piece in pieces {
                    let w = piece.width();
                    // Never start a line with the space that separated two words.
                    if used == 0 && is_space {
                        continue;
                    }
                    if used > 0 && used + w > width && !is_space {
                        flush(&mut line, &mut used, out);
                    }
                    // Re-check: a leading space can appear after the flush above.
                    if used == 0 && is_space {
                        continue;
                    }
                    push_word(&mut line, piece, run.token, run.link.as_deref());
                    used += w;
                }
            }
        }
    }
    if !line.is_empty() || out.is_empty() {
        flush(&mut line, &mut used, out);
    }
}

/// Drop trailing whitespace from a completed line, discarding runs it empties.
fn trim_trailing_space(line: &mut Vec<TextSpan>) {
    while let Some(last) = line.last_mut() {
        let trimmed = last.text.trim_end();
        if trimmed.len() == last.text.len() {
            return; // nothing to trim
        }
        last.text.truncate(trimmed.len());
        if !last.text.is_empty() {
            return;
        }
        line.pop();
    }
}

/// Append `word` to `line`, coalescing with a preceding run of the same token.
fn push_word(line: &mut Vec<TextSpan>, word: &str, token: Option<TokenId>, link: Option<&str>) {
    match line.last_mut() {
        Some(last) if last.token == token && last.link.as_deref() == link => {
            last.text.push_str(word);
        },
        _ => line.push(TextSpan {
            text: word.to_owned(),
            token,
            link: link.map(str::to_owned),
        }),
    }
}

/// Split `text` into alternating word and whitespace chunks, preserving both.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_space = None;
    for (index, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if in_space.is_some_and(|s| s != space) {
            out.push(&text[start..index]);
            start = index;
        }
        in_space = Some(space);
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The painted lines of a code block: highlighted when a grammar is available for
/// `lang`, otherwise a single raw-markup run per line.
fn code_lines(lang: Option<&str>, code: &str) -> Vec<Vec<TextSpan>> {
    #[cfg(feature = "highlight")]
    if let Some(lines) = crate::highlight::code_lines(lang, code) {
        return lines;
    }
    let _ = lang;
    code.lines()
        .map(|line| {
            vec![TextSpan {
                text: line.to_owned(),
                token: Some(StandardToken::MarkupRaw.id()),
                link: None,
            }]
        })
        .collect()
}
