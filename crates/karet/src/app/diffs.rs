use super::*;

/// A trivially-constructible prepared change for a binary pair: an empty diff
/// flagged binary, so the painter shows the standard placeholder without a
/// backend round-trip.
pub(super) fn binary_prepared_change(path: &Path) -> PreparedChange {
    let mut diff = karet_diff::diff_text("", "", &karet_diff::DiffOptions::default());
    diff.is_binary = true;
    PreparedChange {
        path: path.to_path_buf(),
        old_path: None,
        status: StatusKind::Modified,
        language: karet_filetype::file_type_for_path(path).name().to_string(),
        diff: karet_diff::PreparedDiff::new(diff, Vec::new(), Vec::new()),
    }
}

impl App {
    /// Ask the backend to prepare `summary`'s displayable diff. The answering
    /// [`SessionEvent::ChangePrepared`] fills the reserved tab (matched by path
    /// and section, so a stale answer for a closed tab is dropped).
    pub(super) fn request_change_diff(&mut self, path: PathBuf, section: Section) {
        let staged = section == Section::Staged;
        if self
            .send_command_id(SessionCommand::PrepareChange { path, staged })
            .is_none()
        {
            self.status = Some("diff backend is unavailable".to_string());
        }
    }

    /// Fill the loading diff tab for `(path, section)` with its prepared file,
    /// or record the failure. Late answers for closed or re-targeted tabs are
    /// dropped by the match.
    pub(super) fn apply_change_prepared(
        &mut self,
        path: &Path,
        staged: bool,
        result: Result<Box<PreparedChange>, String>,
    ) {
        let section = if staged {
            Section::Staged
        } else {
            Section::Working
        };
        for tab in self.all_tabs_mut() {
            let TabKind::Diff {
                path: tab_path,
                section: tab_section,
                file,
                loading_since,
                error,
                ..
            } = &mut tab.kind
            else {
                continue;
            };
            if tab_path != path || *tab_section != section || file.is_some() {
                continue;
            }
            match result {
                Ok(prepared) => {
                    *file = Some(Box::new(FileView::new(*prepared)));
                    *loading_since = None;
                    *error = None;
                },
                Err(message) => {
                    *loading_since = None;
                    *error = Some(message);
                },
            }
            return;
        }
    }

    /// Fill the reserved ad-hoc diff tab (revision or two-file diff) owned by the
    /// request. A failure closes the reserved tab and reports through the status
    /// line instead — a diff that could not even be computed should not linger as
    /// a dead tab. A closed tab drops the answer.
    pub(super) fn apply_diff_prepared(
        &mut self,
        id: Option<RequestId>,
        result: Result<Box<PreparedChange>, String>,
    ) {
        let Some(view) = id.and_then(|id| self.pending_prepared_diffs.remove(&id)) else {
            return;
        };
        match result {
            Ok(prepared) => {
                for tab in self.all_tabs_mut() {
                    if tab.view != view {
                        continue;
                    }
                    if let TabKind::Diff {
                        file,
                        loading_since,
                        error,
                        ..
                    } = &mut tab.kind
                    {
                        *file = Some(Box::new(FileView::new(*prepared)));
                        *loading_since = None;
                        *error = None;
                    }
                    return;
                }
            },
            Err(message) => {
                if let Some(index) = self.tabs.iter().position(|tab| {
                    tab.view == view && matches!(tab.kind, TabKind::Diff { file: None, .. })
                }) {
                    self.close_tab_at(index);
                }
                self.status = Some(message);
            },
        }
    }

    /// Record a failure on the loading diff tab owned by `view` (e.g. the
    /// backend was unavailable to even send the request).
    pub(super) fn fail_diff_tab(&mut self, view: ViewId, message: &str) {
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            if let TabKind::Diff {
                loading_since,
                error,
                ..
            } = &mut tab.kind
            {
                *loading_since = None;
                *error = Some(message.to_string());
            }
            return;
        }
    }

    /// Toggle the active diff tab between unified and side-by-side.
    pub(super) fn toggle_diff_layout(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && let TabKind::Diff { view, scroll, .. } = &mut tab.kind
        {
            *view = match *view {
                ViewMode::Unified => ViewMode::SideBySide,
                ViewMode::SideBySide => ViewMode::Unified,
            };
            *scroll = 0;
            // Remember the choice so subsequently-opened diffs adopt it.
            self.diff_layout = *view;
        }
    }

    /// Replace the active diff tab with the next/previous changed file.
    pub(super) fn step_changed_file(&mut self, delta: i32) {
        if let Some(TabKind::Commit { files, view, .. } | TabKind::Compare { files, view, .. }) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            if files.is_empty() || view.file_anchors.is_empty() {
                return;
            }
            let current = view
                .file_anchors
                .iter()
                .rposition(|anchor| *anchor <= view.scroll);
            let next = current.map_or(0, |file| {
                (file as i64 + i64::from(delta))
                    .clamp(0, view.file_anchors.len().saturating_sub(1) as i64)
                    as usize
            });
            view.scroll = view.file_anchors[next];
            return;
        }
        if !self.active_is_diff() {
            return;
        }
        let len = self.scm.changes.len();
        if len == 0 {
            return;
        }
        let next = (self.scm.selection.cursor() as i64 + i64::from(delta)).clamp(0, len as i64 - 1)
            as usize;
        self.scm.selection.move_to(next);
        let view = match &self.tabs[self.active].kind {
            TabKind::Diff { view, .. } => *view,
            _ => ViewMode::Unified,
        };
        let change = self.scm.changes[next].clone();
        let section = self.scm.section(next);
        let title = change
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diff")
            .to_string();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.title = title;
            tab.kind = TabKind::Diff {
                path: change.path.clone(),
                section,
                file: None,
                loading_since: Some(Instant::now()),
                error: None,
                view,
                scroll: 0,
                column: 0,
            };
            self.request_change_diff(change.path, section);
        }
    }
}
