//! Quick-open (go to file) and Open Recent (workspaces).
//!
//! The picker opens at once over the [`FileIndex`](workspace::FileIndex), which
//! walks the workspace off the UI thread and follows filesystem changes; a walk
//! still in flight fills an open picker in when it lands. Recently opened files
//! rank first, and the workspaces karet was opened on are offered by Open Recent,
//! both kept in the user state directory by [`RecentStore`](workspace::RecentStore).

use super::*;

impl App {
    /// Open the quick-open (go-to-file) overlay at once, over whatever the file
    /// index holds; a walk still in flight fills it in when it lands.
    pub(super) fn open_quick_open(&mut self) {
        self.file_index.ensure_started();
        self.overlay = Some(self.quick_open_overlay());
    }

    /// The quick-open picker: recently opened files first, then the rest.
    fn quick_open_overlay(&self) -> Overlay {
        // The store keeps absolute paths; the index lists them under the root as
        // it was given (`.` for a bare `karet`), so rank them in that form.
        let absolute_root = workspace::absolute(&self.root);
        let recent: Vec<PathBuf> = self
            .recent
            .files(&self.root)
            .iter()
            .filter_map(|file| file.strip_prefix(&absolute_root).ok())
            .map(|relative| self.root.join(relative))
            .collect();
        Overlay::quick_open(self.file_index.ranked(&recent))
    }

    /// The first walk of the workspace while a quick-open waits on it: what its
    /// delayed "indexing" placeholder anchors on.
    pub(crate) fn quick_open_loading(&self) -> Option<Pending> {
        self.overlay
            .as_ref()
            .filter(|overlay| overlay.is_quick_open())
            .and_then(|_| self.file_index.loading())
    }

    /// Adopt a finished workspace walk, refreshing an open quick-open.
    pub(super) fn on_file_index(&mut self, scan: workspace::Scan) {
        if self.file_index.accept(scan) {
            self.refresh_quick_open();
        }
    }

    /// Follow filesystem changes into the file index.
    pub(super) fn file_index_fs_changed(&mut self, paths: &[PathBuf]) {
        if self.file_index.on_fs_changed(paths) {
            self.refresh_quick_open();
        }
    }

    /// Rebuild an open quick-open over the current list, keeping what the user
    /// typed and, where it is still listed, the row they had highlighted.
    fn refresh_quick_open(&mut self) {
        let Some(open) = self.overlay.as_ref().filter(|o| o.is_quick_open()) else {
            return;
        };
        let query = open.query().to_string();
        let selected = open.selected_file().map(Path::to_path_buf);
        let mut overlay = self.quick_open_overlay();
        overlay.push_str(&query);
        if let Some(selected) = selected {
            overlay.select_file(&selected);
        }
        self.overlay = Some(overlay);
    }

    /// Remember that `path` was opened, for quick-open's recency ranking. Only
    /// files in this workspace are ranked, so only those are remembered.
    pub(super) fn note_recent_file(&mut self, path: &Path) {
        self.recent.touch_file(&self.root, path);
    }

    /// Open the Open Recent picker over the other remembered workspaces that
    /// still exist.
    pub(super) fn open_recent_workspace(&mut self) {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
        let this = workspace::absolute(&self.root);
        let items: Vec<(String, PathBuf)> = self
            .recent
            .workspaces()
            .filter(|root| *root != this && root.is_dir())
            .map(|root| (tilde(root, home.as_deref()), root.to_path_buf()))
            .collect();
        if items.is_empty() {
            self.notify(
                Report::Outcome,
                NotificationKind::System,
                "open recent: no other recent workspaces",
            );
            return;
        }
        self.overlay = Some(Overlay::recent_workspaces(items));
    }

    /// Reopen karet on `root`: quit this window (through the usual unsaved
    /// changes prompt), then start again there once the terminal is restored.
    pub(super) fn reopen_on_workspace(&mut self, root: PathBuf) {
        if !root.is_dir() {
            self.notify(
                Report::Failure,
                NotificationKind::System,
                format!("open recent: {} is no longer a directory", root.display()),
            );
            return;
        }
        self.guarded_close(CloseRequest::Quit);
        self.relaunch = Some(workspace::absolute(&root));
    }

    /// The workspace to reopen on after quitting, if Open Recent asked for one.
    pub(super) fn take_relaunch(&mut self) -> Option<PathBuf> {
        self.relaunch.take()
    }

    /// Forget a pending reopen: the quit that would have carried it was cancelled.
    pub(super) fn cancel_relaunch(&mut self) {
        self.relaunch = None;
    }
}

/// `path` for display, with the home directory written `~`.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_shortens_only_paths_under_home() {
        let home = Path::new("/home/me");
        assert_eq!(tilde(Path::new("/home/me/src/k"), Some(home)), "~/src/k");
        assert_eq!(tilde(Path::new("/home/me"), Some(home)), "~");
        assert_eq!(tilde(Path::new("/home/meow"), Some(home)), "/home/meow");
        assert_eq!(tilde(Path::new("/srv/k"), None), "/srv/k");
    }
}
