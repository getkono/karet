use std::ops::RangeInclusive;

use super::hint::HintIndex;
use super::hint::LineHints;
use super::text::*;
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct VisualAnchor {
    pub(super) line: u32,
    pub(super) subrow: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisualRange {
    pub(super) start: u32,
    pub(super) end: u32,
}

impl VisualRange {
    pub(super) const fn empty(at: u32) -> Self {
        Self { start: at, end: at }
    }
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
}

impl<'a> Layout<'a> {
    /// The hints on `line`.
    pub(super) fn hints_on(&self, line: u32) -> LineHints<'a> {
        self.hints.line(line)
    }
}

/// Split one logical line into source-column ranges for soft wrapping. Whitespace is
/// kept in the range before the break so every source column maps to exactly one row;
/// words wider than the viewport are split at the hard width.
pub(super) fn character_width(ch: char, display_col: u32, tab_width: u16) -> u32 {
    if ch == '\t' {
        let width = u32::from(tab_width.max(1));
        width - (display_col % width)
    } else {
        u32::try_from(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u32::MAX)
    }
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
    if hints.is_empty() {
        return chars
            .iter()
            .take(source_col as usize)
            .fold(0_u32, |col, ch| {
                col.saturating_add(character_width(*ch, col, tab_width))
            });
    }
    // Clamped to the line: past its end there are no characters left to walk,
    // so a hint anchored beyond it cannot shift anything.
    let limit = source_col.min(chars.len() as u32);
    let mut col = 0_u32;
    for index in 0..limit {
        col = col.saturating_add(hints.width_at(index));
        if let Some(ch) = chars.get(index as usize) {
            col = col.saturating_add(character_width(*ch, col, tab_width));
        }
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
    let mut source = start.min(chars.len() as u32);
    let end = end.min(chars.len() as u32);
    let mut absolute = display_col(chars, source, tab_width, hints);
    let target = absolute.saturating_add(offset);
    while source < end {
        let Some(ch) = chars.get(source as usize) else {
            break;
        };
        // The hint anchored at `source` paints between its caret slot and the
        // character there, so its cells belong to this column.
        let after_hint = absolute.saturating_add(hints.width_at(source));
        if target < after_hint {
            return source;
        }
        let width = character_width(*ch, after_hint, tab_width);
        if target < after_hint.saturating_add(width) {
            // Round to the nearer boundary: the right half of a wide
            // character, or a tab's trailing cells, put the caret after it.
            if snap == Snap::Nearest && target.saturating_sub(after_hint).saturating_mul(2) >= width
            {
                source = source.saturating_add(1);
            }
            break;
        }
        absolute = after_hint.saturating_add(width);
        source = source.saturating_add(1);
    }
    source
}

pub(super) fn visual_ranges(
    buffer: &TextBuffer,
    line: u32,
    layout: Layout<'_>,
) -> Vec<VisualRange> {
    let chars: Vec<char> = buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let len = chars.len() as u32;
    if len == 0 {
        return vec![VisualRange::empty(0)];
    }
    if layout
        .unwrapped_lines
        .iter()
        .any(|range| range.contains(&line))
    {
        return vec![VisualRange { start: 0, end: len }];
    }
    let hints = layout.hints_on(line);
    let width = layout.width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0_u32;
    while start < len {
        let mut hard_end = start;
        let mut used = 0_u32;
        // `display_col(start)` stops short of the hint at `start`: this row
        // paints it, so its cells are counted below.
        let mut absolute = display_col(&chars, start, layout.tab_width, hints);
        while hard_end < len {
            let hint = hints.width_at(hard_end);
            let char_width = character_width(
                chars[hard_end as usize],
                absolute.saturating_add(hint),
                layout.tab_width,
            );
            let step = hint.saturating_add(char_width);
            if hard_end > start && used.saturating_add(step) > width {
                break;
            }
            used = used.saturating_add(step);
            absolute = absolute.saturating_add(step);
            hard_end += 1;
            if used >= width {
                break;
            }
        }
        let end = if hard_end < len {
            chars[start as usize..hard_end as usize]
                .iter()
                .rposition(|ch| ch.is_whitespace())
                .map_or(hard_end, |index| start + index as u32 + 1)
        } else {
            hard_end
        };
        let end = end.max(start.saturating_add(1)).min(len);
        ranges.push(VisualRange { start, end });
        start = end;
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
    let rows = visual_ranges(buffer, line, layout).len().max(1) as u32;
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
    let rows = visual_ranges(buffer, anchor.line, layout).len() as u32;
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
            let rows = visual_ranges(buffer, line, layout).len().max(1) as u32;
            return VisualAnchor {
                line,
                subrow: rows - 1,
            };
        }
    }
    anchor
}

pub(super) fn next_line_anchor(
    folds: &[Fold],
    line_count: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let mut line = anchor.line.saturating_add(1);
    while line < line_count && hidden_in(folds, line) {
        line += 1;
    }
    if line >= line_count {
        anchor
    } else {
        VisualAnchor { line, subrow: 0 }
    }
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
/// [`row_holds`].
pub(super) fn visual_anchor_for_position(
    buffer: &TextBuffer,
    layout: Layout<'_>,
    pos: LineCol,
    upstream: bool,
) -> VisualAnchor {
    let line = pos.line.min(last_line(buffer));
    let ranges = visual_ranges(buffer, line, layout);
    let last = ranges.len().saturating_sub(1);
    let subrow = ranges
        .iter()
        .enumerate()
        .find_map(|(index, range)| {
            row_holds(*range, index == last, pos.col, upstream).then_some(index as u32)
        })
        .unwrap_or(last as u32);
    VisualAnchor { line, subrow }
}

/// The scroll margin a viewport of `height` rows can honour: `off` rows,
/// capped so the rows above and below the caret still fit (a margin of half
/// the viewport or more would pin the caret and scroll on every move).
pub(super) fn scroll_margin(off: u32, height: u32) -> u32 {
    off.min(height.max(1).saturating_sub(1) / 2)
}

/// The top row that shows the caret at `cursor` in a soft-wrapped viewport of
/// `height` rows currently starting at `current`, keeping `off` rows (see
/// [`scroll_margin`]) between it and either edge. The margin stops at the
/// buffer's first and last rows, so it never scrolls past either end.
#[allow(clippy::too_many_arguments)]
// The wrapped twin of `reveal_line`: the geometry, the viewport and the
// caret are each needed, and bundling them would add a type used once.
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
    let caret = visual_anchor_for_position(buffer, layout, cursor, upstream);
    let off = scroll_margin(off, u32::from(height));
    let (mut upper, mut target) = (caret, caret);
    for _ in 0..off {
        upper = previous_visual_anchor(buffer, folds, layout, upper);
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
    let mut cols = Vec::with_capacity(chars.len().saturating_add(1));
    let mut col = 0_u32;
    cols.push(col);
    for (index, ch) in (0_u32..).zip(chars) {
        col = col.saturating_add(hints.width_at(index));
        col = col.saturating_add(character_width(*ch, col, tab_width));
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
    let gutter = gutter_width(line_count, state.last_line_numbers, state.last_change_lane);
    let content_x = area.x.saturating_add(gutter);
    let content_y = area.y.saturating_add(state.sticky_height);
    let content_height = area.height.saturating_sub(state.sticky_height);
    if content_x >= area.right() || content_height == 0 {
        return None;
    }
    let content_width = area.right().saturating_sub(content_x);

    if state.last_word_wrap {
        let layout = state.layout(u32::from(content_width.max(1)));
        let mut anchor = normalize_visual_anchor(
            buffer,
            folds,
            layout,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        for row in 0..content_height {
            let ranges = visual_ranges(buffer, anchor.line, layout);
            let index = (anchor.subrow as usize).min(ranges.len().saturating_sub(1));
            let range = ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            let last = index + 1 == ranges.len();
            if anchor.line == at.line
                && row_holds(range, last, at.col, state.upstream.contains(&at))
            {
                let chars: Vec<char> = buffer
                    .line(at.line as usize)
                    .unwrap_or_default()
                    .chars()
                    .collect();
                let rel = offset_within_row(
                    &chars,
                    range.start,
                    at.col,
                    state.last_tab_width,
                    layout.hints_on(at.line),
                );
                let x = content_x.saturating_add(
                    u16::try_from(rel.min(u32::from(content_width.saturating_sub(1))))
                        .unwrap_or(u16::MAX),
                );
                return Some((x, content_y.saturating_add(row)));
            }
            anchor = next_visual_anchor(buffer, folds, layout, anchor);
        }
        return None;
    }

    let top = first_visible(
        folds,
        state.scroll_line.min(line_count.saturating_sub(1)),
        line_count,
    );
    if at.line < top || hidden_in(folds, at.line) {
        return None;
    }
    let mut vis_row: u16 = 0;
    let mut ll = top;
    while ll < at.line {
        if !hidden_in(folds, ll) {
            vis_row = vis_row.saturating_add(1);
        }
        ll += 1;
    }
    if vis_row >= content_height {
        return None;
    }
    let chars: Vec<char> = buffer
        .line(at.line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    if at.col > chars.len() as u32 {
        return None;
    }
    // A caret scrolled off either side is not on screen: drawing it pinned to
    // the edge would show it over text it is not on.
    if at.col < state.scroll_col {
        return None;
    }
    let rel = offset_within_row(
        &chars,
        state.scroll_col,
        at.col,
        state.last_tab_width,
        state.hints_on(at.line),
    );
    if rel >= u32::from(content_width) {
        return None;
    }
    let cx = content_x.saturating_add(u16::try_from(rel).unwrap_or(u16::MAX));
    let cy = content_y.saturating_add(vis_row);
    (cy < area.bottom()).then_some((cx, cy))
}

pub(super) fn first_visible(folds: &[Fold], mut line: u32, line_count: u32) -> u32 {
    while line < line_count && hidden_in(folds, line) {
        line += 1;
    }
    line
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
/// Soft-wrapped lines take more than one row, so for a wrapped view this is an
/// under-estimate that never scrolls further than needed; the render then
/// completes it against the real wrap.
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
