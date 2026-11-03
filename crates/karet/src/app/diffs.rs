use std::path::Path;
use std::path::PathBuf;

use karet_core::NotificationKind;
use karet_session::PreparedChange;
use karet_session::RequestId;
use karet_session::VcsCommand;
use karet_session::ViewId;
use karet_vcs::StatusKind;

use crate::app::App;
use crate::app::notifications::Report;
use crate::pending::Pending;
use crate::tab::ChangedFile;
use crate::tab::PagerState;
use crate::tab::Section;
use crate::tab::TabKind;
use crate::tab::ViewMode;

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

/// The 0-based `\n`-delimited line of `text` that holds the start of editor row
/// `row`, the line numbering a diff of `text` itself uses.
///
/// The editor also breaks rows on a lone `\r`, VT, FF, NEL, LS and PS
/// ([`karet_text::lines`]), which the diff does not, so past such a break an
/// editor row and a diff line part ways. A row past the last line (the empty
/// row after a final break) maps to the line after every `\n`.
pub(super) fn diff_line_of_row(text: &str, row: u32) -> u32 {
    let start = usize::try_from(row)
        .ok()
        .and_then(|row| karet_text::lines(text).nth(row))
        .map_or(text.len(), |line| {
            line.as_ptr().addr().saturating_sub(text.as_ptr().addr())
        });
    let newlines = text.get(..start).unwrap_or(text).matches('\n').count();
    u32::try_from(newlines).unwrap_or(u32::MAX)
}

/// The display row, in `layout`, of the hunk a change marker on 0-based diff
/// line `line` (see [`diff_line_of_row`]) stands for: the first hunk whose new
/// side reaches that line, covering it or (a pure deletion) sitting just below
/// it. A line past every
/// hunk lands on the last one; a binary or hunkless diff has no row.
fn landing_row(prepared: &karet_diff::PreparedDiff, layout: ViewMode, line: u32) -> Option<usize> {
    let line = line.saturating_add(1);
    let hunks = &prepared.diff().hunks;
    let index = hunks
        .iter()
        .position(|hunk| {
            // A hunk with no new-side lines sits on the line above its gap,
            // which `new_start` names.
            hunk.new_start.saturating_add(hunk.new_count.max(1)) > line
        })
        .or_else(|| hunks.len().checked_sub(1))?;
    match layout {
        ViewMode::Unified => karet_diff::unified_hunk_row(prepared, index),
        ViewMode::SideBySide => karet_diff::side_by_side_hunk_row(prepared, index),
    }
}

impl App {
    /// Ask the backend to prepare `summary`'s displayable diff. The answering
    /// [`SessionEvent::ChangePrepared`] fills the reserved tab (matched by path
    /// and section, so a stale answer for a closed tab is dropped).
    pub(super) fn request_change_diff(&mut self, path: PathBuf, section: Section) {
        let staged = section == Section::Staged;
        if self
            .send(VcsCommand::PrepareChange { path, staged })
            .is_none()
        {
            self.notify(
                Report::Failure,
                NotificationKind::Vcs,
                "diff backend is unavailable",
            );
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
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "stage hunk: open a diff first",
            );
            return;
        };
        // Staged-side diffs un-stage; working-side diffs stage. Mismatched
        // verbs are a no-op with a hint rather than a surprising inversion.
        let staged_side = *section == Section::Staged;
        if reverse != staged_side {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                if reverse {
                    "unstage hunk: this diff shows unstaged changes (press s)"
                } else {
                    "stage hunk: this diff shows staged changes (press u)"
                },
            );
            return;
        }
        let prepared = &file.change.diff;
        let row = usize::from(pager.scroll);
        let hunk_index = match view {
            ViewMode::Unified => karet_diff::unified_hunk_at_row(prepared, row),
            ViewMode::SideBySide => karet_diff::side_by_side_hunk_at_row(prepared, row),
        };
        let Some(hunk) = hunk_index.and_then(|index| prepared.diff().hunks.get(index)) else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "stage hunk: no hunk here",
            );
            return;
        };
        let patch = karet_diff::format_hunk_patch(prepared.diff(), hunk);
        let (path, section) = (path.clone(), *section);
        if self
            .send(VcsCommand::ApplyIndexPatch { patch, reverse })
            .is_none()
        {
            self.notify(
                Report::Failure,
                NotificationKind::Vcs,
                "stage hunk: backend is unavailable",
            );
            return;
        }
        self.notify(
            Report::Outcome,
            NotificationKind::Vcs,
            if reverse {
                "hunk unstaged"
            } else {
                "hunk staged"
            },
        );
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
                    *file = Some(Box::new(ChangedFile::new(*prepared)));
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
    /// request. A failure closes the reserved tab and reports the reason as a
    /// notification instead — a diff that could not even be computed should not
    /// linger as a dead tab. A closed tab drops the answer. A request carrying
    /// a landing line scrolls the filled tab so that line's hunk is at the
    /// top of the viewport, in the tab's current layout.
    pub(super) fn apply_diff_prepared(
        &mut self,
        id: Option<RequestId>,
        result: Result<Box<PreparedChange>, String>,
    ) {
        let Some((view, landing)) = id.and_then(|id| self.pending_prepared_diffs.remove(&id))
        else {
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
                        view: layout,
                        pager,
                        ..
                    } = &mut tab.kind
                    {
                        if let Some(row) =
                            landing.and_then(|line| landing_row(&prepared.diff, *layout, line))
                        {
                            pager.scroll = u16::try_from(row).unwrap_or(u16::MAX);
                        }
                        *file = Some(Box::new(ChangedFile::new(*prepared)));
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
                self.notify(Report::Failure, NotificationKind::Vcs, message);
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
        // The tab keeps its view id while it is re-targeted, so a revision or two-file
        // diff still being prepared for it would otherwise land on the new file: the
        // superseded request is dropped before the tab changes underneath it.
        let replaced = self.tabs[self.active].view;
        self.cancel_loading_for_views(&std::collections::HashSet::from([replaced]));
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
