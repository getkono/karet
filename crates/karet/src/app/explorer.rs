use super::*;

impl App {
    /// Start background status reads for nested repository rows not already cached.
    pub(crate) fn request_nested_repository_statuses(&mut self) {
        self.explorer.ensure_built(&self.root);
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
                self.send_command_id(SessionCommand::NestedRepositoryStatus { path: path.clone() })
            {
                self.nested_repository_pending
                    .insert(request, (path, Instant::now()));
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
                .filter(|(_, since)| now.saturating_duration_since(*since) >= LOADING_REVEAL_DELAY)
                .map(|(path, since)| {
                    (
                        path.clone(),
                        repository_spinner(now.saturating_duration_since(*since), self.icon_style),
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
            .map(|(_, since)| {
                LOADING_REVEAL_DELAY
                    .checked_sub(now.saturating_duration_since(*since))
                    .unwrap_or(Duration::from_millis(100))
            })
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
        self.explorer.ensure_built(&self.root);
        self.explorer.begin_new(folder);
    }

    /// Begin renaming the selected explorer entry (no-op unless the Explorer panel is
    /// the active sidebar panel).
    pub(super) fn explorer_begin_rename(&mut self) {
        if self.sidebar_panel != SidebarPanel::Explorer {
            return;
        }
        self.explorer.ensure_built(&self.root);
        self.explorer.begin_rename();
    }

    /// Hard-reload the explorer tree and re-request VCS status — a bullet-proof
    /// refresh that drops every cached row and re-reads the filesystem.
    pub(super) fn explorer_refresh(&mut self) {
        self.explorer.rebuild(&self.root);
        self.nested_repository_status.clear();
        let pending: Vec<RequestId> = self.nested_repository_pending.keys().copied().collect();
        self.nested_repository_pending.clear();
        for request in pending {
            self.cancel_backend_request(request);
        }
        self.send_vcs(SessionCommand::RefreshVcs);
    }

    /// Apply the explorer inline edit: create the file/folder or rename on disk, then
    /// reload the tree (and open a newly-created file).
    pub(super) fn explorer_commit_edit(&mut self) {
        let Some(pending) = self.explorer.take_edit() else {
            return;
        };
        match &pending {
            PendingEdit::Create { path, folder } => {
                let result = if *folder {
                    std::fs::create_dir_all(path)
                } else {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    std::fs::File::create(path).map(|_| ())
                };
                match result {
                    Ok(()) => {
                        self.explorer.rebuild(&self.root);
                        self.send_vcs(SessionCommand::RefreshVcs);
                        if !*folder {
                            self.open_path(path);
                        }
                    },
                    Err(e) => {
                        self.explorer.restore_edit(&pending);
                        self.notify(
                            Severity::Error,
                            NotificationKind::Io,
                            format!("create failed: {e}"),
                        );
                    },
                }
            },
            PendingEdit::Rename { from, to } => match std::fs::rename(from, to) {
                Ok(()) => {
                    self.retarget_open_paths(from, to);
                    self.explorer.rebuild(&self.root);
                    self.send_vcs(SessionCommand::RefreshVcs);
                },
                Err(e) => {
                    self.explorer.restore_edit(&pending);
                    self.notify(
                        Severity::Error,
                        NotificationKind::Io,
                        format!("rename failed: {e}"),
                    );
                },
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
        self.explorer.ensure_built(&self.root);
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
    pub(super) fn explorer_paste_files(&mut self) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            self.status = Some("paste: no explorer files".to_string());
            return;
        };
        let dest_dir = self.explorer_paste_destination();
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            self.notify(
                Severity::Error,
                NotificationKind::Io,
                format!("paste failed: {e}"),
            );
            return;
        }

        let mut pasted = 0usize;
        let mut skipped = 0usize;
        let mut failed = 0usize;
        let mut first_error: Option<String> = None;
        let mut moves = Vec::new();

        for source in &clipboard.paths {
            if !source.exists() {
                failed += 1;
                first_error.get_or_insert_with(|| {
                    format!("paste failed: {} no longer exists", source.display())
                });
                continue;
            }
            if clipboard.op == ExplorerFileOp::Cut
                && source
                    .parent()
                    .is_some_and(|parent| same_path(parent, &dest_dir))
            {
                skipped += 1;
                continue;
            }
            if source.is_dir() && path_contains_or_equals(source, &dest_dir) {
                failed += 1;
                first_error.get_or_insert_with(|| {
                    format!(
                        "paste failed: cannot paste {} into itself",
                        source.display()
                    )
                });
                continue;
            }

            let target = unique_child_path(&dest_dir, source);
            let result = match clipboard.op {
                ExplorerFileOp::Copy => copy_path_recursive(source, &target),
                ExplorerFileOp::Cut => move_path(source, &target),
            };
            match result {
                Ok(()) => {
                    pasted += 1;
                    if clipboard.op == ExplorerFileOp::Cut {
                        moves.push((source.clone(), target));
                    }
                },
                Err(e) => {
                    failed += 1;
                    first_error.get_or_insert_with(|| format!("paste failed: {e}"));
                },
            }
        }

        if pasted > 0 {
            for (from, to) in &moves {
                self.retarget_open_paths(from, to);
            }
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            if clipboard.op == ExplorerFileOp::Cut {
                self.explorer_clipboard = None;
            }
        }

        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
        }

        self.status = if pasted > 0 && failed > 0 {
            Some(format!("pasted {pasted} item(s), {failed} failed"))
        } else if pasted > 0 {
            Some(format!("pasted {pasted} item(s)"))
        } else if skipped > 0 && failed == 0 {
            Some("paste: already in target folder".to_string())
        } else {
            Some("paste failed".to_string())
        };
    }

    /// The explorer paste target: selected directory, selected file's parent, or root.
    pub(super) fn explorer_paste_destination(&mut self) -> PathBuf {
        self.explorer.ensure_built(&self.root);
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
        self.explorer.ensure_built(&self.root);
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
        let mut copied = 0usize;
        let mut first_error = None;
        for source in paths {
            let Some(parent) = source.parent() else {
                continue;
            };
            let target = unique_child_path(parent, &source);
            match copy_path_recursive(&source, &target) {
                Ok(()) => copied += 1,
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("duplicate failed: {e}"));
                },
            }
        }
        if copied > 0 {
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            self.status = Some(format!("duplicated {copied} item(s)"));
        }
        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
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
        let mut deleted = 0usize;
        let mut first_error = None;
        for path in &paths {
            let result = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
            match result {
                Ok(()) => deleted += 1,
                Err(e) if !path.exists() => {
                    deleted += 1;
                    first_error.get_or_insert_with(|| format!("delete warning: {e}"));
                },
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("delete failed: {e}"));
                },
            }
        }
        if deleted > 0 {
            self.explorer.rebuild(&self.root);
            self.send_vcs(SessionCommand::RefreshVcs);
            self.status = Some(format!("deleted {deleted} item(s)"));
        }
        if let Some(message) = first_error {
            self.notify(Severity::Error, NotificationKind::Io, message);
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
        self.explorer.ensure_built(&self.root);
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
        self.explorer.ensure_built(&self.root);
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
    pub(super) fn pane_context_entries(&self, path: &Path) -> Vec<ContextMenuEntry> {
        let mut entries = vec![
            ContextMenuEntry::enabled(Command::CopyPath),
            ContextMenuEntry::enabled(Command::CopyRelativePath),
            ContextMenuEntry::enabled(Command::RevealActiveInExplorer),
        ];
        let facts = self.remote_facts(path);
        let link_entry = |command, kind| match &facts {
            Ok(facts) => match remote::link(&facts.link_target(), kind, None) {
                Ok(_) => ContextMenuEntry::enabled(command),
                Err(note) => ContextMenuEntry::disabled(command, note),
            },
            Err(note) => ContextMenuEntry::disabled(command, note.clone()),
        };
        entries.push(link_entry(
            Command::CopyRemoteFileUrl,
            remote::LinkKind::RemoteFile,
        ));
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
        entries.push(link_entry(
            Command::CopyGithubPermalink,
            remote::LinkKind::GithubPermalink,
        ));
        entries.push(link_entry(
            Command::CopyGithubHeadLink,
            remote::LinkKind::GithubHeadLink,
        ));
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
        let command = entry.command;
        self.context_menu = None;
        self.dispatch(command);
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

fn repository_spinner(elapsed: Duration, icons: IconStyle) -> String {
    let frame = usize::try_from(elapsed.as_millis() / 100).unwrap_or(usize::MAX);
    let frames: &[&str] = if icons == IconStyle::Ascii {
        &["-", "\\", "|", "/"]
    } else {
        &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
    };
    frames[frame % frames.len()].to_string()
}
