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

/// Split one logical line into source-column ranges for soft wrapping. Whitespace is
/// kept in the range before the break so every source column maps to exactly one row;
/// words wider than the viewport are split at the hard width.
pub(super) fn visual_ranges(buffer: &TextBuffer, line: u32, width: u32) -> Vec<VisualRange> {
    let chars: Vec<char> = buffer
        .line(line as usize)
        .unwrap_or_default()
        .chars()
        .collect();
    let len = chars.len() as u32;
    if len == 0 {
        return vec![VisualRange::empty(0)];
    }
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0_u32;
    while start < len {
        let hard_end = start.saturating_add(width).min(len);
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
    width: u32,
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
    let rows = visual_ranges(buffer, line, width).len().max(1) as u32;
    VisualAnchor {
        line,
        subrow: anchor.subrow.min(rows - 1),
    }
}

pub(super) fn next_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, anchor);
    let rows = visual_ranges(buffer, anchor.line, width).len() as u32;
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
    width: u32,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, anchor);
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
            let rows = visual_ranges(buffer, line, width).len().max(1) as u32;
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

pub(super) fn visual_anchor_at_row(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    start: VisualAnchor,
    row: u32,
) -> VisualAnchor {
    let mut anchor = normalize_visual_anchor(buffer, folds, width, start);
    for _ in 0..row {
        let next = next_visual_anchor(buffer, folds, width, anchor);
        if next == anchor {
            break;
        }
        anchor = next;
    }
    anchor
}

pub(super) fn visual_anchor_for_position(
    buffer: &TextBuffer,
    width: u32,
    pos: LineCol,
) -> VisualAnchor {
    let line = pos.line.min(last_line(buffer));
    let ranges = visual_ranges(buffer, line, width);
    let last = ranges.len().saturating_sub(1);
    let subrow = ranges
        .iter()
        .enumerate()
        .find_map(|(index, range)| {
            (range.start <= pos.col
                && (pos.col < range.end || (index == last && pos.col == range.end)))
                .then_some(index as u32)
        })
        .unwrap_or(last as u32);
    VisualAnchor { line, subrow }
}

pub(super) fn reveal_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    height: u16,
    current: VisualAnchor,
    cursor: LineCol,
) -> VisualAnchor {
    let current = normalize_visual_anchor(buffer, folds, width, current);
    let target = visual_anchor_for_position(buffer, width, cursor);
    let mut probe = current;
    for _ in 0..height.max(1) {
        if probe == target {
            return current;
        }
        let next = next_visual_anchor(buffer, folds, width, probe);
        if next == probe {
            break;
        }
        probe = next;
    }
    if target < current {
        return target;
    }
    let mut revealed = target;
    for _ in 1..height.max(1) {
        let previous = previous_visual_anchor(buffer, folds, width, revealed);
        if previous == revealed {
            break;
        }
        revealed = previous;
    }
    revealed
}

pub(super) fn caret_cell(
    area: Rect,
    buffer: &TextBuffer,
    folds: &[Fold],
    state: &EditorState,
    at: LineCol,
) -> Option<(u16, u16)> {
    let line_count = buffer.line_count() as u32;
    let gutter = 1 + digit_count(line_count.max(1)) as u16 + 1;
    let content_x = area.x.saturating_add(gutter);
    let content_y = area.y.saturating_add(state.sticky_height);
    let content_height = area.height.saturating_sub(state.sticky_height);
    if content_x >= area.right() || content_height == 0 {
        return None;
    }
    let content_width = area.right().saturating_sub(content_x);

    if state.last_word_wrap {
        let width = u32::from(content_width.max(1));
        let mut anchor = normalize_visual_anchor(
            buffer,
            folds,
            width,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        for row in 0..content_height {
            let ranges = visual_ranges(buffer, anchor.line, width);
            let index = (anchor.subrow as usize).min(ranges.len().saturating_sub(1));
            let range = ranges
                .get(index)
                .copied()
                .unwrap_or_else(|| VisualRange::empty(0));
            let last = index + 1 == ranges.len();
            if anchor.line == at.line
                && (range.start <= at.col)
                && (at.col < range.end || (last && at.col == range.end))
            {
                let rel = at.col.saturating_sub(range.start);
                let x = content_x.saturating_add(
                    u16::try_from(rel.min(u32::from(content_width.saturating_sub(1))))
                        .unwrap_or(u16::MAX),
                );
                return Some((x, content_y.saturating_add(row)));
            }
            anchor = next_visual_anchor(buffer, folds, width, anchor);
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
    let rel = i64::from(at.col) - i64::from(state.scroll_col);
    let max_rel = i64::from(content_width.saturating_sub(1));
    let cx = content_x.saturating_add(u16::try_from(rel.clamp(0, max_rel)).unwrap_or(0));
    let cy = content_y.saturating_add(vis_row);
    (cy < area.bottom()).then_some((cx, cy))
}

pub(super) fn first_visible(folds: &[Fold], mut line: u32, line_count: u32) -> u32 {
    while line < line_count && hidden_in(folds, line) {
        line += 1;
    }
    line
}

/// Clamp `pos` to a valid position within `buffer` (line, then column).
pub(super) fn clamp_to_buffer(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let line = pos.line.min(last_line(buffer));
    LineCol::new(line, pos.col.min(line_len(buffer, line)))
}
