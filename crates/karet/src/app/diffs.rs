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
            .send(SessionCommand::PrepareChange { path, staged })
            .is_none()
        {
            self.status = Some("diff backend is unavailable".to_string());
        }
    }

    /// Stage (`reverse: false`) or un-stage (`reverse: true`) the hunk at the
    /// top of the active diff tab's viewport, then re-request the diff so the
    /// tab reflects the moved hunk. The backend answers the apply with a fresh
    /// [`SessionEvent::VcsStatus`], which refreshes the Source-Control lists.
    pub(super) fn stage_hunk_at_viewport(&mut self, reverse: bool) {
        let Some(TabKind::Diff {
            path,
            section,
            file: Some(file),
            view,
            pager,
            ..
        }) = self.tabs.get(self.active).map(|tab| &tab.kind)
        else {
            self.status = Some("stage hunk: open a diff first".to_string());
            return;
        };
        // Staged-side diffs un-stage; working-side diffs stage. Mismatched
        // verbs are a no-op with a hint rather than a surprising inversion.
        let staged_side = *section == Section::Staged;
        if reverse != staged_side {
            self.status = Some(if reverse {
                "unstage hunk: this diff shows unstaged changes (press s)".to_string()
            } else {
                "stage hunk: this diff shows staged changes (press u)".to_string()
            });
            return;
        }
        let prepared = &file.change.diff;
        let row = usize::from(pager.scroll);
        let hunk_index = match view {
            ViewMode::Unified => karet_diff::unified_hunk_at_row(prepared, row),
            ViewMode::SideBySide => karet_diff::side_by_side_hunk_at_row(prepared, row),
        };
        let Some(hunk) = hunk_index.and_then(|index| prepared.diff.hunks.get(index)) else {
            self.status = Some("stage hunk: no hunk here".to_string());
            return;
        };
        let patch = karet_diff::format_hunk_patch(&prepared.diff, hunk);
        let (path, section) = (path.clone(), *section);
        if self
            .send(SessionCommand::ApplyIndexPatch { patch, reverse })
            .is_none()
        {
            self.status = Some("stage hunk: backend is unavailable".to_string());
            return;
        }
        self.status = Some(if reverse {
            "hunk unstaged".to_string()
        } else {
            "hunk staged".to_string()
        });
        // Reserve the tab's loading state and ask for the now-current diff.
        if let Some(TabKind::Diff {
            file,
            loading_since,
            ..
        }) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            *file = None;
            *loading_since = Some(Pending::start());
        }
        self.request_change_diff(path, section);
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
            && let TabKind::Diff { view, pager, .. } = &mut tab.kind
        {
            *view = match *view {
                ViewMode::Unified => ViewMode::SideBySide,
                ViewMode::SideBySide => ViewMode::Unified,
            };
            pager.scroll = 0;
            // Remember the choice so subsequently-opened diffs adopt it.
            self.diff_layout = *view;
        }
    }

    /// Replace the active diff tab with the next/previous changed file.
    pub(super) fn step_changed_file(&mut self, delta: i32) {
        if let Some(TabKind::Commit { files, view, .. } | TabKind::Compare { files, view, .. }) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            if files.files.is_empty() || view.file_anchors.is_empty() {
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
                loading_since: Some(Pending::start()),
                error: None,
                view,
                pager: PagerState::default(),
            };
            self.request_change_diff(change.path, section);
        }
    }
}
