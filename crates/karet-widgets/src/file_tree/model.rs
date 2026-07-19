use super::state::EditState;
use super::*;

/// One immediate directory entry, with its gitignore status.
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
    pub(super) ignored: bool,
}

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

pub(super) fn prev_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    if i == 0 {
        return 0;
    }
    i -= 1;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
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
    edit.selection
        .map(|(a, b)| (a.min(b), a.max(b)))
        .filter(|(a, b)| a < b && *b <= edit.buffer.len())
}

pub(super) fn replace_edit_selection(edit: &mut EditState, text: &str) -> bool {
    let Some((start, end)) = edit_selection(edit) else {
        return false;
    };
    edit.buffer.replace_range(start..end, text);
    edit.cursor = start + text.len();
    edit.selection = None;
    true
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
    let cursor = edit.cursor.min(edit.buffer.len());
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

/// Read the immediate entries of `dir`, dirs first then case-insensitive name.
///
/// Gitignored entries are listed and flagged `ignored` (VS Code dims them) rather
/// than filtered out. The `.git` directory is always excluded; dotfiles are shown
/// unless `show_hidden` is false.
pub(super) fn read_dir_sorted(
    dir: &Path,
    show_hidden: bool,
    respect_gitignore: bool,
) -> Vec<Entry> {
    // The full listing (gitignore off): everything the user should see.
    let all = walk_immediate(dir, show_hidden, false);
    let mut entries: Vec<Entry> = if respect_gitignore {
        // The non-ignored subset; anything in `all` but not here is gitignored.
        let visible: BTreeSet<PathBuf> = walk_immediate(dir, show_hidden, true)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        all.into_iter()
            .map(|(path, is_dir)| {
                let ignored = !visible.contains(&path);
                Entry {
                    path,
                    is_dir,
                    ignored,
                }
            })
            .collect()
    } else {
        all.into_iter()
            .map(|(path, is_dir)| Entry {
                path,
                is_dir,
                ignored: false,
            })
            .collect()
    };
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| name_key(&a.path).cmp(&name_key(&b.path)))
    });
    entries
}

/// List the immediate children of `dir` as `(path, is_dir)`, honoring the hidden
/// and gitignore filters, but always excluding the `.git` directory.
pub(super) fn walk_immediate(
    dir: &Path,
    show_hidden: bool,
    git_ignore: bool,
) -> Vec<(PathBuf, bool)> {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .max_depth(Some(1))
        .hidden(!show_hidden)
        .git_ignore(git_ignore)
        .git_global(git_ignore)
        .git_exclude(git_ignore)
        .require_git(false)
        .parents(git_ignore);
    builder
        .build()
        .flatten()
        .filter(|e| e.depth() > 0) // skip the directory itself
        .filter(|e| e.file_name() != std::ffi::OsStr::new(".git"))
        .map(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            (e.path().to_path_buf(), is_dir)
        })
        .collect()
}

/// A case-insensitive sort key from a path's file name.
pub(super) fn name_key(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}
