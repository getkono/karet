use super::*;

/// One immediate directory entry, with its gitignore status.
pub(super) struct Entry {
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
    pub(super) is_symlink: bool,
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
            .map(|(path, _, _)| path)
            .collect();
        all.into_iter()
            .map(|(path, is_dir, is_symlink)| {
                let ignored = !visible.contains(&path);
                Entry {
                    path,
                    is_dir,
                    is_symlink,
                    ignored,
                }
            })
            .collect()
    } else {
        all.into_iter()
            .map(|(path, is_dir, is_symlink)| Entry {
                path,
                is_dir,
                is_symlink,
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
) -> Vec<(PathBuf, bool, bool)> {
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
            let is_symlink = e.file_type().is_some_and(|t| t.is_symlink());
            (e.path().to_path_buf(), is_dir, is_symlink)
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
