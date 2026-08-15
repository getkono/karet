use super::*;

impl App {
    pub(super) fn toggle_commit_file(&mut self, file: usize) -> bool {
        let Some(TabKind::Commit { view, .. } | TabKind::Compare { view, .. }) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        else {
            return false;
        };
        if !view.collapsed_files.remove(&file) {
            view.collapsed_files.insert(file);
        }
        if let Some(anchor) = view.file_anchors.get(file) {
            view.scroll = *anchor;
        }
        true
    }

    /// The active tab's commit graph browser, if it is one.
    pub(super) fn active_commit_graph(&mut self) -> Option<&mut TabKind> {
        let tab = self.tabs.get_mut(self.active)?;
        matches!(tab.kind, TabKind::CommitGraph { .. }).then_some(&mut tab.kind)
    }

    /// Move the browser's selection by `delta` (clamped), and request the newly
    /// selected commit's detail if it isn't already shown.
    pub(super) fn graph_select(&mut self, delta: i32) {
        let Some(TabKind::CommitGraph {
            commits, selected, ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        let Some(last) = commits.len().checked_sub(1) else {
            return;
        };
        let next = (*selected as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
        self.graph_select_to(next);
    }

    /// Select commit `index` outright, rather than stepping towards it.
    ///
    /// The browser's list offset is recomputed from the selection every frame, so a
    /// dragged scrollbar has to move the selection to move the view — and it has to
    /// come through here, not by writing `selected`, or the detail pane would keep
    /// showing whichever commit the selection left behind.
    pub(super) fn graph_select_to(&mut self, index: usize) {
        let Some(TabKind::CommitGraph {
            history_path,
            commits,
            selected,
            has_more,
            loading,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        if commits.is_empty() {
            return;
        }
        let next = index.min(commits.len() - 1);
        *selected = next;
        // Page in more history when nearing the end, from the same source (whole-repo
        // log or a single file's history).
        let near_end = next + COMMIT_AUTOLOAD_THRESHOLD >= commits.len();
        let want_more = *has_more && !*loading && near_end;
        let loaded = commits.len();
        let path = history_path.clone();
        let hash = commits[next].hash.clone();
        self.graph_request_detail(hash);
        if want_more {
            if let Some(TabKind::CommitGraph {
                loading,
                loading_since,
                ..
            }) = self.active_commit_graph()
            {
                *loading = true;
                *loading_since = Some(Pending::start());
            }
            let command = match path {
                Some(path) => SessionCommand::FileHistory {
                    path,
                    skip: loaded,
                    limit: SCM_LOG_PAGE,
                },
                None => SessionCommand::VcsLog {
                    skip: loaded,
                    limit: SCM_LOG_PAGE,
                },
            };
            let view = self.tabs[self.active].view;
            self.graph_log_req = self.send(command).map(|id| (id, view));
        }
    }

    /// Request `hash`'s detail for the browser pane, unless it is already the shown
    /// detail (avoids re-fetching when re-selecting the same commit).
    pub(super) fn graph_request_detail(&mut self, hash: String) {
        let view = self.tabs.get(self.active).map_or(ViewId(0), |tab| tab.view);
        if let Some(TabKind::CommitGraph { detail, .. }) = self.active_commit_graph()
            && detail.as_ref().is_some_and(|d| d.hash == hash)
        {
            return;
        }
        let stale: Vec<RequestId> = self
            .pending_commit_detail
            .iter()
            .filter_map(|(request, destination)| {
                matches!(destination, CommitDest::Browser { view: owner, .. } if *owner == view)
                    .then_some(*request)
            })
            .collect();
        for request in stale {
            self.pending_commit_detail.remove(&request);
            self.cancel_backend_request(request);
        }
        let stale_verification: Vec<RequestId> = self
            .pending_commit_verification
            .iter()
            .filter_map(|(request, (owner, _))| (*owner == view).then_some(*request))
            .collect();
        for request in stale_verification {
            self.pending_commit_verification.remove(&request);
            self.cancel_backend_request(request);
        }
        if let Some(TabKind::CommitGraph {
            detail,
            files,
            detail_loading_since,
            ..
        }) = self.active_commit_graph()
        {
            *detail = None;
            *files = CommitFiles::default();
            *detail_loading_since = Some(Pending::start());
        }
        if let Some(id) = self.send(SessionCommand::CommitDetail { rev: hash.clone() }) {
            self.pending_commit_detail
                .insert(id, CommitDest::Browser { view, hash });
        }
    }

    /// Open the browser's selected commit as a standalone commit tab.
    pub(super) fn graph_open_selected(&mut self) {
        if let Some(TabKind::CommitGraph {
            commits, selected, ..
        }) = self.active_commit_graph()
            && let Some(commit) = commits.get(*selected)
        {
            let hash = commit.hash.clone();
            self.open_commit(hash);
        }
    }

    /// Request a range diff; the answering [`SessionEvent::RangeReady`] opens the compare
    /// tab, and an unresolvable range answers with a VCS notification instead.
    pub(super) fn open_range(&mut self, command: SessionCommand) {
        self.status = Some("computing diff…".to_string());
        self.send_command(command);
    }

    /// Mark the browser's selected commit as the base for a two-commit comparison.
    pub(super) fn graph_mark_base(&mut self) {
        if let Some(TabKind::CommitGraph {
            commits,
            selected,
            compare_base,
            ..
        }) = self.active_commit_graph()
            && let Some(commit) = commits.get(*selected)
        {
            let short = commit.short_hash.clone();
            *compare_base = Some(commit.hash.clone());
            self.status = Some(format!(
                "compare base marked: {short} (select another, then compare)"
            ));
        }
    }

    /// Compare the browser's marked base commit against the current selection (a two-dot
    /// `base..selected` diff). Reports a status when no base has been marked yet.
    pub(super) fn graph_compare(&mut self) {
        let Some(TabKind::CommitGraph {
            commits,
            selected,
            compare_base,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        let Some(base) = compare_base.clone() else {
            self.status =
                Some("mark a compare base first (Commit Graph: Mark Compare Base)".to_string());
            return;
        };
        let Some(head) = commits.get(*selected).map(|c| c.hash.clone()) else {
            return;
        };
        self.open_range(SessionCommand::RangeChanges {
            spec: RangeSpec::Between {
                base,
                head,
                merge_base: false,
            },
        });
    }

    /// Fill the graph browser's metadata pane from a resolved commit, and fire the lazy
    /// GitHub verification fetch. A no-op if no browser is open.
    pub(super) fn fill_graph_metadata(&mut self, view: ViewId, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            if let TabKind::CommitGraph {
                commits,
                selected,
                detail: slot,
                files,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                *slot = Some(detail.clone());
                files.reset_loading();
                *detail_loading_since = None;
                filled = true;
            }
        }
        if filled {
            self.request_commit_verification(view, hash);
        }
    }

    /// Fill the graph browser's detail pane from a resolved commit, and fire the lazy
    /// GitHub verification fetch. A no-op if no browser is open.
    pub(super) fn fill_graph_detail(
        &mut self,
        view: ViewId,
        detail: Box<CommitDetail>,
        prepared: Vec<PreparedChange>,
    ) {
        let hash = detail.hash.clone();
        let mut prepared = Some(commit_file_views(prepared));
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            if let TabKind::CommitGraph {
                commits,
                selected,
                detail: slot,
                files,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                let verification = (slot.as_ref().is_some_and(|d| d.hash == hash))
                    .then(|| files.verification.take())
                    .flatten();
                *files = CommitFiles {
                    verification,
                    ..CommitFiles::ready(prepared.take().unwrap_or_default())
                };
                *slot = Some(detail.clone());
                *detail_loading_since = None;
                filled = true;
            }
        }
        if filled {
            self.request_commit_verification(view, hash);
        }
    }

    /// Apply a fetched history page to the graph browser: replace on the first page,
    /// append otherwise. On the first page, select the top commit and load its detail.
    pub(super) fn apply_graph_log(&mut self, skip: usize, commits: Vec<Commit>, has_more: bool) {
        let mut first_hash = None;
        for tab in self.all_tabs_mut() {
            if let TabKind::CommitGraph {
                commits: loaded,
                has_more: more,
                loading,
                loading_since,
                selected,
                ..
            } = &mut tab.kind
            {
                *loading = false;
                *loading_since = None;
                *more = has_more;
                if skip == 0 {
                    *loaded = commits.clone();
                    *selected = 0;
                    first_hash = loaded.first().map(|c| c.hash.clone());
                } else if skip == loaded.len() {
                    loaded.extend(commits.clone());
                }
            }
        }
        if let Some(hash) = first_hash {
            self.graph_request_detail(hash);
        }
    }

    /// Build and open a commit tab from a resolved [`CommitDetail`] and its
    /// backend-prepared changes, then fire the lazy GitHub verification fetch to
    /// upgrade the signature badge.
    pub(super) fn open_commit_tab(
        &mut self,
        detail: Box<CommitDetail>,
        changes: Vec<PreparedChange>,
    ) {
        let files = CommitFiles::ready(commit_file_views(changes));
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, files));
        let view = self.tabs[self.active].view;
        self.request_commit_verification(view, hash);
    }

    /// Open a standalone commit tab with metadata visible while changed files are still
    /// loading. Used for unsolicited commit-detail events.
    pub(super) fn open_commit_metadata_tab(&mut self, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, CommitFiles::loading()));
        let view = self.tabs[self.active].view;
        self.request_commit_verification(view, hash);
    }

    /// Fill an already-open pending commit tab with metadata, leaving its changed-file
    /// block in a progressive loading state.
    pub(super) fn fill_commit_metadata(&mut self, view: ViewId, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        let title = commit_title(&detail.short_hash);
        let mut detail = Some(detail);
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            tab.title = title;
            if let Some(detail) = detail.take() {
                let scroll = match &tab.kind {
                    TabKind::CommitLoading { pager, .. } => pager.scroll,
                    TabKind::Commit { view, .. } => view.scroll,
                    _ => 0,
                };
                tab.kind = TabKind::Commit {
                    detail,
                    files: CommitFiles::loading(),
                    explain_since: None,
                    view: CommitViewState {
                        scroll,
                        ..CommitViewState::default()
                    },
                };
                filled = true;
            }
            break;
        }
        if filled {
            self.request_commit_verification(view, hash);
        }
    }

    /// Fill an already-open pending commit tab. If the tab was closed before the
    /// request answered, the detail is discarded instead of surprising the user with
    /// a late tab.
    pub(super) fn fill_commit_tab(
        &mut self,
        view: ViewId,
        detail: Box<CommitDetail>,
        prepared: Vec<PreparedChange>,
    ) {
        let mut files = Some(commit_file_views(prepared));
        let hash = detail.hash.clone();
        let title = commit_title(&detail.short_hash);
        let mut detail = Some(detail);
        let mut filled = false;
        for tab in self.all_tabs_mut() {
            if tab.view == view {
                tab.title = title;
                if let (Some(detail), Some(files)) = (detail.take(), files.take()) {
                    match &mut tab.kind {
                        TabKind::Commit {
                            detail: slot,
                            files: current_files,
                            ..
                        } if slot.hash == hash => {
                            *slot = detail;
                            let verification = current_files.verification.take();
                            *current_files = CommitFiles {
                                verification,
                                ..CommitFiles::ready(files)
                            };
                        },
                        _ => {
                            tab.kind = TabKind::Commit {
                                detail,
                                files: CommitFiles::ready(files),
                                explain_since: None,
                                view: CommitViewState::default(),
                            };
                        },
                    }
                    filled = true;
                }
                break;
            }
        }
        if filled {
            self.request_commit_verification(view, hash);
        }
    }

    /// Fetch a forge verdict once per `(view, commit)`, retaining ownership so close
    /// can cancel the network future and a late response cannot affect another view.
    pub(super) fn request_commit_verification(&mut self, view: ViewId, hash: String) {
        if self
            .pending_commit_verification
            .values()
            .any(|pending| pending.0 == view && pending.1 == hash)
        {
            return;
        }
        if let Some(request) =
            self.send(SessionCommand::FetchCommitVerification { hash: hash.clone() })
        {
            self.pending_commit_verification
                .insert(request, (view, hash));
        }
    }

    /// Mark a pending commit-detail request as failed and clear any visible loading
    /// placeholder tied to that request.
    pub(super) fn fail_pending_commit_detail(&mut self, request: RequestId, message: &str) {
        let Some(dest) = self.pending_commit_detail.remove(&request) else {
            return;
        };
        match dest {
            CommitDest::Tab { view } => {
                for tab in self.all_tabs_mut() {
                    if tab.view != view {
                        continue;
                    }
                    match &mut tab.kind {
                        TabKind::CommitLoading { error, .. } => {
                            *error = Some(message.to_string());
                        },
                        TabKind::Commit { files, .. } => {
                            files.loading_since = None;
                            files.error = Some(message.to_string());
                        },
                        _ => {},
                    }
                    break;
                }
            },
            CommitDest::Browser { hash, .. } => {
                for tab in self.all_tabs_mut() {
                    if let TabKind::CommitGraph {
                        commits,
                        selected,
                        detail,
                        detail_loading_since,
                        files,
                        ..
                    } = &mut tab.kind
                    {
                        let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                        if selected_hash != Some(hash.as_str()) {
                            continue;
                        }
                        if detail.as_ref().is_some_and(|d| d.hash == hash) {
                            files.loading_since = None;
                            files.error = Some(message.to_string());
                        } else {
                            *detail_loading_since = None;
                        }
                    }
                }
            },
        }
    }

    /// Build and open a compare tab from a resolved range and its changes. An empty
    /// range (identical endpoints) opens with a "no changes" state rather than nothing.
    pub(super) fn open_compare_tab(
        &mut self,
        base_label: String,
        head_label: String,
        merge_base: bool,
        changes: Vec<PreparedChange>,
    ) {
        if changes.is_empty() {
            self.status = Some(format!("no changes between {base_label} and {head_label}"));
        }
        let files = CommitFiles::ready(commit_file_views(changes));
        self.push_tab(Tab::compare(base_label, head_label, merge_base, files));
    }

    /// Apply the forge's verification verdict to every open commit view for `hash` —
    /// both standalone commit tabs and the graph browser's shown detail.
    pub(super) fn apply_commit_verification(&mut self, hash: &str, status: GithubVerification) {
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Commit { detail, files, .. } if detail.hash == hash => {
                    files.verification = Some(status.clone());
                },
                TabKind::CommitGraph {
                    detail: Some(detail),
                    files,
                    ..
                } if detail.hash == hash => files.verification = Some(status.clone()),
                _ => {},
            }
        }
    }
}

/// Wrap backend-prepared commit/range changes for display.
fn commit_file_views(changes: Vec<PreparedChange>) -> Vec<FileView> {
    changes.into_iter().map(FileView::new).collect()
}

impl App {
    /// The hash of the graph browser's selected commit.
    fn selected_graph_commit(&self) -> Option<String> {
        match &self.tabs.get(self.active)?.kind {
            TabKind::CommitGraph {
                commits, selected, ..
            } => commits.get(*selected).map(|commit| commit.hash.clone()),
            _ => None,
        }
    }

    /// Open the action menu for the selected commit (`.` in the graph).
    pub(super) fn commit_graph_menu(&mut self) {
        let Some(hash) = self.selected_graph_commit() else {
            return;
        };
        let short: String = hash.chars().take(7).collect();
        self.overlay = Some(Overlay::commands(
            format!("Commit {short}"),
            vec![
                Command::CommitGraphTag,
                Command::CommitGraphCherryPick,
                Command::CommitGraphRevert,
                Command::CommitGraphCheckout,
                Command::CommitGraphResetSoft,
                Command::CommitGraphResetMixed,
                Command::CommitGraphResetHard,
                Command::CommitGraphMarkBase,
                Command::CommitGraphCompare,
            ],
        ));
    }

    /// Prompt for a tag name on the selected commit.
    pub(super) fn commit_graph_tag(&mut self) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.overlay = Some(Overlay::text(
            format!("Tag name for {short}"),
            crate::overlay::TextPurpose::TagCreate { rev },
        ));
    }

    /// Run one non-destructive graph operation on the selected commit.
    pub(super) fn commit_graph_op(&mut self, verb: &str, action: impl FnOnce(String) -> VcsAction) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.status = Some(format!("{verb}: {short}"));
        self.run_vcs_action(action(rev));
    }

    /// Hard reset requires typing `reset` — it discards local changes.
    pub(super) fn commit_graph_reset_hard(&mut self) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.overlay = Some(Overlay::text(
            format!("Type reset to hard-reset to {short} (discards local changes)"),
            crate::overlay::TextPurpose::ConfirmResetHard { rev },
        ));
    }

    /// Fetch (and prune) every remote the snapshot knows.
    pub(super) fn scm_fetch(&mut self) {
        let remotes: Vec<String> = self
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
        if remotes.is_empty() {
            self.status = Some("fetch: no remotes".to_string());
            return;
        }
        for remote in remotes {
            self.send(SessionCommand::VcsAction {
                action: VcsAction::Fetch { remote },
            });
        }
        self.status = Some("fetching…".to_string());
    }
}
