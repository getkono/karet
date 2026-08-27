use super::state::EditState;
use super::*;

/// The display label for a path: its file name, or `?` if it has none.
pub(super) fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

pub(super) fn rename_selection(path: &Path, buffer: &str) -> Option<(usize, usize)> {
    if path.is_dir() {
        return Some((0, buffer.len()));
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .and_then(|stem| buffer.find(stem).map(|start| (start, start + stem.len())))
        .or(Some((0, buffer.len())))
}

pub(super) fn next_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i >= s.len() {
        return s.len();
    }
    i += 1;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

pub(super) fn edit_selection(edit: &EditState) -> Option<(usize, usize)> {
    edit.field
        .selection()
        .filter(|range| range.end <= edit.buffer.len())
        .map(|range| (range.start, range.end))
}

pub(super) fn push_editing_spans(
    spans: &mut Vec<Span<'static>>,
    edit: Option<&EditState>,
    fg: Color,
    selection_bg: Color,
) {
    let Some(edit) = edit else {
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        return;
    };
    let normal = Style::default().fg(fg);
    if let Some((start, end)) = edit_selection(edit) {
        if start > 0 {
            spans.push(Span::styled(edit.buffer[..start].to_string(), normal));
        }
        spans.push(Span::styled(
            edit.buffer[start..end].to_string(),
            Style::default().fg(fg).bg(selection_bg),
        ));
        if end < edit.buffer.len() {
            spans.push(Span::styled(edit.buffer[end..].to_string(), normal));
        }
        return;
    }
    let cursor = edit.field.cursor().min(edit.buffer.len());
    if cursor > 0 {
        spans.push(Span::styled(edit.buffer[..cursor].to_string(), normal));
    }
    if cursor < edit.buffer.len() {
        let next = next_boundary(&edit.buffer, cursor);
        spans.push(Span::styled(
            edit.buffer[cursor..next].to_string(),
            Style::default().fg(fg).add_modifier(Modifier::REVERSED),
        ));
        if next < edit.buffer.len() {
            spans.push(Span::styled(edit.buffer[next..].to_string(), normal));
        }
    } else {
        spans.push(Span::styled(
            " ",
            Style::default().fg(fg).add_modifier(Modifier::REVERSED),
        ));
    }
}
