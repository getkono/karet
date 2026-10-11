//! The quick-open file list, built off the UI thread and kept fresh.
//!
//! A workspace walk is uncapped, so on a large repository it takes long enough
//! that it must never run on the UI thread. [`FileIndex`] runs [`list_files`] on
//! a worker thread, hands the result back over a channel the event loop selects
//! on, and then follows the session's filesystem-watch events (`FsChanged`,
//! sourced from `karet-watch`): a removal is applied in place, and a path the
//! list does not know yet asks for a fresh walk, coalesced so a burst of
//! creations costs one walk rather than one each.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use tokio::sync::mpsc;

use super::display_path;
use super::list_files;
use crate::app::Pending;

/// One finished walk of a workspace.
#[derive(Debug)]
pub(crate) struct Scan {
    /// The root the walk covered, so a result for another root is dropped.
    root: PathBuf,
    /// Every file found, `(display, absolute)`, sorted by display path.
    files: Vec<(String, PathBuf)>,
}

/// The workspace's file list for quick-open.
pub(crate) struct FileIndex {
    root: PathBuf,
    /// `(display, absolute)` pairs sorted by display path.
    files: Vec<(String, PathBuf)>,
    /// Whether any walk has landed yet.
    loaded: bool,
    /// When the walk in flight started, if one is.
    scanning: Option<Pending>,
    /// Whether another walk is owed once the one in flight lands.
    rescan: bool,
    /// Paths that asked for a walk and were not in its result: ignored by an
    /// ignore file, so a later event for them must not walk again.
    ignored: HashSet<PathBuf>,
    /// Paths that asked for the walk in flight (or the one owed).
    probes: Vec<PathBuf>,
    tx: mpsc::UnboundedSender<Scan>,
    rx: Option<mpsc::UnboundedReceiver<Scan>>,
}

impl FileIndex {
    /// An empty index for `root`; nothing is walked until
    /// [`ensure_started`](Self::ensure_started).
    #[must_use]
    pub(crate) fn new(root: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            root,
            files: Vec::new(),
            loaded: false,
            scanning: None,
            rescan: false,
            ignored: HashSet::new(),
            probes: Vec::new(),
            tx,
            rx: Some(rx),
        }
    }

    /// The channel finished walks arrive on; the event loop takes it once.
    pub(crate) fn take_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<Scan>> {
        self.rx.take()
    }

    /// The current list, `(display, absolute)`, sorted by display path.
    #[must_use]
    pub(crate) fn files(&self) -> &[(String, PathBuf)] {
        &self.files
    }

    /// The first walk, while it is still in flight: what a loading placeholder
    /// anchors on. A later refresh is not loading — the list it replaces is good.
    #[must_use]
    pub(crate) fn loading(&self) -> Option<Pending> {
        self.scanning.filter(|_| !self.loaded)
    }

    /// Start the first walk unless one has run or is running.
    pub(crate) fn ensure_started(&mut self) {
        if !self.loaded && self.scanning.is_none() {
            self.spawn();
        }
    }

    /// Walk again: now, or once the walk in flight lands.
    fn request_scan(&mut self) {
        if self.scanning.is_some() {
            self.rescan = true;
        } else {
            self.spawn();
        }
    }

    fn spawn(&mut self) {
        let root = self.root.clone();
        let tx = self.tx.clone();
        let started = std::thread::Builder::new()
            .name("karet-file-index".to_string())
            .spawn(move || {
                let files = list_files(&root);
                // The receiver is gone only when the app is shutting down.
                let _ = tx.send(Scan { root, files });
            });
        match started {
            Ok(_) => self.scanning = Some(Pending::start()),
            Err(error) => tracing::warn!("file index: cannot start a walk: {error}"),
        }
    }

    /// Adopt a finished walk. Returns whether the list changed (a walk for
    /// another root is dropped).
    pub(crate) fn accept(&mut self, scan: Scan) -> bool {
        if scan.root != self.root {
            return false;
        }
        self.files = scan.files;
        self.loaded = true;
        self.scanning = None;
        if self.rescan {
            // The probes stay: they are answered by the walk owed, not this one,
            // which may have started before they were created.
            self.rescan = false;
            self.spawn();
        } else {
            for probe in std::mem::take(&mut self.probes) {
                if !self.covers(&probe) {
                    self.ignored.insert(probe);
                }
            }
        }
        true
    }

    /// Follow a batch of filesystem changes. Returns whether the list changed
    /// now (a walk asked for here reports through [`accept`](Self::accept)).
    pub(crate) fn on_fs_changed(&mut self, paths: &[PathBuf]) -> bool {
        let mut changed = false;
        let mut walk = false;
        for path in paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if relative
                .file_name()
                .is_some_and(|name| name == ".gitignore" || name == ".ignore")
            {
                // What is ignored changed, so every verdict on record is stale.
                self.ignored.clear();
                walk = true;
                continue;
            }
            if relative.as_os_str().is_empty() || !walked(relative) {
                continue;
            }
            match std::fs::symlink_metadata(path) {
                Err(_) => {
                    let before = self.files.len();
                    self.files.retain(|(_, abs)| !abs.starts_with(path));
                    self.ignored.retain(|ignored| !ignored.starts_with(path));
                    changed |= self.files.len() != before;
                },
                Ok(meta) if meta.is_file() || meta.is_dir() => {
                    if !self.covers(path) && !self.ignored.contains(path) {
                        self.probes.push(path.clone());
                        walk = true;
                    }
                },
                // A symlink: the walk never follows one, so nothing to add.
                Ok(_) => {},
            }
        }
        if walk && self.loaded {
            self.request_scan();
        }
        changed
    }

    /// Whether the list holds `path` itself, or a file under it.
    fn covers(&self, path: &Path) -> bool {
        let display = display_path(&self.root, path);
        let at = self
            .files
            .partition_point(|(d, _)| d.as_str() < display.as_str());
        self.files
            .get(at..)
            .unwrap_or_default()
            .iter()
            .take_while(|(d, _)| d.starts_with(display.as_str()))
            .any(|(_, abs)| abs.starts_with(path))
    }

    /// The list for a quick-open: `recent` files first (newest first, only
    /// those still listed), then every other file in path order. The fuzzy
    /// ranking keeps this order between equal scores, so recency also breaks
    /// ties once a query is typed.
    #[must_use]
    pub(crate) fn ranked(&self, recent: &[PathBuf]) -> Vec<(String, PathBuf)> {
        let mut seen = HashSet::new();
        let mut out = Vec::with_capacity(self.files.len());
        for path in recent {
            let display = display_path(&self.root, path);
            if let Ok(at) = self.files.binary_search_by(|(d, _)| d.cmp(&display))
                && let Some(entry) = self.files.get(at)
                && seen.insert(at)
            {
                out.push(entry.clone());
            }
        }
        out.extend(
            self.files
                .iter()
                .enumerate()
                .filter(|(at, _)| !seen.contains(at))
                .map(|(_, entry)| entry.clone()),
        );
        out
    }
}

/// Whether a workspace walk could list `relative`: hidden entries and the
/// pruned heavyweight dirs never appear, so changes there cost nothing.
fn walked(relative: &Path) -> bool {
    relative.components().all(|component| {
        component.as_os_str().to_str().is_none_or(|name| {
            !name.starts_with('.') && !karet_search::IGNORED_DIRS.contains(&name)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary workspace holding `files` (relative paths, created with
    /// their parent directories).
    fn workspace(files: &[&str]) -> tempfile::TempDir {
        // A visible name: `tempdir()`'s default `.tmp` prefix would make the
        // workspace root itself a hidden directory.
        let dir = tempfile::Builder::new()
            .prefix("karet-index")
            .tempdir()
            .expect("temp dir");
        for file in files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, "x");
        }
        dir
    }

    /// Wait for the walk in flight and adopt it.
    fn land(index: &mut FileIndex, rx: &mut mpsc::UnboundedReceiver<Scan>) -> bool {
        match rx.blocking_recv() {
            Some(scan) => index.accept(scan),
            None => false,
        }
    }

    fn displays(index: &FileIndex) -> Vec<&str> {
        index.files().iter().map(|(d, _)| d.as_str()).collect()
    }

    fn loaded(files: &[&str]) -> (tempfile::TempDir, FileIndex, mpsc::UnboundedReceiver<Scan>) {
        let dir = workspace(files);
        let mut index = FileIndex::new(dir.path().to_path_buf());
        let mut rx = index.take_receiver().expect("receiver");
        index.ensure_started();
        assert!(land(&mut index, &mut rx));
        (dir, index, rx)
    }

    #[test]
    fn the_first_walk_runs_off_thread_and_is_uncapped() {
        let names: Vec<String> = (0..2500).map(|n| format!("f{n:04}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let dir = workspace(&refs);
        let mut index = FileIndex::new(dir.path().to_path_buf());
        let mut rx = index.take_receiver().expect("receiver");
        assert!(
            index.loading().is_none(),
            "nothing walks before it is asked"
        );
        index.ensure_started();
        assert!(
            index.loading().is_some(),
            "the first walk is a loading state"
        );
        index.ensure_started();
        assert!(land(&mut index, &mut rx));
        assert!(index.loading().is_none());
        assert_eq!(index.files().len(), 2500, "the old 2000-file cap is gone");
        assert!(
            rx.try_recv().is_err(),
            "a second ensure_started walks nothing"
        );
    }

    #[test]
    fn a_removed_file_or_directory_leaves_the_list_at_once() {
        let (dir, mut index, _rx) = loaded(&["a.rs", "src/b.rs", "src/c.rs", "srcx.rs"]);
        let _ = std::fs::remove_file(dir.path().join("a.rs"));
        assert!(index.on_fs_changed(&[dir.path().join("a.rs")]));
        let _ = std::fs::remove_dir_all(dir.path().join("src"));
        assert!(index.on_fs_changed(&[dir.path().join("src")]));
        assert_eq!(displays(&index), ["srcx.rs"]);
        assert!(index.scanning.is_none(), "a removal needs no walk");
    }

    #[test]
    fn a_created_file_walks_again_and_lands() {
        let (dir, mut index, mut rx) = loaded(&["a.rs"]);
        let _ = std::fs::write(dir.path().join("b.rs"), "x");
        assert!(!index.on_fs_changed(&[dir.path().join("b.rs")]));
        assert!(index.scanning.is_some());
        assert!(
            index.loading().is_none(),
            "a refresh keeps the good list showing"
        );
        assert!(land(&mut index, &mut rx));
        assert_eq!(displays(&index), ["a.rs", "b.rs"]);
    }

    #[test]
    fn modifying_a_listed_file_walks_nothing() {
        let (dir, mut index, _rx) = loaded(&["a.rs", "src/b.rs"]);
        assert!(!index.on_fs_changed(&[dir.path().join("a.rs"), dir.path().join("src")]));
        assert!(index.scanning.is_none());
    }

    #[test]
    fn hidden_pruned_and_outside_paths_walk_nothing() {
        let (dir, mut index, _rx) = loaded(&["a.rs", ".git/index", "target/x", ".env"]);
        assert_eq!(displays(&index), ["a.rs"]);
        let changed = index.on_fs_changed(&[
            dir.path().join(".git/index"),
            dir.path().join("target/x"),
            dir.path().join(".env"),
            std::env::temp_dir(),
        ]);
        assert!(!changed);
        assert!(index.scanning.is_none());
    }

    #[test]
    fn creations_during_a_walk_coalesce_into_one_more() {
        let (dir, mut index, mut rx) = loaded(&["a.rs"]);
        let _ = std::fs::write(dir.path().join("b.rs"), "x");
        index.on_fs_changed(&[dir.path().join("b.rs")]);
        let _ = std::fs::write(dir.path().join("c.rs"), "x");
        index.on_fs_changed(&[dir.path().join("c.rs")]);
        let _ = std::fs::write(dir.path().join("d.rs"), "x");
        index.on_fs_changed(&[dir.path().join("d.rs")]);
        assert!(index.rescan);
        assert!(land(&mut index, &mut rx));
        assert!(
            index.scanning.is_some(),
            "the owed walk starts as the first lands"
        );
        assert!(land(&mut index, &mut rx));
        assert_eq!(displays(&index), ["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert!(index.scanning.is_none());
    }

    #[test]
    fn an_ignored_file_walks_once_until_the_ignore_file_changes() {
        let (dir, mut index, mut rx) = loaded(&["a.rs", ".gitignore"]);
        let _ = std::fs::write(dir.path().join(".gitignore"), "*.log\n");
        assert!(!index.on_fs_changed(&[dir.path().join(".gitignore")]));
        assert!(land(&mut index, &mut rx));
        let log = dir.path().join("build.log");
        let _ = std::fs::write(&log, "x");
        index.on_fs_changed(std::slice::from_ref(&log));
        assert!(land(&mut index, &mut rx));
        assert_eq!(displays(&index), ["a.rs"]);
        index.on_fs_changed(std::slice::from_ref(&log));
        assert!(
            index.scanning.is_none(),
            "a known-ignored path walks no more"
        );
        let _ = std::fs::write(dir.path().join(".gitignore"), "");
        index.on_fs_changed(&[dir.path().join(".gitignore")]);
        assert!(land(&mut index, &mut rx));
        assert_eq!(displays(&index), ["a.rs", "build.log"]);
    }

    #[test]
    fn a_walk_for_another_root_is_dropped() {
        let (_dir, mut index, _rx) = loaded(&["a.rs"]);
        let stale = Scan {
            root: PathBuf::from("/elsewhere"),
            files: Vec::new(),
        };
        assert!(!index.accept(stale));
        assert_eq!(displays(&index), ["a.rs"]);
    }

    #[test]
    fn recent_files_rank_first_newest_first_without_duplicates() {
        let (dir, index, _rx) = loaded(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        let recent = [
            dir.path().join("c.rs"),
            dir.path().join("gone.rs"),
            dir.path().join("a.rs"),
            dir.path().join("c.rs"),
        ];
        let ranked: Vec<String> = index.ranked(&recent).into_iter().map(|(d, _)| d).collect();
        assert_eq!(ranked, ["c.rs", "a.rs", "b.rs", "d.rs"]);
    }

    #[test]
    fn walked_skips_hidden_and_pruned_components() {
        assert!(walked(Path::new("src/main.rs")));
        assert!(!walked(Path::new("node_modules/x/index.js")));
        assert!(!walked(Path::new("src/.cache/x")));
    }
}
