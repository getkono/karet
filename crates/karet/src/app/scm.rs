use super::*;

impl App {
    /// Refresh branch, remote, recovery, and stash facts without blocking the UI.
    pub(super) fn request_repository_snapshot(&mut self) {
        if self.scm.repository_loading_since.is_some() {
            return;
        }
        self.scm.repository_loading_since = Some(Instant::now());
        self.scm.repository_request = self.send_command_id(SessionCommand::RepositorySnapshot);
    }

    /// Submit one ordered repository action.
    pub(super) fn run_vcs_action(&mut self, action: VcsAction) {
        self.send_vcs(SessionCommand::VcsAction { action });
    }

    /// Refuse to change the worktree while any editor has unsaved content, offering
    /// the explicit save-all path instead.
    pub(super) fn guard_branch_switch(&mut self, target: karet_vcs::BranchTarget) {
        if self.all_tabs().any(|tab| tab.dirty) {
            self.overlay = Some(Overlay::text(
                "Unsaved editors · type save to save all and switch",
                TextPurpose::SaveAndSwitch { target },
            ));
        } else {
            self.run_vcs_action(VcsAction::SwitchBranch(target));
        }
    }

    /// Save every distinct dirty document and park the switch until all answers arrive.
    pub(super) fn save_then_switch(&mut self, target: karet_vcs::BranchTarget) {
        let mut docs: Vec<DocumentId> = self
            .all_tabs()
            .filter(|tab| tab.dirty)
            .filter_map(Self::tab_doc)
            .collect();
        docs.sort();
        docs.dedup();
        let action = VcsAction::SwitchBranch(target);
        if self.save_docs(&docs) == 0 {
            self.run_vcs_action(action);
        } else {
            self.vcs_after_save = Some(action);
            self.status = Some(format!("saving {} editor(s) before switching…", docs.len()));
        }
    }

    /// Open the discoverable overflow menu for repository workflows.
    pub(super) fn open_scm_menu(&mut self) {
        let mut commands = vec![
            Command::ScmSync,
            Command::ScmSwitchBranch,
            Command::ScmCreateBranch,
            Command::ScmPickPullRequest,
            Command::ScmUndoCommit,
            Command::ScmStash,
            Command::ScmManageStashes,
            Command::ScmPublish,
            Command::ScmRenameBranch,
            Command::ScmDeleteBranch,
            Command::ScmDeleteRemoteBranch,
            Command::ScmRefresh,
        ];
        if let Some(operation) = self
            .scm
            .repository
            .as_ref()
            .and_then(|snapshot| snapshot.state.operation)
        {
            commands.insert(0, Command::ScmAbort);
            commands.insert(0, Command::ScmContinue);
            if !matches!(operation, karet_vcs::RepositoryOperation::Merge) {
                commands.insert(2, Command::ScmSkip);
            }
        }
        self.overlay = Some(Overlay::commands("Source Control", commands));
    }

    /// Open a combined local/remote branch picker.
    pub(super) fn open_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            self.status = Some("branches: loading repository state".to_string());
            return;
        };
        let mut items = Vec::new();
        for branch in &snapshot.branches {
            let head = if branch.is_head { "✓ " } else { "  " };
            items.push((
                format!("{head}{}", branch.name),
                karet_vcs::BranchTarget::Local(branch.name.clone()),
            ));
        }
        for branch in &snapshot.remote_branches {
            let local_name = branch.name.clone();
            items.push((
                format!("  {}/{}", branch.remote, branch.name),
                karet_vcs::BranchTarget::Remote {
                    remote: branch.remote.clone(),
                    branch: branch.name.clone(),
                    local_name,
                },
            ));
        }
        self.overlay = Some(Overlay::branches(items));
    }

    /// Open the full branch-creation form with every configured remote available.
    pub(super) fn open_create_branch_form(&mut self) {
        let remotes = self
            .scm
            .repository
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .remotes
                    .iter()
                    .map(|remote| remote.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.overlay = Some(Overlay::create_branch(remotes));
    }

    /// Query open pull requests for the upstream-aware primary remote.
    pub(super) fn open_pull_request_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            self.status = Some("pull requests: loading repository state".to_string());
            return;
        };
        let preferred = snapshot
            .state
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/').map(|(remote, _)| remote));
        let remote = preferred
            .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
            })
            .or_else(|| snapshot.remotes.first())
            .map(|remote| remote.name.clone());
        let Some(remote) = remote else {
            self.status = Some("pull requests: no remote is configured".to_string());
            return;
        };
        self.status = Some(format!("loading open pull requests from {remote}"));
        self.pull_request_items.clear();
        self.pull_request_remote = Some(remote.clone());
        self.pending_pull_requests = self.send_command_id(SessionCommand::PullRequests {
            remote,
            page: 1,
            per_page: 100,
        });
    }

    /// Open stash creation controls.
    pub(super) fn open_stash_form(&mut self) {
        self.overlay = Some(Overlay::stash_form());
    }

    /// Open actions for every current stash entry.
    pub(super) fn open_stash_manager(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        if snapshot.stashes.is_empty() {
            self.status = Some("stashes: none".to_string());
            return;
        }
        self.overlay = Some(Overlay::stashes(&snapshot.stashes));
    }

    /// Publish the current branch to its upstream remote, `origin`, or first remote.
    pub(super) fn publish_current_branch(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let Some(branch) = snapshot.state.branch.clone() else {
            self.status = Some("publish: HEAD is detached".to_string());
            return;
        };
        let preferred = snapshot
            .state
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/').map(|(remote, _)| remote));
        let remote = preferred
            .and_then(|name| snapshot.remotes.iter().find(|remote| remote.name == name))
            .or_else(|| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
            })
            .or_else(|| snapshot.remotes.first())
            .map(|remote| remote.name.clone());
        let Some(remote) = remote else {
            self.status = Some("publish: no remote is configured".to_string());
            return;
        };
        self.run_vcs_action(VcsAction::PublishBranch {
            remote,
            branch,
            set_upstream: true,
        });
    }

    /// Prompt for a replacement name for the current local branch.
    pub(super) fn prompt_rename_current_branch(&mut self) {
        let current = self
            .scm
            .repository
            .as_ref()
            .and_then(|snapshot| snapshot.state.branch.clone());
        let Some(old) = current else {
            self.status = Some("rename branch: HEAD is detached".to_string());
            return;
        };
        self.overlay = Some(Overlay::text(
            format!("Rename {old}"),
            TextPurpose::RenameBranch { old },
        ));
    }

    /// Pick a non-current local branch for safe (`git branch -d`) deletion.
    pub(super) fn open_delete_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let items: Vec<String> = snapshot
            .branches
            .iter()
            .filter(|branch| !branch.is_head)
            .map(|branch| branch.name.clone())
            .collect();
        if items.is_empty() {
            self.status = Some("delete branch: no eligible local branches".to_string());
        } else {
            self.overlay = Some(Overlay::delete_local_branches(items));
        }
    }

    /// Pick a non-default remote branch, then require its exact name as confirmation.
    pub(super) fn open_delete_remote_branch_picker(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            self.request_repository_snapshot();
            return;
        };
        let items: Vec<(String, String)> = snapshot
            .remote_branches
            .iter()
            .filter(|branch| !branch.is_default)
            .map(|branch| (branch.remote.clone(), branch.name.clone()))
            .collect();
        if items.is_empty() {
            self.status = Some("delete remote branch: no eligible branches".to_string());
        } else {
            self.overlay = Some(Overlay::delete_remote_branches(items));
        }
    }

    /// Request live blame when its document/version/cursor anchor changed.
    pub(super) fn request_live_blame(&mut self) {
        if !self.settings.git.blame {
            self.live_blame = None;
            self.pending_blame = None;
            self.failed_blame = None;
            return;
        }
        let target = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(doc),
                buffer,
                ..
            } => Some((*doc, buffer.version(), tab.editor.cursor().line)),
            _ => None,
        });
        let Some((doc, version, line)) = target else {
            self.live_blame = None;
            self.pending_blame = None;
            self.failed_blame = None;
            return;
        };
        if self.failed_blame == Some((doc, version, line)) {
            return;
        }
        self.failed_blame = None;
        if self
            .live_blame
            .as_ref()
            .is_some_and(|blame| blame.doc == doc && blame.version == version && blame.line == line)
        {
            return;
        }
        self.live_blame = None;
        // Let the one in-flight computation finish instead of queueing a full blame
        // for every intermediate cursor row. Its result handler immediately requests
        // the latest anchor when the cursor has moved in the meantime.
        if self.pending_blame.is_some() {
            return;
        }
        if let Some(id) = self.send_command_id(SessionCommand::Blame { doc, version, line }) {
            self.pending_blame = Some((id, doc, version, line));
        }
    }

    /// Toggle current-line blame and persist the user setting.
    pub(super) fn toggle_live_blame(&mut self) {
        self.apply_blame_setting(!self.settings.git.blame);
    }

    /// Open the attributed commit for the current line in the standard commit tab.
    pub(super) fn open_live_blame_detail(&mut self) {
        let hash = self
            .live_blame
            .as_ref()
            .and_then(LiveBlame::commit_hash)
            .map(str::to_string);
        if let Some(hash) = hash {
            self.open_commit(hash);
        }
    }

    fn apply_blame_setting(&mut self, enabled: bool) {
        self.settings.git.blame = enabled;
        self.loaded_config.settings.git.blame = enabled;
        #[cfg(not(test))]
        if let Err(error) = karet_session::config::set_user_blame(enabled) {
            self.notify(
                Severity::Error,
                NotificationKind::System,
                format!("settings: {error}"),
            );
        }
        self.pending_blame = None;
        self.failed_blame = None;
        self.live_blame = None;
        self.request_live_blame();
        let label = if enabled { "on" } else { "off" };
        self.status = Some(format!("inline blame: {label}"));
    }

    /// Open the Source-Control cursor's change as a materialized (permanent) diff
    /// view and move keyboard focus into it — the explicit Enter / double-click
    /// "take me into the view" action. Browsing (arrow moves, single click) goes
    /// through [`preview_selected_diff`](Self::preview_selected_diff) instead,
    /// which keeps focus on the panel so the staging keys stay live.
    pub(super) fn open_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        // Never duplicate: an existing diff tab for the same change — the preview
        // slot or a permanent one — is materialized and focused instead.
        if let Some(idx) = self.find_diff_tab(&change.path, section) {
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.is_preview = false;
            }
            self.select_tab(idx);
            return;
        }
        let tab = self.build_diff_tab(change, section);
        self.push_tab(tab);
    }

    /// Show the Source-Control cursor's change in the pane's shared preview slot
    /// *without* stealing keyboard focus (selection-follows-preview): browsing the
    /// change list with the arrows (or a single click) shows each diff while the
    /// panel keeps focus, so stage/unstage/discard/commit and the selection keys
    /// keep working. An existing diff tab for the same change is just shown;
    /// otherwise the preview slot is replaced in place — never one new tab per
    /// visited change.
    pub(super) fn preview_selected_diff(&mut self) {
        let cursor = self.scm.selection.cursor();
        let Some(change) = self.scm.changes.get(cursor).cloned() else {
            return;
        };
        let section = self.scm.section(cursor);
        if let Some(idx) = self.find_diff_tab(&change.path, section) {
            self.active = idx;
            self.find_open = false;
            return;
        }
        let mut tab = self.build_diff_tab(change, section);
        tab.is_preview = true;
        self.install_preview_tab(tab, false);
    }

    /// The index of this pane's existing diff tab for `path` in `section`, if any
    /// (preview or permanent) — the dedup lookup for the Source-Control open paths.
    pub(super) fn find_diff_tab(&self, path: &Path, section: Section) -> Option<usize> {
        self.tabs.iter().position(|t| {
            matches!(&t.kind, TabKind::Diff { file, .. }
                if file.change.path == *path && file.section == section)
        })
    }

    /// Diff and highlight `change` into a fresh [`TabKind::Diff`] tab using the
    /// remembered layout. The caller decides how the tab enters the pane (preview
    /// slot vs permanent) and where focus lands.
    pub(super) fn build_diff_tab(&self, change: FileChange, section: Section) -> Tab {
        let title = change
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("diff")
            .to_string();
        let file = FileView::new(change, section, self.syntax);
        Tab::new(
            title,
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        )
    }

    // --- source control ---------------------------------------------------

    /// Request one page of the commit log starting at `skip`, unless one is already
    /// in flight. The result arrives as [`SessionEvent::VcsLog`].
    pub(super) fn request_scm_log(&mut self, skip: usize) {
        if self.scm.log_loading {
            return;
        }
        self.scm.log_loading = true;
        self.scm.log_loading_since = Some(Instant::now());
        self.send_vcs(SessionCommand::VcsLog {
            skip,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Fetch the next page of the commit log (from the end of what is loaded).
    pub(super) fn load_more_scm_log(&mut self) {
        if self.scm.log_has_more {
            let skip = self.scm.log.len();
            self.request_scm_log(skip);
        }
    }

    /// Open the commit view for `rev` (a hash or ref) immediately, fill metadata when
    /// [`SessionEvent::CommitDetailReady`] arrives, then fill changed files when
    /// [`SessionEvent::CommitReady`] arrives.
    pub(super) fn open_commit(&mut self, rev: String) {
        self.push_tab(Tab::commit_loading(rev.clone()));
        let view = self.tabs[self.active].view;
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev }) {
            self.pending_commit_detail
                .insert(id, CommitDest::Tab { view });
        }
    }

    /// Open the full-screen commit graph browser and request its first history page.
    pub(super) fn open_commit_graph(&mut self) {
        self.push_tab(Tab::commit_graph(None, "Commits"));
        self.graph_log_req = self.send_command_id(SessionCommand::VcsLog {
            skip: 0,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Open the graph browser scoped to the active file's history (`git log -- file`).
    pub(super) fn open_file_history(&mut self) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("file history: open a file first".to_string());
            return;
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("history")
            .to_string();
        self.push_tab(Tab::commit_graph(Some(path.clone()), format!("⌥ {name}")));
        self.graph_log_req = self.send_command_id(SessionCommand::FileHistory {
            path,
            skip: 0,
            limit: SCM_LOG_PAGE,
        });
    }

    /// Open the go-to-commit input; the typed revision resolves via [`open_commit`].
    pub(super) fn open_rev_input(&mut self) {
        self.rev_input = Some(String::new());
    }

    /// Cancel the go-to-commit input.
    pub(super) fn rev_cancel(&mut self) {
        self.rev_input = None;
        self.status = Some("go to commit cancelled".to_string());
    }

    /// Submit the typed revision: open a range when it contains `..`/`...`, otherwise the
    /// single commit; re-prompt when empty.
    pub(super) fn rev_submit(&mut self) {
        let rev = self.rev_input.take().unwrap_or_default().trim().to_string();
        if rev.is_empty() {
            self.rev_input = Some(String::new());
            self.status =
                Some("go to commit: enter a hash, ref, or range (a..b, a...b)".to_string());
        } else if let Some((base, head, merge_base)) = parse_rev_range(&rev) {
            self.open_range(SessionCommand::RangeChanges {
                spec: RangeSpec::Between {
                    base,
                    head,
                    merge_base,
                },
            });
        } else {
            self.open_commit(rev);
        }
    }

    /// Edit the go-to-commit revision with an unbound key (backspace / printable).
    pub(super) fn rev_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.pop();
                }
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(rev) = self.rev_input.as_mut() {
                    rev.push(c);
                }
            },
            _ => {},
        }
    }
    /// Send a fire-and-forget command to the backend (no document context).
    pub(super) fn send_vcs(&mut self, command: SessionCommand) {
        self.send_command(command);
    }

    /// Submit a fire-and-forget backend command (the answering event, if any, is
    /// handled generically), surfacing a dropped-backend error as a notification.
    pub(super) fn send_command(&mut self, command: SessionCommand) {
        let result = self.backend.as_ref().map(|backend| {
            let id = backend.next_id();
            backend.send(id, command)
        });
        if let Some(Err(e)) = result {
            self.notify_backend_error(e);
        }
    }

    /// Submit a backend command and return its [`RequestId`], so the answering event
    /// can be correlated (e.g. to route a commit detail to the right destination).
    /// Returns `None` when there is no backend or the submission failed.
    pub(super) fn send_command_id(&mut self, command: SessionCommand) -> Option<RequestId> {
        let (id, result) = {
            let backend = self.backend.as_ref()?;
            let id = backend.next_id();
            (id, backend.send(id, command))
        };
        match result {
            Ok(()) => Some(id),
            Err(e) => {
                self.notify_backend_error(e);
                None
            },
        }
    }

    /// Send a path-scoped Source-Control command for the current selection.
    pub(super) fn scm_send_paths(&mut self, make: impl FnOnce(Vec<PathBuf>) -> SessionCommand) {
        let paths = self.scm.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.send_vcs(make(paths));
    }

    /// Toggle staging for the selection. A multi-file selection may span both
    /// groups, so partition it by section: staged rows are unstaged, working rows
    /// are staged.
    pub(super) fn scm_toggle_stage(&mut self) {
        let mut to_stage = Vec::new();
        let mut to_unstage = Vec::new();
        for i in self.scm.selection.selected_indices() {
            let Some(change) = self.scm.changes.get(i) else {
                continue;
            };
            match self.scm.section(i) {
                Section::Staged => to_unstage.push(change.path.clone()),
                Section::Working => to_stage.push(change.path.clone()),
            }
        }
        if !to_unstage.is_empty() {
            self.send_vcs(SessionCommand::Unstage { paths: to_unstage });
        }
        if !to_stage.is_empty() {
            self.send_vcs(SessionCommand::Stage { paths: to_stage });
        }
    }

    /// Extend the focused list panel's range selection by `delta` rows.
    pub(super) fn sidebar_select_extend(&mut self, delta: i32) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.select_extend(delta);
            },
            SidebarPanel::SourceControl => self.scm.selection.extend_by(delta),
            SidebarPanel::Search => {},
        }
    }

    /// Toggle the cursor row in the focused list panel's selection.
    pub(super) fn sidebar_select_toggle(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.mark_toggle();
            },
            SidebarPanel::SourceControl => self.scm.selection.toggle_cursor(),
            SidebarPanel::Search => {},
        }
    }

    /// Select every row in the focused list panel.
    pub(super) fn sidebar_select_all(&mut self) {
        match self.sidebar_panel {
            SidebarPanel::Explorer => {
                self.explorer.ensure_built(&self.root);
                self.explorer.select_all();
            },
            SidebarPanel::SourceControl => self.scm.selection.select_all(),
            SidebarPanel::Search => {},
        }
    }

    /// Open the commit-message input, if there is something staged to commit.
    pub(super) fn scm_open_commit_input(&mut self) {
        if self.scm.staged_count == 0 {
            self.status = Some("commit: stage changes first".to_string());
            return;
        }
        self.commit_input = Some(String::new());
    }

    /// Arm a discard confirmation for the current selection.
    pub(super) fn scm_arm_discard(&mut self) {
        let paths = self.scm.selected_paths();
        if paths.is_empty() {
            return;
        }
        self.status = Some(format!(
            "discard {} file(s)? press y to confirm, any other key to cancel",
            paths.len()
        ));
        self.pending_discard = Some(paths);
    }

    /// Cancel the commit input.
    pub(super) fn commit_cancel(&mut self) {
        self.commit_input = None;
        self.status = Some("commit cancelled".to_string());
    }

    /// Submit the commit message (or report that one is required).
    pub(super) fn commit_submit(&mut self) {
        let message = self.commit_input.take().unwrap_or_default();
        let message = message.trim().to_string();
        if message.is_empty() {
            self.commit_input = Some(String::new());
            self.status = Some("commit: message required".to_string());
        } else {
            self.send_vcs(SessionCommand::Commit { message });
        }
    }

    /// Ask the backend to draft a commit message from the staged diff. The result
    /// arrives asynchronously as [`SessionEvent::CommitMessageGenerated`] and replaces
    /// the input; problems (nothing staged, disabled, generator error) come back as a
    /// notification.
    pub(super) fn commit_generate(&mut self) {
        self.status = Some("generating commit message…".to_string());
        self.send_vcs(SessionCommand::GenerateCommitMessage);
    }

    /// Edit the commit message with an unbound key (backspace / printable).
    pub(super) fn commit_edit(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                if let Some(message) = self.commit_input.as_mut() {
                    message.pop();
                }
            },
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(message) = self.commit_input.as_mut() {
                    message.push(c);
                }
            },
            _ => {},
        }
    }

    /// Resolve a pending discard: `confirmed` discards the armed paths, otherwise
    /// the prompt is cancelled. Any key without a `DiscardConfirm` binding cancels.
    pub(super) fn resolve_discard(&mut self, confirmed: bool) {
        let paths = self.pending_discard.take();
        if confirmed {
            if let Some(paths) = paths {
                self.send_vcs(SessionCommand::Discard { paths });
                self.notify(
                    Severity::Information,
                    NotificationKind::Vcs,
                    "discarded changes",
                );
            }
        } else {
            self.status = Some("discard cancelled".to_string());
        }
    }

    /// Replace the Source-Control panel state from a fresh backend status,
    /// reconciling the existing selection against the new row count.
    pub(super) fn apply_vcs_status(&mut self, staged: Vec<FileChange>, working: Vec<FileChange>) {
        let staged_count = staged.len();
        let mut changes = staged;
        changes.extend(working);
        self.scm.changes = changes;
        self.scm.staged_count = staged_count;
        self.scm.selection.set_len(self.scm.changes.len());
    }

    /// Apply a fetched commit-log page: the first page (`skip == 0`) replaces the
    /// log; a later page appends. Guards against duplicate appends if a page is
    /// re-delivered.
    pub(super) fn apply_vcs_log(&mut self, skip: usize, commits: Vec<Commit>, has_more: bool) {
        self.scm.log_loading = false;
        self.scm.log_loading_since = None;
        self.scm.log_has_more = has_more;
        if skip == 0 {
            // A fresh first page (initial load or a reconciliation reset) replaces the
            // log; scroll back to the top so the newest commits are in view.
            self.scm.log = commits;
            self.scm_commits_offset = 0;
        } else if skip == self.scm.log.len() {
            self.scm.log.extend(commits);
        }
    }

    /// Prepend newly-observed commits reported by the backend (an external commit
    /// picked up via file-watching). Duplicates are dropped, and the viewport is
    /// nudged so the user's position in the older history is preserved.
    pub(super) fn apply_vcs_commits_prepended(&mut self, mut commits: Vec<Commit>) {
        let known: HashSet<&str> = self.scm.log.iter().map(|c| c.hash.as_str()).collect();
        commits.retain(|c| !known.contains(c.hash.as_str()));
        let inserted = commits.len();
        if inserted == 0 {
            return;
        }
        commits.append(&mut self.scm.log);
        self.scm.log = commits;
        // If the user had scrolled into the log, shift down so the same commits stay
        // put; at the top (offset 0) keep them at the newest.
        if self.scm_commits_offset > 0 {
            self.scm_commits_offset += inserted;
        }
    }
}
