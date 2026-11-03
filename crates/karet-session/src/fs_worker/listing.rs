//! Directory and workspace listings.
//!
//! Both are gitignore-aware walks, and both deliberately *flag* rather than hide:
//! an ignored entry still appears in a tree (dimmed, the way VS Code shows it), so
//! a user can see what is on disk even when the build system generated it.

use std::path::Path;
use std::path::PathBuf;

use karet_core::DirEntry;

/// Walk `root` for the quick-open list, stopping at `limit`.
///
/// Returns the paths and whether the walk was cut short, so a client can say its
/// list is partial rather than implying the workspace is that small.
pub(super) fn workspace_files(root: &Path, limit: usize) -> (Vec<PathBuf>, bool) {
    let mut walk = karet_search::walk_files(root);
    let mut files: Vec<PathBuf> = walk.by_ref().take(limit).collect();
    let truncated = walk.next().is_some();
    files.sort();
    (files, truncated)
}

/// List the immediate children of `dir` in display order.
///
/// When `respect_gitignore` is set, the listing is the union of the filtered and
/// unfiltered walks: everything present in the latter but not the former is
/// flagged [`DirEntry::ignored`]. `.git` is always excluded — it is never
/// something a user browses, and it is enormous.
pub(super) fn directory(
    dir: &Path,
    show_hidden: bool,
    respect_gitignore: bool,
) -> Result<Vec<DirEntry>, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let all = immediate(dir, show_hidden, false);
    let visible: std::collections::BTreeSet<PathBuf> = if respect_gitignore {
        immediate(dir, show_hidden, true)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect()
    } else {
        std::collections::BTreeSet::new()
    };
    let mut entries: Vec<DirEntry> = all
        .into_iter()
        .map(|(path, is_dir, is_symlink)| {
            let ignored = respect_gitignore && !visible.contains(&path);
            // A nested repository is badged by the tree, and only this side can
            // see the `.git` that makes it one.
            let is_repository = is_dir && path.join(".git").exists();
            DirEntry {
                path,
                is_dir,
                is_symlink,
                ignored,
                is_repository,
            }
        })
        .collect();
    karet_core::sort_entries(&mut entries);
    Ok(entries)
}

/// The immediate children of `dir` as `(path, is_dir, is_symlink)`.
///
/// Depth is capped at one: a tree expands lazily, and walking deeper would read
/// directories the user has not opened.
fn immediate(dir: &Path, show_hidden: bool, git_ignore: bool) -> Vec<(PathBuf, bool, bool)> {
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .standard_filters(git_ignore)
        .hidden(!show_hidden)
        // Honor `.gitignore` outside a git repository too, matching the
        // workspace walk and what an editor user expects.
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .flatten()
        // The walk yields `dir` itself first; a listing is its children.
        .filter(|entry| entry.path() != dir)
        .map(|entry| {
            let path = entry.path().to_path_buf();
            let is_symlink = entry.path_is_symlink();
            // The entry's own type, never the link target's: a link to a
            // directory is shown as a link rather than expanded, so a link to an
            // ancestor cannot drive an endless chain of nested listings.
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            (path, is_dir, is_symlink)
        })
        .collect()
}
