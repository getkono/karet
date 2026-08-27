//! A local directory source, for tests only.
//!
//! `FileTreeState` renders listings it is *given*; it no longer reads any. These
//! tests still build real directories, because the layout rules under test —
//! chain compaction, ordering, how an ignored ancestor dims its whole subtree —
//! are only interesting against realistic input. So the walk that used to live in
//! the widget lives here instead, as the embedder's half of the contract.

use karet_core::DirEntry;

use super::*;

/// Build `state`'s rows for `root`, supplying every listing it asks for.
///
/// Mirrors what an embedder does with a backend: rebuild, see what is missing,
/// fetch it, supply it, repeat. The loop terminates because each round either
/// resolves a miss or produces none.
pub(super) fn build_from_disk(state: &mut FileTreeState, root: &Path) {
    state.ensure_built(root);
    // Bounded so a bug cannot hang the suite; far more rounds than any fixture
    // here needs, since one round resolves a whole level.
    for _ in 0..64 {
        let missing = state.take_missing();
        if missing.is_empty() {
            break;
        }
        for dir in missing {
            let children = read_dir_sorted(&dir, state.show_hidden(), state.respect_gitignore());
            state.supply(dir, children);
        }
        state.ensure_built(root);
    }
}

/// Read the immediate entries of `dir`, dirs first then case-insensitive name.
///
/// Gitignored entries are listed and flagged `ignored` (VS Code dims them) rather
/// than filtered out. The `.git` directory is always excluded; dotfiles are shown
/// unless `show_hidden` is false.
fn read_dir_sorted(dir: &Path, show_hidden: bool, respect_gitignore: bool) -> Vec<DirEntry> {
    // The full listing (gitignore off): everything the user should see.
    let all = walk_immediate(dir, show_hidden, false);
    let mut entries: Vec<DirEntry> = if respect_gitignore {
        // The non-ignored subset; anything in `all` but not here is gitignored.
        let visible: BTreeSet<PathBuf> = walk_immediate(dir, show_hidden, true)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect();
        all.into_iter()
            .map(|(path, is_dir, is_symlink)| {
                let ignored = !visible.contains(&path);
                DirEntry {
                    is_repository: is_dir && path.join(".git").exists(),
                    path,
                    is_dir,
                    is_symlink,
                    ignored,
                }
            })
            .collect()
    } else {
        all.into_iter()
            .map(|(path, is_dir, is_symlink)| DirEntry {
                is_repository: is_dir && path.join(".git").exists(),
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
fn walk_immediate(dir: &Path, show_hidden: bool, git_ignore: bool) -> Vec<(PathBuf, bool, bool)> {
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
