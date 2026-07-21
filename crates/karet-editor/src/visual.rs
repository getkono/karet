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
pub(super) fn character_width(ch: char, display_col: u32, tab_width: u16) -> u32 {
    if ch == '\t' {
        let width = u32::from(tab_width.max(1));
        width - (display_col % width)
    } else {
        u32::try_from(unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u32::MAX)
    }
}

pub(super) fn display_col(chars: &[char], source_col: u32, tab_width: u16) -> u32 {
    chars
        .iter()
        .take(source_col as usize)
        .fold(0_u32, |col, ch| {
            col.saturating_add(character_width(*ch, col, tab_width))
        })
}

pub(super) fn source_col_at_display_offset(
    chars: &[char],
    start: u32,
    end: u32,
    offset: u32,
    tab_width: u16,
) -> u32 {
    let mut source = start.min(chars.len() as u32);
    let end = end.min(chars.len() as u32);
    let mut absolute = display_col(chars, source, tab_width);
    let target = absolute.saturating_add(offset);
    while source < end {
        let width = character_width(chars[source as usize], absolute, tab_width);
        if absolute.saturating_add(width) > target {
            break;
        }
        absolute = absolute.saturating_add(width);
        source += 1;
    }
    source
}

pub(super) fn visual_ranges(
    buffer: &TextBuffer,
    line: u32,
    width: u32,
    tab_width: u16,
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
    let width = width.max(1);
    let mut ranges = Vec::new();
    let mut start = 0_u32;
    while start < len {
        let mut hard_end = start;
        let mut used = 0_u32;
        let mut absolute = display_col(&chars, start, tab_width);
        while hard_end < len {
            let char_width = character_width(chars[hard_end as usize], absolute, tab_width);
            if hard_end > start && used.saturating_add(char_width) > width {
                break;
            }
            used = used.saturating_add(char_width);
            absolute = absolute.saturating_add(char_width);
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
    width: u32,
    tab_width: u16,
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
    let rows = visual_ranges(buffer, line, width, tab_width).len().max(1) as u32;
    VisualAnchor {
        line,
        subrow: anchor.subrow.min(rows - 1),
    }
}

pub(super) fn next_visual_anchor(
    buffer: &TextBuffer,
    folds: &[Fold],
    width: u32,
    tab_width: u16,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, tab_width, anchor);
    let rows = visual_ranges(buffer, anchor.line, width, tab_width).len() as u32;
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
    tab_width: u16,
    anchor: VisualAnchor,
) -> VisualAnchor {
    let anchor = normalize_visual_anchor(buffer, folds, width, tab_width, anchor);
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
            let rows = visual_ranges(buffer, line, width, tab_width).len().max(1) as u32;
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
    tab_width: u16,
    start: VisualAnchor,
    row: u32,
) -> VisualAnchor {
    let mut anchor = normalize_visual_anchor(buffer, folds, width, tab_width, start);
    for _ in 0..row {
        let next = next_visual_anchor(buffer, folds, width, tab_width, anchor);
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
    tab_width: u16,
    pos: LineCol,
) -> VisualAnchor {
    let line = pos.line.min(last_line(buffer));
    let ranges = visual_ranges(buffer, line, width, tab_width);
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
    tab_width: u16,
    height: u16,
    current: VisualAnchor,
    cursor: LineCol,
) -> VisualAnchor {
    let current = normalize_visual_anchor(buffer, folds, width, tab_width, current);
    let target = visual_anchor_for_position(buffer, width, tab_width, cursor);
    let mut probe = current;
    for _ in 0..height.max(1) {
        if probe == target {
            return current;
        }
        let next = next_visual_anchor(buffer, folds, width, tab_width, probe);
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
        let previous = previous_visual_anchor(buffer, folds, width, tab_width, revealed);
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
            state.last_tab_width,
            VisualAnchor {
                line: state.scroll_line,
                subrow: state.scroll_subrow,
            },
        );
        for row in 0..content_height {
            let ranges = visual_ranges(buffer, anchor.line, width, state.last_tab_width);
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
                let chars: Vec<char> = buffer
                    .line(at.line as usize)
                    .unwrap_or_default()
                    .chars()
                    .collect();
                let rel = display_col(&chars, at.col, state.last_tab_width)
                    .saturating_sub(display_col(&chars, range.start, state.last_tab_width));
                let x = content_x.saturating_add(
                    u16::try_from(rel.min(u32::from(content_width.saturating_sub(1))))
                        .unwrap_or(u16::MAX),
                );
                return Some((x, content_y.saturating_add(row)));
            }
            anchor = next_visual_anchor(buffer, folds, width, state.last_tab_width, anchor);
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
    let rel = if at.col < state.scroll_col {
        0
    } else {
        display_col(&chars, at.col, state.last_tab_width).saturating_sub(display_col(
            &chars,
            state.scroll_col,
            state.last_tab_width,
        ))
    };
    let rel = rel.min(u32::from(content_width.saturating_sub(1)));
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

/// Clamp `pos` to a valid position within `buffer` (line, then column).
pub(super) fn clamp_to_buffer(buffer: &TextBuffer, pos: LineCol) -> LineCol {
    let line = pos.line.min(last_line(buffer));
    LineCol::new(line, pos.col.min(line_len(buffer, line)))
}
