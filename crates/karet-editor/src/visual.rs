use std::ops::RangeInclusive;

use karet_core::LineCol;
use karet_text::TextBuffer;
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;

use super::hint::BlockIndex;
use super::hint::HintIndex;
use super::hint::LineHints;
use super::text::*;
use crate::EditorState;
use crate::Fold;
use crate::cluster_bounds;
use crate::view::gutter_width;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct VisualAnchor {
    pub(super) line: u32,
    pub(super) subrow: u32,
}

/// One visual row of a line: the buffer columns `start..end` it shows, and
/// the blank cells painted ahead of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisualRange {
    pub(super) start: u32,
    pub(super) end: u32,
    /// Cells left blank before the row's first column: the
    /// [`WrappingIndent`] of a soft-wrapped continuation row, else 0.
    ///
    /// Painting, caret placement, hit-testing and vertical motion all add
    /// it to (or take it from) [`offset_within_row`], so the four agree on
    /// which cell shows which column.
    pub(super) indent: u32,
}

impl VisualRange {
    pub(super) const fn empty(at: u32) -> Self {
        Self::new(at, at)
    }

    /// An unindented row showing `start..end`.
    pub(super) const fn new(start: u32, end: u32) -> Self {
        Self {
            start,
            end,
            indent: 0,
        }
    }
}

/// How a soft-wrapped line's continuation rows are indented.
///
/// The indent is capped at half the wrap width, so a deeply indented line
/// in a narrow pane still leaves its continuation rows room for text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrappingIndent {
    /// Continuation rows start at the left edge of the text area.
    #[default]
    None,
    /// Continuation rows line up with the line's own leading whitespace.
    Same,
    /// Continuation rows sit one tab stop deeper than the line's leading
    /// whitespace.
    Indent,
}

/// Everything that decides where a buffer position lands on screen.
///
/// Bundled rather than passed as four parallel arguments because inlay hints
/// had to reach every one of these functions, and several were already at the
/// argument limit. It also removes the `too_many_arguments` allow that
/// [`reveal_visual_anchor`] used to need.
#[derive(Clone, Copy)]
pub(super) struct Layout<'a> {
    /// Content width available for wrapping.
    pub(super) width: u32,
    /// Columns between hard-tab stops.
    pub(super) tab_width: u16,
    /// Lines kept on one visual row regardless of width.
    pub(super) unwrapped_lines: &'a [RangeInclusive<u32>],
    /// Virtual text occupying cells no buffer column owns.
    pub(super) hints: &'a HintIndex,
    /// Virtual lines taking whole rows above and below their buffer lines.
    pub(super) blocks: &'a BlockIndex,
    /// Whether lines soft-wrap at `width`; unwrapped, every line has one
    /// text row.
    pub(super) wrap: bool,
    /// The column soft wrapping breaks at when it is narrower than `width`;
    /// `None` (or 0) wraps at `width`.
    pub(super) wrap_column: Option<u32>,
    /// How a wrapped line's continuation rows are indented.
    pub(super) wrapping_indent: WrappingIndent,
}

/// What one visual row of a shown line holds.
///
/// A line's rows run: its virtual lines above, top first, then its text
/// rows, then its virtual lines below. A [`VisualAnchor`]'s `subrow` counts
/// all of them from the first row above, so scrolling, reveal and
/// hit-testing walk virtual rows exactly as they walk wrapped ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowKind {
    /// The `n`th virtual line above the line.
    Above(u32),
    /// The line's `n`th text row.
    Text(u32),
    /// The `n`th virtual line below the line.
    Below(u32),
}

impl<'a> Layout<'a> {
    /// The hints on `line`.
    pub(super) fn hints_on(&self, line: u32) -> LineHints<'a> {
        self.hints.line(line)
    }

    /// How many virtual rows sit above `line`.
    pub(super) fn above(&self, line: u32) -> u32 {
        self.blocks.above_count(line)
    }

    /// What row `subrow` of `line` holds, when the line has `text` text
    /// rows; past its last row, its last row.
    pub(super) fn kind(&self, line: u32, subrow: u32, text: u32) -> RowKind {
        let above = self.above(line);
        let text = text.max(1);
        if subrow < above {
            RowKind::Above(subrow)
        } else if subrow - above < text {
            RowKind::Text(subrow - above)
        } else {
            let below = self.blocks.below_count(line);
            RowKind::Below((subrow - above - text).min(below.saturating_sub(1)))
        }
    }

    /// The cells a soft-wrapped row may fill: `width`, narrowed to
    /// [`wrap_column`](Self::wrap_column) when that is smaller. A soft-wrapped
    /// view has no horizontal scroll, so a column wider than the viewport
    /// wraps at the viewport rather than clipping text off its right edge.
    pub(super) fn wrap_width(&self) -> u32 {
        self.wrap_column
            .filter(|&column| column > 0)
            .map_or(self.width, |column| column.min(self.width))
            .max(1)
    }

    /// The text rows of `line` as a frame paints them: its soft-wrapped
    /// rows, or, unwrapped, its one row scrolled to `scroll_col`.
    pub(super) fn text_rows(
        &self,
        buffer: &TextBuffer,
        line: u32,
        scroll_col: u32,
    ) -> Vec<VisualRange> {
        if self.wrap {
            visual_ranges(buffer, line, *self)
        } else {
            vec![VisualRange::new(scroll_col, u32::MAX)]
        }
    }
}

/// The blank cells ahead of each continuation row of `chars` wrapped at
/// `width`, as [`Layout::wrapping_indent`] asks: the line's leading
/// whitespace with its tabs expanded, one tab stop deeper for
/// [`WrappingIndent::Indent`], and never more than half of `width`, so a
/// continuation row keeps room for its text.
fn continuation_indent(chars: &[char], width: u32, layout: Layout<'_>) -> u32 {
    let extra = match layout.wrapping_indent {
        WrappingIndent::None => return 0,
        WrappingIndent::Same => 0,
        WrappingIndent::Indent => u32::from(layout.tab_width.max(1)),
    };
    let leading = chars
        .iter()
        .take_while(|&&ch| ch == ' ' || ch == '\t')
        .fold(0_u32, |col, &ch| {
            col.saturating_add(character_width(ch, 1, col, layout.tab_width))
        });
    leading.saturating_add(extra).min(width / 2)
}

/// Every row `line` takes: its text rows and its virtual lines.
fn line_rows(buffer: &TextBuffer, line: u32, layout: Layout<'_>) -> u32 {
    let text = visual_ranges(buffer, line, layout).len().max(1) as u32;
    layout
        .above(line)
        .saturating_add(text)
        .saturating_add(layout.blocks.below_count(line))
}

/// The screen cells `ch` occupies when it starts at `display_col`.
///
/// `cells` is the char's entry in [`cluster_widths`]: a hard tab ignores it and
/// expands to the next tab stop from `display_col`.
pub(super) fn character_width(ch: char, cells: u32, display_col: u32, tab_width: u16) -> u32 {
    if ch == '\t' {
        let width = u32::from(tab_width.max(1));
        width - (display_col % width)
    } else {
        cells
    }
}

/// The cells each char of `chars` occupies, tabs aside: an extended grapheme
/// cluster's width sits on its first char and every later char of the cluster
/// takes none. A terminal lays a cluster out as one glyph, so summing its chars
/// would over-count a ZWJ emoji sequence (`👨‍👩‍👧` is two cells, not six) or an
/// emoji presentation sequence. A one-char cluster keeps the char's own width.
/// A hard tab reports 0 here: its cells depend on where it starts.
#[must_use]
pub fn cluster_widths(chars: &[char]) -> Vec<u32> {
    let text: String = chars.iter().collect();
    let mut widths = Vec::with_capacity(chars.len());
    for cluster in text.graphemes(true) {
        let mut members = cluster.chars();
        let width = match (members.next(), members.next()) {
            (Some(ch), None) => unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0),
            _ => unicode_width::UnicodeWidthStr::width(cluster),
        };
        widths.push(u32::try_from(width).unwrap_or(u32::MAX));
        widths.extend(cluster.chars().skip(1).map(|_| 0));
    }
    widths
}

/// The cells the char at `index` of `chars` occupies at `display_col`, given
/// the line's [`cluster_widths`].
fn width_at(chars: &[char], widths: &[u32], index: u32, display_col: u32, tab_width: u16) -> u32 {
    chars.get(index as usize).map_or(0, |&ch| {
        let cells = widths.get(index as usize).copied().unwrap_or(0);
        character_width(ch, cells, display_col, tab_width)
    })
}

/// The screen column a buffer column renders at.
///
/// Characters *and* hints strictly before `source_col`: a hint anchored at `c`
/// renders between the caret slot of `c` and the character there, so a caret
/// at `c` sits *before* it, adjacent to the text it edits. Typing at the end of
/// `count` in `let count: i32 = …` therefore inserts exactly where the caret
/// is drawn, left of the annotation. A tab's width is measured against the
/// running screen column, hints included, because a tab stops at a real
/// screen position.
pub(super) fn display_col(
    chars: &[char],
    source_col: u32,
    tab_width: u16,
    hints: LineHints<'_>,
) -> u32 {
    let widths = cluster_widths(chars);
    // Clamped to the line: past its end there are no characters left to walk,
    // so a hint anchored beyond it cannot shift anything.
    let limit = source_col.min(chars.len() as u32);
    let mut col = 0_u32;
    for index in 0..limit {
        col = col.saturating_add(hints.width_at(index));
        col = col.saturating_add(width_at(chars, &widths, index, col, tab_width));
    }
    col
}

/// How a cell inside a multi-cell character resolves to a buffer column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Snap {
    /// The nearer of the character's two boundaries: where a click puts the
    /// caret.
    Nearest,
    /// The character's own column: which character the pointer is over, for
    /// word selection and anything else that asks what is under the pointer.
    Containing,
}

/// The buffer column `offset` screen cells into the row starting at `start`.
///
/// `offset` is measured from the row's own first cell — the same origin
/// [`offset_within_row`] produces and the same one a mouse click arrives in —
/// so the two are exact inverses. The row begins at `display_col(start)`,
/// which is also where the hint anchored at `start` paints: that hint belongs
/// to the row that starts at its column.
///
/// A cell inside a character resolves as `snap` says: [`Snap::Nearest`] to
/// the nearer of its two boundaries, so the right half of a wide character,
/// or the trailing cells of a tab, put the caret after it; [`Snap::Containing`]
/// to the character's own column. A cell inside a hint resolves to the hint's
/// own anchor column, where the caret is drawn immediately left of the
/// annotation. Clicking an annotation therefore puts the caret against the
/// text the hint describes rather than past the character that follows it.
pub(super) fn source_col_at_display_offset(
    chars: &[char],
    start: u32,
    end: u32,
    offset: u32,
    tab_width: u16,
    hints: LineHints<'_>,
    snap: Snap,
) -> u32 {
    let len = chars.len() as u32;
    let mut source = start.min(len);
    let end = end.min(len);
    let mut absolute = display_col(chars, source, tab_width, hints);
    let target = absolute.saturating_add(offset);
    let widths = cluster_widths(chars);
    let bounds = cluster_bounds(chars);
    // Walk whole grapheme clusters: a column inside one is never a caret
    // position, so a cell resolves to a cluster's start or its end.
    while source < end {
        let next = bounds
            .iter()
            .copied()
            .find(|&bound| bound > source)
            .unwrap_or(len);
        // The hint anchored at `source` paints between its caret slot and the
        // character there, so its cells belong to this column.
        let after_hint = absolute.saturating_add(hints.width_at(source));
        if target < after_hint {
            return source;
        }
        let width = width_at(chars, &widths, source, after_hint, tab_width);
        if target < after_hint.saturating_add(width) {
            // Round to the nearer boundary: the right half of a wide
            // character, or a tab's trailing cells, put the caret after it.
            if snap == Snap::Nearest && target.saturating_sub(after_hint).saturating_mul(2) >= width
            {
                source = next;
            }
            break;
        }
        // A cluster's later chars take no cells of their own; a hint anchored
        // between them still does.
        absolute = (source.saturating_add(1)..next)
            .fold(after_hint.saturating_add(width), |col, inner| {
                col.saturating_add(hints.width_at(inner))
            });
        source = next;
    }
    source
}

pub(super) fn visual_ranges(
    buffer: &TextBuffer,
    line: u32,
    layout: Layout<'_>,
) -> Vec<VisualRange> {
    // Checked before the line is collected: an unwrapped view walks rows
    // through here once per line.
    if !layout.wrap
        || layout
            .unwrapped_lines
            .iter()
            .any(|range| range.contains(&line))
    {
        return vec![VisualRange::new(0, line_len(buffer, line))];
    }
    let chars: Vec<char> = buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let len = chars.len() as u32;
    if len == 0 {
        return vec![VisualRange::empty(0)];
    }
    let hints = layout.hints_on(line);
    let full = layout.wrap_width();
    let indent = continuation_indent(&chars, full, layout);
    let widths = cluster_widths(&chars);
    // Rows break between grapheme clusters only, so a wrap never splits a
    // combining sequence, a ZWJ emoji or a flag across two rows.
    let runs = clusters(&chars);
    let mut ranges = Vec::new();
    let mut first = 0_usize;
    while let Some(&(start, ..)) = runs.get(first) {
        // A continuation row gives its indent's cells to blanks; the indent
        // is at most half of `full`, so at least one cell is left.
        let row_indent = if ranges.is_empty() { 0 } else { indent };
        let width = full.saturating_sub(row_indent).max(1);
        let mut next = first;
        let mut used = 0_u32;
        // `display_col(start)` stops short of the hint at `start`: this row
        // paints it, so its cells are counted below.
        let mut absolute = display_col(&chars, start, layout.tab_width, hints);
        while let Some(&(from, to, _)) = runs.get(next) {
            let step = (from..to).fold(0_u32, |step, index| {
                let hint = hints.width_at(index);
                let at = absolute.saturating_add(step).saturating_add(hint);
                let cells = width_at(&chars, &widths, index, at, layout.tab_width);
                step.saturating_add(hint).saturating_add(cells)
            });
            if next > first && used.saturating_add(step) > width {
                break;
            }
            used = used.saturating_add(step);
            absolute = absolute.saturating_add(step);
            next += 1;
            if used >= width {
                break;
            }
        }
        let row = runs.get(first..next).unwrap_or_default();
        let end = if next < runs.len() {
            row.iter()
                .rposition(|cluster| cluster.2.is_whitespace())
                .map_or(next, |index| first + index + 1)
        } else {
            next
        };
        let end = end.max(first + 1);
        ranges.push(VisualRange {
            start,
            end: runs.get(end).map_or(len, |cluster| cluster.0),
            indent: row_indent,
        });
        first = end;
    }
    ranges
}

pub(super) fn normalize_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    layout: Layout<'_>,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let last = last_line(buffer);
    let mut line = anchor.line.min(last);
    while line < last && hidden_in(folds, line) {
        line += 1;
    }
    while line > 0 && hidden_in(folds, line) {
        line -= 1;
    }
    let rows = line_rows(buffer, line, layout);
    VisualAnchor {
        line,
        subrow: anchor.subrow.min(rows - 1),
    }
}

pub(super) fn next_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    layout: Layout<'_>,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, layout, anchor);
    let rows = line_rows(buffer, anchor.line, layout);
    if anchor.subrow + 1 < rows {
        return VisualAnchor {
            subrow: anchor.subrow + 1,
            ..anchor
        };
    }
    let last = last_line(buffer);
    let mut line = anchor.line.saturating_add(1);
    while line <= last && hidden_in(folds, line) {
        line += 1;
    }
    if line > last {
        anchor
    } else {
        VisualAnchor { line, subrow: 0 }
    }
}

pub(super) fn previous_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    layout: Layout<'_>,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, layout, anchor);
    if anchor.subrow > 0 {
        return VisualAnchor {
            subrow: anchor.subrow - 1,
            ..anchor
        };
    }
    let mut line = anchor.line;
    while line > 0 {
        line -= 1;
        if !hidden_in(folds, line) {
            let rows = line_rows(buffer, line, layout);
            return VisualAnchor {
                line,
                subrow: rows - 1,
            };
        }
    }
    anchor
}

/// The visual row `row` rows below `start`, and whether `row` lies past the
/// document's last row — in which case the anchor is that last row.
pub(super) fn visual_anchor_at_row(
    buffer: &TextBuffer,
    folds: &[Fold],
    layout: Layout<'_>,
    start: VisualAnchor,
    row: u32,
) -> (VisualAnchor, bool) {
    let mut anchor = normalize_visual_anchor(buffer, folds, layout, start);
    for _ in 0..row {
        let next = next_visual_anchor(buffer, folds, layout, anchor);
        if next == anchor {
            return (anchor, true);
        }
        anchor = next;
    }
    (anchor, false)
}

/// Whether a caret at `col` is drawn on the visual row `range`.
///
/// A row owns the columns it starts through the one before its end. Its end
/// column is also where the next row starts, so the row draws it only when
/// it is the line's last row, or when the caret is `upstream`: placed by a
/// click past the end of this row rather than at the start of the next.
pub(super) fn row_holds(range: VisualRange, last: bool, col: u32, upstream: bool) -> bool {
    range.start <= col && (col < range.end || (col == range.end && (last || upstream)))
}

/// The visual row a caret at `pos` is drawn on; `upstream` as in
/// [`row_holds`]. Always a text row: no caret sits on a virtual line.
pub(super) fn visual_anchor_for_position(
    buffer: &TextBuffer,
    layout: Layout<'_>,
    pos: LineCol,
    upstream: bool,
) -> VisualAnchor {
    let line = pos.line.min(last_line(buffer));
    let ranges = visual_ranges(buffer, line, layout);
    let last = ranges.len().saturating_sub(1);
    let index = ranges
        .iter()
        .enumerate()
        .find_map(|(index, range)| {
            row_holds(*range, index == last, pos.col, upstream).then_some(index as u32)
        })
        .unwrap_or(last as u32);
    VisualAnchor {
        line,
        subrow: layout.above(line).saturating_add(index),
    }
}

/// The scroll margin a viewport of `height` rows can honour: `off` rows,
/// capped so the rows above and below the caret still fit (a margin of half
/// the viewport or more would pin the caret and scroll on every move).
pub(super) fn scroll_margin(off: u32, height: u32) -> u32 {
    off.min(height.max(1).saturating_sub(1) / 2)
}

/// The top row that shows the caret at `cursor` in a viewport of `height`
/// rows currently starting at `current`, keeping `off` rows (see
/// [`scroll_margin`]) between it and either edge. The margin stops at the
/// buffer's first and last rows, so it never scrolls past either end.
///
/// Rows are visual rows: wrapped rows and virtual lines each count. A caret
/// on its line's first text row also brings the virtual lines above that
/// line into view when scrolling up to it, as far as they fit, so a code
/// lens is not left just out of sight above the line it describes. A caret
/// a collapsed fold hides is revealed through the header that shows it.
#[allow(clippy::too_many_arguments)]
// The geometry, the viewport and the caret are each needed, and bundling
// them would add a type used once.
pub(super) fn reveal_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    layout: Layout<'_>,
    height: u16,
    current: VisualAnchor,
    cursor: LineCol,
    upstream: bool,
    off: u32,
) -> VisualAnchor {
    let current = normalize_visual_anchor(buffer, folds, layout, current);
    let header = shown_line(folds, cursor.line);
    let cursor = if header == cursor.line {
        cursor
    } else {
        LineCol::new(header, line_len(buffer, header))
    };
    let caret = visual_anchor_for_position(buffer, layout, cursor, upstream);
    let off = scroll_margin(off, u32::from(height));
    // The rows above the line, capped so the caret and the margin below it
    // still fit: otherwise a block taller than the viewport would push the
    // caret off the bottom.
    let lead = if caret.subrow == layout.above(caret.line) {
        caret
            .subrow
            .min(u32::from(height.max(1)).saturating_sub(1 + off))
    } else {
        0
    };
    let (mut upper, mut target) = (caret, caret);
    for _ in 0..off.max(lead) {
        upper = previous_visual_anchor(buffer, folds, layout, upper);
    }
    for _ in 0..off {
        target = next_visual_anchor(buffer, folds, layout, target);
    }
    if upper < current {
        return upper;
    }
    let mut probe = current;
    for _ in 0..height.max(1) {
        if probe == target {
            return current;
        }
        let next = next_visual_anchor(buffer, folds, layout, probe);
        if next == probe {
            break;
        }
        probe = next;
    }
    let mut revealed = target;
    for _ in 1..height.max(1) {
        let previous = previous_visual_anchor(buffer, folds, layout, revealed);
        if previous == revealed {
            break;
        }
        revealed = previous;
    }
    revealed
}

/// The screen offset of `col` within the row that begins at `row_start`.
///
/// The row's origin is `display_col(row_start)`, which excludes the hint
/// anchored at `row_start` because that hint paints inside this row, after the
/// caret slot of its anchor. So a caret at the row's first column sits at
/// offset 0, ahead of any leading hint.
pub(super) fn offset_within_row(
    chars: &[char],
    row_start: u32,
    col: u32,
    tab_width: u16,
    hints: LineHints<'_>,
) -> u32 {
    display_col(chars, col, tab_width, hints)
        .saturating_sub(display_col(chars, row_start, tab_width, hints))
}

/// [`display_col`] for every column `0..=len` of one line, in a single pass.
///
/// Non-decreasing, so the horizontal-scroll searches below can bisect it.
fn display_cols(chars: &[char], tab_width: u16, hints: LineHints<'_>) -> Vec<u32> {
    let widths = cluster_widths(chars);
    let mut cols = Vec::with_capacity(chars.len().saturating_add(1));
    let mut col = 0_u32;
    cols.push(col);
    for index in (0_u32..).take(chars.len()) {
        col = col.saturating_add(hints.width_at(index));
        col = col.saturating_add(width_at(chars, &widths, index, col, tab_width));
        cols.push(col);
    }
    cols
}

/// How far an unwrapped line reaches horizontally, in the units of
/// `EditorState::scroll_col`.
///
/// `scroll_col` is a *buffer* column — the column a scrolled row starts
/// painting at — while the viewport is `width` screen cells, and tabs, wide
/// characters and hints make the two diverge. The extent is therefore the
/// leftmost `scroll_col` that shows the line's last cell (a trailing hint, or
/// the end-of-line caret slot), plus `width - 1`, so that the usual clamp
/// `extent + 1 - width` is exactly the scroll that reveals the tail. A line
/// that fits reports its last cell instead; with no tabs, wide characters or
/// hints both cases reduce to the line's length in `char`s.
pub(super) fn horizontal_extent(
    chars: &[char],
    width: u32,
    tab_width: u16,
    hints: LineHints<'_>,
) -> u32 {
    let cols = display_cols(chars, tab_width, hints);
    let end = cols.last().copied().unwrap_or(0);
    let trailing = hints.width_at(chars.len() as u32);
    // The caret slot at end-of-line takes a cell even when no hint follows it.
    let needed = end.saturating_add(trailing.max(1));
    let width = width.max(1);
    let first = cols.partition_point(|&origin| needed.saturating_sub(origin) > width) as u32;
    if first == 0 {
        needed.saturating_sub(1)
    } else {
        first.saturating_add(width).saturating_sub(1)
    }
}

/// How much room a reveal leaves around the caret.
///
/// Who moved the caret decides it. A keyboard motion walks toward the edge, so
/// the view scrolls before the caret reaches it and the text ahead stays in
/// sight. A pointer placed the caret on a cell the user is looking at, so the
/// text under the pointer must not move: the view scrolls only when the target
/// is not on screen at all, and then only as far as it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reveal {
    /// Keep up to 10 cells either side of the caret (keyboard motions,
    /// navigation).
    Margin,
    /// Scroll only when the target is off screen, by the least that shows it
    /// (pointer-originated moves).
    Minimal,
}

impl Reveal {
    /// The horizontal margin this reveal keeps in a viewport `width` cells
    /// wide (at least 1).
    const fn margin(self, width: u32) -> u32 {
        match self {
            Self::Margin => {
                let half = (width - 1) / 2;
                if half < 10 { half } else { 10 }
            },
            Self::Minimal => 0,
        }
    }
}

/// The `scroll_col` that brings the caret at `col` into an unwrapped viewport
/// of `width` cells, keeping the margin `reveal` asks for on either side.
///
/// Measured in screen cells from the scrolled row's origin — the same
/// [`offset_within_row`] the caret is placed with — so a line whose tabs,
/// wide characters or hints push the caret right scrolls as far as it is
/// drawn, rather than as far as its `char` count suggests. Returns `current`
/// when the caret is already inside the margins.
pub(super) fn reveal_column(
    chars: &[char],
    current: u32,
    col: u32,
    width: u32,
    tab_width: u16,
    hints: LineHints<'_>,
    reveal: Reveal,
) -> u32 {
    let cols = display_cols(chars, tab_width, hints);
    let col = col.min(chars.len() as u32) as usize;
    let caret = cols.get(col).copied().unwrap_or(0);
    // Candidate origins are the columns up to the caret's own.
    let origins = cols.get(..=col).unwrap_or_default();
    let width = width.max(1);
    let margin = reveal.margin(width);
    let offset_at = |origin: usize| {
        origins
            .get(origin)
            .map(|&origin| caret.saturating_sub(origin))
    };
    // The leftmost origin that leaves `margin` cells after the caret.
    let rightward =
        origins.partition_point(|&origin| caret.saturating_sub(origin) > width - margin - 1);
    match offset_at(current as usize) {
        Some(offset) if offset >= margin && offset < width - margin => current,
        // Too far right.
        Some(offset) if offset >= width - margin => rightward as u32,
        // Too far left (or scrolled past it): the rightmost origin that leaves
        // `margin` cells before the caret, or the line start when none can --
        // unless that breaks the right margin. A wide hint or character can
        // leave no origin clear of both; the right one wins, since it keeps
        // the caret on screen, and so the answer is the same whichever side
        // it was reached from. Without that, re-resolving would flip between
        // the two every time.
        _ => {
            let leftward = origins
                .partition_point(|&origin| caret.saturating_sub(origin) >= margin)
                .saturating_sub(1);
            if offset_at(leftward).is_some_and(|offset| offset < width - margin) {
                leftward as u32
            } else {
                rightward as u32
            }
        },
    }
}

pub(super) fn caret_cell(
    area: Rect,
    buffer: &TextBuffer,
    folds: &[Fold],
    state: &EditorState,
    at: LineCol,
) -> Option<(u16, u16)> {
    let line_count = buffer.line_count() as u32;
    let frame = &state.frame;
    let gutter = gutter_width(line_count, frame.line_numbers, frame.change_lane);
    let content_x = area.x.saturating_add(gutter);
    let content_y = area.y.saturating_add(frame.sticky_height);
    let content_height = area.height.saturating_sub(frame.sticky_height);
    if content_x >= area.right() || content_height == 0 {
        return None;
    }
    let content_width = area.right().saturating_sub(content_x);
    let wrap = frame.word_wrap;
    let chars = line_chars(buffer, at.line);
    // Off the buffer, past the line's end, or (unwrapped) scrolled off to
    // the left: not on screen. Drawing it pinned to an edge would show it
    // over text it is not on.
    if at.line >= line_count
        || at.col > chars.len() as u32
        || (!wrap && at.col < state.scroll_col())
    {
        return None;
    }
    let layout = state.row_layout(u32::from(content_width.max(1)), wrap);
    let target = visual_anchor_for_position(buffer, layout, at, state.upstream.contains(&at));
    let mut anchor = normalize_visual_anchor(buffer, folds, layout, state.top_anchor());
    // Walked row by row, virtual lines included: a line hidden in a
    // collapsed fold is never reached, so its caret is never drawn.
    for row in 0..content_height {
        if anchor == target {
            let ranges = layout.text_rows(buffer, at.line, state.scroll_col());
            let index = target.subrow.saturating_sub(layout.above(at.line)) as usize;
            let range = ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            // A continuation row paints its indent before its first column.
            let rel = offset_within_row(
                &chars,
                range.start,
                at.col,
                frame.tab_width,
                layout.hints_on(at.line),
            )
            .saturating_add(range.indent);
            let rel = if wrap {
                rel.min(u32::from(content_width.saturating_sub(1)))
            } else if rel >= u32::from(content_width) {
                return None;
            } else {
                rel
            };
            let x = content_x.saturating_add(u16::try_from(rel).unwrap_or(u16::MAX));
            return Some((x, content_y.saturating_add(row)));
        }
        let next = next_visual_anchor(buffer, folds, layout, anchor);
        if next == anchor {
            break;
        }
        anchor = next;
    }
    None
}

/// Every character of `line` drawn within `area` as `(column, x, y)`, in
/// column order: [`caret_cell`] for each, from one measurement of the line
/// and one walk of the viewport.
pub(super) fn line_cells(
    area: Rect,
    buffer: &TextBuffer,
    folds: &[Fold],
    state: &EditorState,
    line: u32,
) -> Vec<(u32, u16, u16)> {
    let line_count = buffer.line_count() as u32;
    let frame = &state.frame;
    let gutter = gutter_width(line_count, frame.line_numbers, frame.change_lane);
    let content_x = area.x.saturating_add(gutter);
    let content_y = area.y.saturating_add(frame.sticky_height);
    let content_height = area.height.saturating_sub(frame.sticky_height);
    if line >= line_count || content_x >= area.right() || content_height == 0 {
        return Vec::new();
    }
    let content_width = area.right().saturating_sub(content_x);
    let layout = state.row_layout(u32::from(content_width.max(1)), frame.word_wrap);
    let chars = line_chars(buffer, line);
    // `display_col` of every column at once: what `offset_within_row`
    // computes per call.
    let cols = display_cols(&chars, frame.tab_width, layout.hints_on(line));
    let mut ranges: Option<Vec<VisualRange>> = None;
    let mut cells = Vec::new();
    let mut anchor = normalize_visual_anchor(buffer, folds, layout, state.top_anchor());
    for row in 0..content_height {
        if anchor.line > line {
            break;
        }
        if anchor.line == line {
            let ranges =
                ranges.get_or_insert_with(|| layout.text_rows(buffer, line, state.scroll_col()));
            let range = anchor
                .subrow
                .checked_sub(layout.above(line))
                .and_then(|index| ranges.get(index as usize));
            if let Some(range) = range {
                let origin = cols.get(range.start as usize).copied().unwrap_or(0);
                let y = content_y.saturating_add(row);
                for col in range.start..range.end.min(chars.len() as u32) {
                    let at = cols.get(col as usize).copied().unwrap_or(origin);
                    let rel = at.saturating_sub(origin).saturating_add(range.indent);
                    if rel >= u32::from(content_width) {
                        break;
                    }
                    let x = content_x.saturating_add(u16::try_from(rel).unwrap_or(u16::MAX));
                    cells.push((col, x, y));
                }
            }
        }
        let next = next_visual_anchor(buffer, folds, layout, anchor);
        if next == anchor {
            break;
        }
        anchor = next;
    }
    cells
}

/// The last line of the collapsed fold interior that hides `line`, when one
/// does: with nested folds, the outermost one's end.
fn hidden_until(folds: &[Fold], line: u32) -> Option<u32> {
    folds
        .iter()
        .filter(|f| f.collapsed && line > f.start && line <= f.end)
        .map(|f| f.end)
        .max()
}

/// The header line that shows `line`: `line` itself unless a collapsed fold
/// hides it, else the outermost hiding fold's start.
fn shown_line(folds: &[Fold], mut line: u32) -> u32 {
    while let Some(start) = folds
        .iter()
        .filter(|f| f.collapsed && line > f.start && line <= f.end)
        .map(|f| f.start)
        .min()
    {
        line = start;
    }
    line
}

/// The first line after `line` that occupies a row, jumping whole collapsed
/// folds rather than walking their interiors.
fn next_shown_line(folds: &[Fold], line: u32) -> u32 {
    let mut next = line.saturating_add(1);
    while let Some(end) = hidden_until(folds, next) {
        next = end.saturating_add(1);
    }
    next
}

/// The top line that shows `target` in an unwrapped viewport of `height`
/// rows currently starting at `top`.
///
/// Counted in rows, not buffer lines: a collapsed fold's interior takes none,
/// so a line far below `top` in the buffer can still be on screen. Returns
/// `top` unchanged when `target` is already on one of its rows; otherwise
/// scrolls by the least that shows it — `target` becomes the top row when it
/// is above, the bottom row when below. A `target` hidden in a collapsed fold
/// is revealed through the header that shows it.
///
/// Soft-wrapped lines and lines with virtual lines take more than one row, so
/// for them this is an under-estimate that never scrolls further than
/// needed; the render then completes it against the rows it paints
/// ([`reveal_visual_anchor`]).
///
/// `off` rows (see [`scroll_margin`]) are kept between `target` and either
/// edge, counting shown lines and stopping at line 0 and at `last`, so the
/// margin never scrolls before the first line or past the last.
pub(super) fn reveal_line(
    folds: &[Fold],
    top: u32,
    target: u32,
    height: u32,
    (off, last): (u32, u32),
) -> u32 {
    let height = height.max(1);
    let caret = shown_line(folds, target);
    let (mut upper, mut target) = (caret, caret);
    for _ in 0..scroll_margin(off, height) {
        upper = shown_line(folds, upper.saturating_sub(1));
        let next = next_shown_line(folds, target);
        if next <= last {
            target = next;
        }
    }
    if upper < top {
        return upper;
    }
    // A top inside a collapsed fold paints from the first line after it, as
    // the render normalizes it.
    let mut line = if hidden_until(folds, top).is_some() {
        next_shown_line(folds, top)
    } else {
        top
    };
    let mut row = 0;
    while line < target {
        line = next_shown_line(folds, line);
        row += 1;
        if row >= height {
            break;
        }
    }
    if row < height {
        return top;
    }
    // Walk up from the target until it sits on the bottom row.
    let mut first = target;
    for _ in 1..height {
        if first == 0 {
            break;
        }
        first = shown_line(folds, first - 1);
    }
    first
}

/// Clamp `pos` to a valid position within `buffer` (line, then column).
pub(super) fn clamp_to_buffer(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let line = pos.line.min(last_line(buffer));
    LineCol::new(line, pos.col.min(line_len(buffer, line)))
}
