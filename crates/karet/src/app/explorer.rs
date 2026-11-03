use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use karet_core::NotificationKind;
use karet_filetype::IconStyle;
use karet_session::RequestId;
use karet_session::VcsCommand;
use karet_session::api::PathMutation;
use karet_vcs::RepositorySummary;
use karet_widgets::PendingEdit;
use karet_widgets::Spinner;

use crate::app::App;
use crate::app::confirm::ConfirmAction;
use crate::app::confirm::describe_paths;
use crate::app::explorer_mutate::FollowUp;
use crate::app::notifications::Report;
use crate::app::state::ContextMenu;
use crate::app::state::ContextMenuEntry;
use crate::app::state::ExplorerFileClipboard;
use crate::app::state::ExplorerFileOp;
use crate::command::Command;
use crate::keymap::SidebarPanel;
use crate::paths::path_under;
use crate::paths::same_path;
use crate::pending::Pending;

impl App {
    /// The absolute explorer row index under the hover cursor, if the mouse is over
    /// the explorer's content starting at row `top` (accounts for the current
    /// scroll offset). The painter passes the row it is laying the tree out at,
    /// which last frame's geometry may not match yet.
    pub(crate) fn hovered_explorer_row(&self, top: u16) -> Option<usize> {
        let (_, hy) = self.hover?;
        (hy >= top).then(|| self.explorer.offset() + usize::from(hy - top))
    }

    /// Start background status reads for nested repository rows not already cached.
    pub(crate) fn request_nested_repository_statuses(&mut self) {
        self.build_explorer();
        let paths: Vec<PathBuf> = self
            .explorer
            .rows()
            .iter()
            .filter(|row| row.is_repository)
            .map(|row| row.path.clone())
            .collect();
        for path in paths {
            if self.nested_repository_status.contains_key(&path)
                || self
                    .nested_repository_pending
                    .values()
                    .any(|(pending, _)| pending == &path)
            {
                continue;
            }
            if let Some(request) =
                self.send(VcsCommand::NestedRepositoryStatus { path: path.clone() })
            {
                self.nested_repository_pending
                    .insert(request, (path, Pending::start()));
            }
        }
    }

    /// Right-aligned status badges for nested repository rows. Fast pending reads
    /// stay blank; slower reads animate after the shared reveal delay.
    pub(crate) fn nested_repository_badges(&self, now: Instant) -> Vec<(PathBuf, String)> {
        let mut badges: Vec<(PathBuf, String)> = self
            .nested_repository_status
            .iter()
            .filter(|(_, summary)| !summary.is_clean())
            .map(|(path, summary)| {
                (
                    path.clone(),
                    repository_summary_label(*summary, self.icon_style),
                )
            })
            .collect();
        badges.extend(
            self.nested_repository_pending
                .values()
                .filter(|(_, since)| since.visible())
                .map(|(path, since)| {
                    (
                        path.clone(),
                        Spinner::new(self.icon_style)
                            .frame(since.elapsed_since(now))
                            .to_string(),
                    )
                }),
        );
        badges.sort_by(|a, b| a.0.cmp(&b.0));
        badges
    }

    /// Next repaint needed to reveal or animate a nested-repository loading badge.
    pub(crate) fn nested_repository_next_wake(&self, now: Instant) -> Option<Duration> {
        if !self.sidebar_visible || self.sidebar_panel != SidebarPanel::Explorer {
            return None;
        }
        self.nested_repository_pending
            .values()
            .map(|(_, since)| since.wake(now).unwrap_or(Spinner::FRAME_INTERVAL))
            .min()
    }

    /// Drop cached summaries affected by changed worktree paths and cancel any
    /// matching in-flight reads. The next Explorer frame requests fresh values.
    pub(crate) fn invalidate_nested_repository_statuses(&mut self, changed: &[PathBuf]) {
        self.nested_repository_status
            .retain(|repository, _| !changed.iter().any(|path| path.starts_with(repository)));
        let cancelled: Vec<RequestId> = self
            .nested_repository_pending
            .iter()
            .filter(|(_, (repository, _))| changed.iter().any(|path| path.starts_with(repository)))
            .map(|(request, _)| *request)
            .collect();
        for request in cancelled {
            self.nested_repository_pending.remove(&request);
            self.cancel_backend_request(request);
        }
    }

    /// Begin creating a new file (or folder) in the explorer, ensuring the panel is
    /// visible and focused so its inline name editor is shown.
    pub(super) fn explorer_begin_new(&mut self, folder: bool) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.keyboard.focus_sidebar();
        self.build_explorer();
        self.explorer.begin_new(folder);
    }

    /// Begin renaming the selected explorer entry (no-op unless the Explorer panel is
    /// the active sidebar panel).
    pub(super) fn explorer_begin_rename(&mut self) {
        if self.sidebar_panel != SidebarPanel::Explorer {
            return;
        }
        self.build_explorer();
        self.explorer.begin_rename();
    }

    /// Hard-reload the explorer tree and re-request VCS status — a bullet-proof
    /// refresh that drops every cached row and re-reads the filesystem.
    pub(super) fn explorer_refresh(&mut self) {
        self.rebuild_explorer();
        self.refresh_listings();
        self.nested_repository_status.clear();
        let pending: Vec<RequestId> = self.nested_repository_pending.keys().copied().collect();
        self.nested_repository_pending.clear();
        for request in pending {
            self.cancel_backend_request(request);
        }
        self.send_command(VcsCommand::Refresh);
    }

    /// Apply the explorer inline edit: ask the backend to create the file/folder
    /// or rename it; the tree reloads (and a newly-created file opens) when the
    /// answer lands.
    pub(super) fn explorer_commit_edit(&mut self) {
        let Some(pending) = self.explorer.take_edit() else {
            return;
        };
        let edit = Box::new(pending.clone());
        match pending {
            PendingEdit::Create { path, folder } => {
                let mutation = if folder {
                    PathMutation::CreateDirectory { path: path.clone() }
                } else {
                    PathMutation::CreateFile { path: path.clone() }
                };
                let follow_up = if folder {
                    FollowUp::CreatedFolder { edit }
                } else {
                    FollowUp::CreatedFile { path, edit }
                };
                self.mutate_path(mutation, follow_up);
            },
            PendingEdit::Rename { from, to } => {
                self.mutate_path(
                    PathMutation::Rename {
                        from: from.clone(),
                        to: to.clone(),
                    },
                    FollowUp::Renamed { from, to, edit },
                );
            },
        }
    }

    /// Copy the explorer's selected files/directories into the internal file
    /// clipboard.
    pub(super) fn explorer_copy_files(&mut self) {
        self.explorer_store_files(ExplorerFileOp::Copy);
    }

    /// Cut the explorer's selected files/directories into the internal file
    /// clipboard.
    pub(super) fn explorer_cut_files(&mut self) {
        self.explorer_store_files(ExplorerFileOp::Cut);
    }

    /// Store the current explorer selection as the source for a future paste.
    pub(super) fn explorer_store_files(&mut self, op: ExplorerFileOp) {
        self.build_explorer();
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "explorer: select a file first",
            );
            return;
        }
        let count = paths.len();
        self.explorer_clipboard = Some(ExplorerFileClipboard { op, paths });
        let verb = match op {
            ExplorerFileOp::Copy => "copied",
            ExplorerFileOp::Cut => "cut",
        };
        self.notify(
            Report::Outcome,
            NotificationKind::Io,
            format!("{verb} {count} explorer item(s)"),
        );
    }

    /// Paste the internal explorer file clipboard into the selected destination.
    ///
    /// Every item is its own request: one failure reports itself and the rest go
    /// on, which is what a user pasting a dozen files expects. The checks that
    /// remain here are the ones answerable from paths alone; whether a source
    /// still exists, and whether a destination is free, are the backend's to
    /// answer — it refuses to clobber, so a name the explorer had not heard of
    /// fails loudly instead of overwriting. Each success reports the running
    /// count on one card (see `explorer_mutate`).
    pub(super) fn explorer_paste_files(&mut self) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "paste: no explorer files",
            );
            return;
        };
        let dest_dir = self.explorer_paste_destination();
        let mut taken = self.names_known_taken();

        let mut skipped = 0usize;
        let mut requested = 0usize;
        let mut first_error: Option<String> = None;
        self.explorer_paste_done = 0;

        for source in &clipboard.paths {
            // A cut into the directory the file already sits in is a no-op, not
            // an error.
            if clipboard.op == ExplorerFileOp::Cut
                && source
                    .parent()
                    .is_some_and(|parent| same_path(parent, &dest_dir))
            {
                skipped += 1;
                continue;
            }
            if path_under(source, &dest_dir) {
                first_error.get_or_insert_with(|| {
                    format!(
                        "paste failed: cannot paste {} into itself",
                        source.display()
                    )
                });
                continue;
            }

            let target = unique_child_path(&dest_dir, source, &taken);
            // Reserve the name so two sources with the same file name do not both
            // choose it in one paste.
            taken.insert(target.clone());
            let mutation = match clipboard.op {
                ExplorerFileOp::Copy => PathMutation::Copy {
                    from: source.clone(),
                    to: target.clone(),
                },
                ExplorerFileOp::Cut => PathMutation::Rename {
                    from: source.clone(),
                    to: target.clone(),
                },
            };
            let moved = (clipboard.op == ExplorerFileOp::Cut).then(|| (source.clone(), target));
            self.mutate_path(
                mutation,
                FollowUp::Pasted {
                    moved,
                    taken: taken.clone(),
                    attempts: 0,
                },
            );
            requested += 1;
        }

        if requested > 0 && clipboard.op == ExplorerFileOp::Cut {
            self.explorer_clipboard = None;
        }
        if let Some(message) = first_error {
            self.notify(Report::Failure, NotificationKind::Io, message);
        } else if requested == 0 && skipped > 0 {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "paste: already in target folder",
            );
        }
    }

    /// The explorer paste target: selected directory, selected file's parent, or root.
    pub(super) fn explorer_paste_destination(&mut self) -> PathBuf {
        self.build_explorer();
        match self.explorer.selected() {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        }
    }

    /// The explorer's selected paths after ensuring its row cache is current.
    pub(super) fn explorer_selected_paths(&mut self) -> Vec<PathBuf> {
        self.build_explorer();
        self.explorer.selected_paths()
    }

    /// The paths currently dimmed as cut in the explorer.
    pub(crate) fn explorer_cut_paths(&self) -> &[PathBuf] {
        self.explorer_clipboard
            .as_ref()
            .filter(|clipboard| clipboard.op == ExplorerFileOp::Cut)
            .map_or(&[], |clipboard| clipboard.paths.as_slice())
    }

    /// Duplicate the selected explorer item(s) beside themselves.
    pub(super) fn explorer_duplicate_files(&mut self) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "duplicate: select a file first",
            );
            return;
        }
        let mut taken = self.names_known_taken();
        // Reset before submitting, matching paste: the running count belongs to
        // this batch, not the one before it.
        self.explorer_paste_done = 0;
        for source in paths {
            let Some(parent) = source.parent() else {
                continue;
            };
            let target = unique_child_path(parent, &source, &taken);
            taken.insert(target.clone());
            self.mutate_path(
                PathMutation::Copy {
                    from: source.clone(),
                    to: target,
                },
                FollowUp::Pasted {
                    moved: None,
                    taken: taken.clone(),
                    attempts: 0,
                },
            );
        }
    }

    /// The paths a paste or duplicate already knows are taken: every row the
    /// explorer shows, and every destination a mutation still in flight is
    /// about to create.
    ///
    /// Only a first guess. A collapsed folder, a hidden or unlisted name, or a
    /// file created outside the editor is not in it; the backend refuses those,
    /// and the paste then tries the next free name (see `explorer_mutate`).
    fn names_known_taken(&self) -> BTreeSet<PathBuf> {
        let in_flight = self
            .pending_mutations
            .values()
            .filter_map(|pending| match &pending.mutation {
                PathMutation::Copy { to, .. } | PathMutation::Rename { to, .. } => Some(to),
                PathMutation::CreateFile { path } | PathMutation::CreateDirectory { path } => {
                    Some(path)
                },
                _ => None,
            })
            .cloned();
        self.explorer
            .rows()
            .iter()
            .map(|row| row.path.clone())
            .chain(in_flight)
            .collect()
    }

    /// Copy selected explorer paths to the system clipboard.
    pub(super) fn explorer_copy_path(&mut self, relative: bool) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "copy path: select a file first",
            );
            return;
        }
        let text = paths
            .iter()
            .map(|path| {
                let display = if relative {
                    path.strip_prefix(&self.root).unwrap_or(path)
                } else {
                    path.as_path()
                };
                display.to_string_lossy().into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.copy_to_clipboard(text, "path");
    }

    /// Arm deletion of the selected explorer item(s).
    pub(super) fn explorer_arm_delete(&mut self) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "delete: select a file first",
            );
            return;
        }
        if self.has_dirty_tabs_under(&paths) {
            self.notify(
                Report::Refusal,
                NotificationKind::Io,
                "delete blocked: save or close dirty files first",
            );
            return;
        }
        self.context_menu_clear();
        let body = format!(
            "Permanently removes {} from disk. Folders are deleted with everything \
             inside them. This cannot be undone.",
            describe_paths(&paths, &self.root)
        );
        let title = format!("Delete {} item(s)?", paths.len());
        self.confirm_action(
            title,
            body,
            "Keep",
            "Delete",
            ConfirmAction::DeleteExplorerPaths(paths),
        );
    }

    /// Delete `paths` from disk, once confirmed.
    pub(super) fn delete_explorer_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.close_tabs_under(&paths);
        // Each path is its own request, so one failure does not abandon the rest —
        // "delete what you can, report what you cannot", with the reporting
        // arriving per answer.
        self.explorer_delete_done = 0;
        for path in paths {
            self.mutate_path(PathMutation::Delete { path }, FollowUp::Deleted);
        }
    }

    pub(super) fn row_context_items(&self) -> Vec<ContextMenuEntry> {
        [
            Command::SidebarActivate,
            Command::ExplorerRename,
            Command::ExplorerNewFile,
            Command::ExplorerNewFolder,
            Command::ExplorerCopy,
            Command::ExplorerCut,
            Command::ExplorerPaste,
            Command::ExplorerDuplicate,
            Command::ExplorerDelete,
            Command::ExplorerCopyPath,
            Command::ExplorerCopyRelativePath,
            Command::ExplorerRefresh,
        ]
        .into_iter()
        .map(ContextMenuEntry::enabled)
        .collect()
    }

    pub(super) fn blank_context_items(&self) -> Vec<ContextMenuEntry> {
        [
            Command::ExplorerNewFile,
            Command::ExplorerNewFolder,
            Command::ExplorerPaste,
            Command::ExplorerRefresh,
            Command::ExplorerCollapseAll,
        ]
        .into_iter()
        .map(ContextMenuEntry::enabled)
        .collect()
    }

    pub(super) fn open_context_menu(&mut self, x: u16, y: u16, row: Option<usize>) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.keyboard.focus_sidebar();
        self.build_explorer();
        let items = if let Some(row) = row {
            if !self.explorer.is_selected(row) {
                self.explorer.select_index(row);
            }
            self.row_context_items()
        } else {
            self.blank_context_items()
        };
        self.context_menu = Some(ContextMenu::new(x, y, items));
    }

    pub(super) fn open_context_menu_for_selection(&mut self) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.keyboard.focus_sidebar();
        self.build_explorer();
        let cursor = self.explorer.cursor();
        let (left, top) = self.explorer_content_origin();
        let y = top.saturating_add(
            cursor
                .saturating_sub(self.explorer.offset())
                .try_into()
                .unwrap_or(0),
        );
        let x = left.saturating_add(2);
        let row = (!self.explorer.rows().is_empty()).then_some(cursor);
        self.open_context_menu(x, y, row);
    }

    /// The cell the explorer tree's first row sits at: where the last frame
    /// painted it, or, when that frame painted no sidebar, where the next frame
    /// lays it out — column 0, below the view switcher (absent in Zen Mode) and
    /// the sidebar's one-row header, as [`ui::draw`] places it.
    fn explorer_content_origin(&self) -> (u16, u16) {
        let painted = self.last_frame.sidebar_content_rect;
        if painted.width > 0 && painted.height > 0 {
            return (painted.x, painted.y);
        }
        let view_switcher = u16::from(self.modes.zen.is_none());
        (0, view_switcher.saturating_add(1))
    }
}

fn repository_summary_label(summary: RepositorySummary, icons: IconStyle) -> String {
    let (up, down) = if icons == IconStyle::Ascii {
        ("^", "v")
    } else {
        ("↑", "↓")
    };
    let mut parts = Vec::new();
    if summary.ahead > 0 {
        parts.push(format!("{up}{}", summary.ahead));
    }
    if summary.behind > 0 {
        parts.push(format!("{down}{}", summary.behind));
    }
    if summary.added > 0 {
        parts.push(format!("+{}", summary.added));
    }
    if summary.removed > 0 {
        parts.push(format!("-{}", summary.removed));
    }
    parts.join(" ")
}

/// A destination path under `dir`, suffixing when the source name is already
/// `taken`.
///
/// `taken` is what the explorer has been told is in `dir`, not what a probe of the
/// filesystem would say — the files may be on another machine. A name the tree has
/// not heard of can still collide, and the backend refuses to clobber rather than
/// overwrite, so the worst case is a reported failure instead of lost data.
pub(super) fn unique_child_path(dir: &Path, source: &Path, taken: &BTreeSet<PathBuf>) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "item".to_string());
    let first = dir.join(&name);
    if !taken.contains(&first) {
        return first;
    }

    for n in 1usize.. {
        let candidate = dir.join(copy_name(source, &name, n));
        if !taken.contains(&candidate) {
            return candidate;
        }
    }

    unreachable!("unbounded suffix search should always return");
}

/// Build `name copy.ext`, `name copy 2.ext`, or `dir copy` style conflict names.
fn copy_name(source: &Path, fallback: &str, n: usize) -> String {
    let suffix = if n == 1 {
        " copy".to_string()
    } else {
        format!(" copy {n}")
    };
    if source.is_dir() {
        return format!("{fallback}{suffix}");
    }
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    match source.extension().map(|ext| ext.to_string_lossy()) {
        Some(ext) if !ext.is_empty() => format!("{stem}{suffix}.{ext}"),
        _ => format!("{stem}{suffix}"),
    }
}
