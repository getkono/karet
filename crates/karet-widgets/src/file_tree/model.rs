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
