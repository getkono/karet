//! Workspace helpers: collecting the files shown in the quick-open list (kept
//! off the UI thread by [`FileIndex`]), and the recently opened files and
//! workspaces ([`RecentStore`]). Opening a file into a tab is the tab's own
//! business ([`Tab::open`](crate::tab::Tab::open)).

use std::path::Path;
use std::path::PathBuf;
mod file_index;
mod recent;

pub(crate) use file_index::FileIndex;
pub(crate) use file_index::Scan;
pub(crate) use recent::RecentStore;
pub(crate) use recent::absolute;

/// Collect every file under `root` (gitignore-aware) for the quick-open list,
/// sorted by display path. Returns workspace-relative display paths paired with
/// their absolute path.
///
/// Uncapped and blocking: a large workspace takes a while to walk, so the app
/// only ever calls this off the UI thread, through [`FileIndex`].
#[must_use]
pub fn list_files(root: &Path) -> Vec<(String, PathBuf)> {
    // The workspace-search walk's own file listing, so quick-open covers the
    // same tree a search does: symlinks never followed, ignore files applied
    // (those above `root` included), heavyweight dirs pruned.
    let mut out: Vec<(String, PathBuf)> = karet_search::walk_files(root)
        .map(|abs| (display_path(root, &abs), abs))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `path` relative to `root` for display, or the whole path when it lies outside.
fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempDir {
        path: PathBuf,
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn temp_dir() -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("karet-ws-{}-{}", std::process::id(), n));
        let _ = std::fs::create_dir_all(&path);
        TempDir { path }
    }
    #[test]
    fn list_files_finds_and_sorts() {
        let dir = temp_dir();
        let _ = std::fs::write(dir.path.join("b.txt"), "b");
        let _ = std::fs::write(dir.path.join("a.txt"), "a");
        let files = list_files(&dir.path);
        let names: Vec<&str> = files.iter().map(|(d, _)| d.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "b.txt"]);
    }
}
