use super::*;

impl App {
    /// Open the Source-Control cursor's change as a materialized view and focus it.
    pub(super) fn open_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        let conflict_path = Self::conflict_path(&change, section);
        if let Some(idx) = self.find_change_tab(&change, section) {
            let needs_conflict = conflict_path.is_some()
                && self.tabs[idx].merge_conflict.is_none()
                && matches!(self.tabs[idx].kind, TabKind::Code { .. });
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.is_preview = false;
                if needs_conflict {
                    tab.title = format!("⚠ {}", tab.title);
                    tab.merge_conflict = Some(MergeConflictState::loading());
                }
            }
            self.select_tab(idx);
            if needs_conflict && let Some(path) = conflict_path {
                self.request_merge_conflict(path);
            }
            return;
        }
        let tab = self.build_change_tab(change, section);
        let request = diff_request(&tab);
        self.push_tab(tab);
        if let Some((path, section)) = request {
            self.request_change_diff(path, section);
        }
        if let Some(path) = conflict_path {
            self.request_merge_conflict(path);
        }
    }

    /// Preview the Source-Control cursor's change without stealing sidebar focus.
    pub(super) fn preview_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        let conflict_path = Self::conflict_path(&change, section);
        if let Some(idx) = self.find_change_tab(&change, section) {
            let needs_conflict = conflict_path.is_some()
                && self.tabs[idx].merge_conflict.is_none()
                && matches!(self.tabs[idx].kind, TabKind::Code { .. });
            if needs_conflict && let Some(tab) = self.tabs.get_mut(idx) {
                tab.title = format!("⚠ {}", tab.title);
                tab.merge_conflict = Some(MergeConflictState::loading());
            }
            self.active = idx;
            self.find_open = false;
            if needs_conflict && let Some(path) = conflict_path {
                self.request_merge_conflict(path);
            }
            return;
        }
        let mut tab = self.build_change_tab(change, section);
        tab.is_preview = true;
        let request = diff_request(&tab);
        self.install_preview_tab(tab, false);
        if let Some((path, section)) = request {
            self.request_change_diff(path, section);
        }
        if let Some(path) = conflict_path {
            self.request_merge_conflict(path);
        }
    }

    fn conflict_path(change: &ChangeSummary, section: Section) -> Option<PathBuf> {
        (section == Section::Working && change.status == karet_vcs::StatusKind::Conflicted)
            .then(|| change.path.clone())
    }

    fn find_change_tab(&self, change: &ChangeSummary, section: Section) -> Option<usize> {
        if Self::conflict_path(change, section).is_some() {
            let expected = if change.path.is_absolute() {
                change.path.clone()
            } else {
                self.root.join(&change.path)
            };
            return self.tabs.iter().position(|tab| {
                matches!(tab.kind, TabKind::Code { .. })
                    && tab
                        .path()
                        .is_some_and(|path| canonical(path) == canonical(&expected))
            });
        }
        self.find_diff_tab(&change.path, section)
    }

    /// The existing regular diff tab for `path` in `section`, if any.
    pub(super) fn find_diff_tab(&self, path: &Path, section: Section) -> Option<usize> {
        self.tabs.iter().position(|tab| {
            matches!(&tab.kind, TabKind::Diff { path: tab_path, section: tab_section, .. }
                if *tab_path == *path && *tab_section == section)
        })
    }

    /// Build an editable conflict editor, or reserve an ordinary diff tab in its
    /// loading state (the caller sends the `PrepareChange` request after
    /// installing the tab — see [`diff_request`]).
    pub(super) fn build_change_tab(&self, change: ChangeSummary, section: Section) -> Tab {
        if let Some(path) = Self::conflict_path(&change, section) {
            let absolute = if path.is_absolute() {
                path
            } else {
                self.root.join(path)
            };
            let mut tab = workspace::open_file(&absolute);
            if matches!(tab.kind, TabKind::Code { .. }) {
                tab.title = format!("⚠ {}", tab.title);
                tab.merge_conflict = Some(MergeConflictState::loading());
                return tab;
            }
        }
        let title = change
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("diff")
            .to_string();
        Tab::diff(title, change.path, section, None, self.diff_layout)
    }

    fn request_merge_conflict(&mut self, path: PathBuf) {
        let view = self.tabs[self.active].view;
        if let Some(request) =
            self.send_command_id(SessionCommand::MergeConflict { path: path.clone() })
        {
            self.pending_merge_conflicts.insert(request, (view, path));
        } else if let Some(conflict) = self.tabs[self.active].merge_conflict.as_mut() {
            conflict.error = Some("merge-conflict backend is unavailable".to_string());
        }
    }
}

/// The `(path, section)` a freshly built loading diff tab needs prepared, if it
/// is one (a conflict editor needs no diff preparation).
fn diff_request(tab: &Tab) -> Option<(PathBuf, Section)> {
    match &tab.kind {
        TabKind::Diff {
            path,
            section,
            file: None,
            ..
        } => Some((path.clone(), *section)),
        _ => None,
    }
}
