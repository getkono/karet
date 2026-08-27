use karet_session::api::PathMutation;

use super::*;

impl App {
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
                self.send(SessionCommand::NestedRepositoryStatus { path: path.clone() })
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
        self.focus = Focus::Sidebar;
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
        self.nested_repository_status.clear();
        let pending: Vec<RequestId> = self.nested_repository_pending.keys().copied().collect();
        self.nested_repository_pending.clear();
        for request in pending {
            self.cancel_backend_request(request);
        }
        self.send_command(SessionCommand::RefreshVcs);
    }

    /// Apply the explorer inline edit: create the file/folder or rename on disk, then
    /// reload the tree (and open a newly-created file).
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
                    crate::app::explorer_mutate::FollowUp::CreatedFolder { edit }
                } else {
                    crate::app::explorer_mutate::FollowUp::CreatedFile { path, edit }
                };
                self.mutate_path(mutation, follow_up);
            },
            PendingEdit::Rename { from, to } => {
                self.mutate_path(
                    PathMutation::Rename {
                        from: from.clone(),
                        to: to.clone(),
                    },
                    crate::app::explorer_mutate::FollowUp::Renamed { from, to, edit },
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
            self.status = Some("explorer: select a file first".to_string());
            return;
        }
        let count = paths.len();
        self.explorer_clipboard = Some(ExplorerFileClipboard { op, paths });
        let verb = match op {
            ExplorerFileOp::Copy => "copied",
            ExplorerFileOp::Cut => "cut",
        };
        self.status = Some(format!("{verb} {count} explorer item(s)"));
    }

    /// Paste the internal explorer file clipboard into the selected destination.
    ///
    /// Every item is its own request: one failure reports itself and the rest go
    /// on, which is what the synchronous version did and what a user pasting a
    /// dozen files expects. The checks that remain here are the ones answerable
    /// from paths alone; whether a source still exists, and whether a destination
    /// is free, are the backend's to answer — it refuses to clobber, so a name the
    /// explorer had not heard of fails loudly instead of overwriting.
    pub(super) fn explorer_paste_files(&mut self) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            self.status = Some("paste: no explorer files".to_string());
            return;
        };
        let dest_dir = self.explorer_paste_destination();
        let mut taken: std::collections::BTreeSet<PathBuf> = self
            .explorer
            .rows()
            .iter()
            .filter(|row| row.path.parent() == Some(dest_dir.as_path()))
            .map(|row| row.path.clone())
            .collect();

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
            if path_contains_or_equals(source, &dest_dir) {
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
                crate::app::explorer_mutate::FollowUp::Pasted { moved },
            );
            requested += 1;
        }

        if requested > 0 && clipboard.op == ExplorerFileOp::Cut {
            self.explorer_clipboard = None;
        }
        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
        } else if requested == 0 && skipped > 0 {
            self.status = Some("paste: already in target folder".to_string());
        } else if requested > 0 {
            // Superseded per answer by the running count; this is what the user
            // sees while the backend is still working through the list.
            self.status = Some(format!("pasting {requested} item(s)…"));
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
            self.status = Some("duplicate: select a file first".to_string());
            return;
        }
        let known: std::collections::BTreeSet<PathBuf> = self
            .explorer
            .rows()
            .iter()
            .map(|row| row.path.clone())
            .collect();
        let mut requested = 0usize;
        for source in paths {
            let Some(parent) = source.parent() else {
                continue;
            };
            let target = unique_child_path(parent, &source, &known);
            self.mutate_path(
                PathMutation::Copy {
                    from: source.clone(),
                    to: target,
                },
                crate::app::explorer_mutate::FollowUp::Pasted { moved: None },
            );
            requested += 1;
        }
        if requested > 0 {
            self.explorer_paste_done = 0;
            self.status = Some(format!("duplicating {requested} item(s)…"));
        }
    }

    /// Copy selected explorer paths to the system clipboard.
    pub(super) fn explorer_copy_path(&mut self, relative: bool) {
        let paths = self.explorer_selected_paths();
        if paths.is_empty() {
            self.status = Some("copy path: select a file first".to_string());
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
            self.status = Some("delete: select a file first".to_string());
            return;
        }
        if self.has_dirty_tabs_under(&paths) {
            self.notify(
                Severity::Warning,
                NotificationKind::Io,
                "delete blocked: save or close dirty files first",
            );
            return;
        }
        self.context_menu_clear();
        self.status = Some(format!(
            "delete {} item(s)? press y to confirm, any other key to cancel",
            paths.len()
        ));
        self.pending_explorer_delete = Some(paths);
    }

    /// Resolve a pending explorer delete confirmation.
    pub(super) fn resolve_explorer_delete(&mut self, confirmed: bool) {
        let Some(paths) = self.pending_explorer_delete.take() else {
            return;
        };
        if !confirmed {
            self.status = Some("delete cancelled".to_string());
            return;
        }
        self.close_tabs_under(&paths);
        // Each path is its own request, so one failure does not abandon the rest —
        // the same "delete what you can, report what you cannot" behaviour, now
        // with the reporting arriving per answer.
        self.explorer_delete_done = 0;
        for path in paths {
            self.mutate_path(
                PathMutation::Delete { path },
                crate::app::explorer_mutate::FollowUp::Deleted,
            );
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

    pub(super) fn context_menu_clear(&mut self) {
        self.context_menu = None;
    }

    pub(super) fn open_context_menu(&mut self, x: u16, y: u16, row: Option<usize>) {
        self.sidebar_panel = SidebarPanel::Explorer;
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
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
        self.focus = Focus::Sidebar;
        self.build_explorer();
        let cursor = self.explorer.cursor();
        let y = self.sidebar_content_rect.y.saturating_add(
            cursor
                .saturating_sub(self.explorer.offset())
                .try_into()
                .unwrap_or(0),
        );
        let x = self.sidebar_content_rect.x.saturating_add(2);
        let row = (!self.explorer.rows().is_empty()).then_some(cursor);
        self.open_context_menu(x, y, row);
    }

    /// Open the pane context menu at `(x, y)` for the focused pane's active tab.
    /// Only file-backed tabs get one; a pathless tab (Welcome, commit graph, …)
    /// opens nothing.
    pub(super) fn open_pane_context_menu(&mut self, x: u16, y: u16) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            return;
        };
        let entries = self.pane_context_entries(&path);
        self.context_menu = Some(ContextMenu::new(x, y, entries));
    }

    /// The pane context menu's rows for the active file at `path`. The path items
    /// always work; the link items are enabled exactly when [`remote::link`] can
    /// build them (the same call their dispatch runs), with its refusal reason as
    /// the disabled note.
    pub(super) fn pane_context_entries(&mut self, path: &Path) -> Vec<ContextMenuEntry> {
        let mut entries = vec![
            ContextMenuEntry::enabled(Command::CopyPath),
            ContextMenuEntry::enabled(Command::CopyRelativePath),
            ContextMenuEntry::enabled(Command::RevealActiveInExplorer),
        ];
        // Owned disabled-notes computed up front, so the facts borrow does not
        // overlap the &mut calls below. `None` = enabled.
        let link_notes: [Option<String>; 3] = {
            let facts = self.cached_remote_facts(path);
            let note = |kind| match facts {
                Some(Ok(facts)) => remote::link(&facts.link_target(), kind, None).err(),
                Some(Err(note)) => Some(note.clone()),
                None => Some("resolving repository remote…".to_string()),
            };
            [
                note(remote::LinkKind::RemoteFile),
                note(remote::LinkKind::GithubPermalink),
                note(remote::LinkKind::GithubHeadLink),
            ]
        };
        let link_entry = |command, note: &Option<String>| match note {
            None => ContextMenuEntry::enabled(command),
            Some(note) => ContextMenuEntry::disabled(command, note.clone()),
        };
        entries.push(link_entry(Command::CopyRemoteFileUrl, &link_notes[0]));
        // The Open Changes actions need a repository and a tracked file, but no
        // remote — their enablement is checked separately from the link rows.
        let changes_note = self.open_changes_note(path);
        for command in [
            Command::OpenChangesWithPrevious,
            Command::OpenChangesWithRevision,
            Command::OpenChangesWithBranch,
        ] {
            entries.push(match &changes_note {
                None => ContextMenuEntry::enabled(command),
                Some(note) => ContextMenuEntry::disabled(command, note.clone()),
            });
        }
        entries.push(link_entry(Command::CopyGithubPermalink, &link_notes[1]));
        entries.push(link_entry(Command::CopyGithubHeadLink, &link_notes[2]));
        entries
    }

    pub(super) fn context_menu_step(&mut self, delta: i32) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.select_by(delta);
        }
    }

    pub(super) fn accept_context_menu(&mut self) {
        let Some(entry) = self
            .context_menu
            .as_ref()
            .and_then(ContextMenu::selected_entry)
        else {
            self.context_menu = None;
            return;
        };
        if !entry.enabled {
            // Refuse a disabled row: surface its explanatory note (when it has one)
            // and keep the menu open so another row can be chosen.
            if let Some(note) = entry.note.clone() {
                self.status = Some(note);
            }
            return;
        }
        let action = entry.action.clone();
        self.context_menu = None;
        match action {
            ContextMenuAction::Command(command) => self.dispatch(command),
            ContextMenuAction::ReplaceSpelling {
                doc,
                range,
                replacement,
            } => self.replace_spelling(doc, range, replacement),
            ContextMenuAction::AddSpellingToDictionary { word, target } => match target {
                DictionaryTarget::Project => self.add_spelling_to_project_dictionary(word),
                DictionaryTarget::User => self.add_spelling_to_user_dictionary(word),
            },
        }
    }

    pub(super) fn close_context_menu(&mut self) {
        self.context_menu_clear();
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
