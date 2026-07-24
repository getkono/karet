use super::*;

impl App {
    /// The active tab's commit graph browser, if it is one.
    pub(super) fn active_commit_graph(&mut self) -> Option<&mut TabKind> {
        let tab = self.tabs.get_mut(self.active)?;
        matches!(tab.kind, TabKind::CommitGraph { .. }).then_some(&mut tab.kind)
    }

    /// Move the browser's selection by `delta` (clamped), and request the newly
    /// selected commit's detail if it isn't already shown.
    pub(super) fn graph_select(&mut self, delta: i32) {
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
        let last = commits.len() - 1;
        let next = (*selected as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
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
                *loading_since = Some(Instant::now());
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
            self.graph_log_req = self.send_command_id(command).map(|id| (id, view));
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
        let stale_preparation: Vec<RequestId> = self
            .pending_commit_preparation
            .iter()
            .filter_map(|(request, pending)| {
                matches!(pending.destination, CommitDest::Browser { view: owner, .. } if owner == view)
                    .then_some(*request)
            })
            .collect();
        for request in stale_preparation {
            if let Some(pending) = self.pending_commit_preparation.remove(&request) {
                pending
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::Release);
            }
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
            files_loading_since,
            files_error,
            verification,
            detail_loading_since,
            ..
        }) = self.active_commit_graph()
        {
            *detail = None;
            files.clear();
            *files_loading_since = None;
            *files_error = None;
            *verification = None;
            *detail_loading_since = Some(Instant::now());
        }
        if let Some(id) = self.send_command_id(SessionCommand::CommitDetail { rev: hash.clone() }) {
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
        self.send_vcs(command);
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
                files_loading_since,
                files_error,
                verification,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                *slot = Some(detail.clone());
                files.clear();
                *files_loading_since = Some(Instant::now());
                *files_error = None;
                *verification = None;
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
        prepared: Vec<FileView>,
    ) {
        let hash = detail.hash.clone();
        let mut prepared = Some(prepared);
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
                files_loading_since,
                files_error,
                verification,
                detail_loading_since,
                ..
            } = &mut tab.kind
            {
                let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                if selected_hash != Some(hash.as_str()) {
                    continue;
                }
                let keep_verification = slot.as_ref().is_some_and(|d| d.hash == hash)
                    && verification.as_ref().is_some();
                *files = prepared.take().unwrap_or_default();
                *slot = Some(detail.clone());
                *files_loading_since = None;
                *files_error = None;
                if !keep_verification {
                    *verification = None;
                }
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

    /// Build and open a commit tab from a resolved [`CommitDetail`] and its changes,
    /// then fire the lazy GitHub verification fetch to upgrade the signature badge.
    pub(super) fn open_commit_tab(&mut self, detail: Box<CommitDetail>, changes: Vec<FileChange>) {
        let files = changes
            .into_iter()
            .map(|c| FileView::new(c, Section::Staged, self.syntax))
            .collect();
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, files));
        let view = self.tabs[self.active].view;
        self.request_commit_verification(view, hash);
    }

    /// Open a standalone commit tab with metadata visible while changed files are still
    /// loading. Used for unsolicited commit-detail events.
    pub(super) fn open_commit_metadata_tab(&mut self, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, Vec::new()));
        if let TabKind::Commit {
            files_loading_since,
            ..
        } = &mut self.tabs[self.active].kind
        {
            *files_loading_since = Some(Instant::now());
        }
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
                    TabKind::CommitLoading { scroll, .. } => *scroll,
                    TabKind::Commit { view, .. } => view.scroll,
                    _ => 0,
                };
                tab.kind = TabKind::Commit {
                    detail,
                    files: Vec::new(),
                    files_loading_since: Some(Instant::now()),
                    files_error: None,
                    verification: None,
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
        prepared: Vec<FileView>,
    ) {
        let mut files = Some(prepared);
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
                            files_loading_since,
                            files_error,
                            ..
                        } if slot.hash == hash => {
                            *slot = detail;
                            *current_files = files;
                            *files_loading_since = None;
                            *files_error = None;
                        },
                        _ => {
                            tab.kind = TabKind::Commit {
                                detail,
                                files,
                                files_loading_since: None,
                                files_error: None,
                                verification: None,
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
            self.send_command_id(SessionCommand::FetchCommitVerification { hash: hash.clone() })
        {
            self.pending_commit_verification
                .insert(request, (view, hash));
        }
    }

    /// Hand neutral commit changes to the app-local preparation worker. The originating
    /// request remains view-owned until the prepared result is adopted or cancelled.
    pub(super) fn prepare_commit_result(
        &mut self,
        request: RequestId,
        destination: CommitDest,
        detail: Box<CommitDetail>,
        changes: Vec<FileChange>,
    ) {
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = prepare::PrepareJob {
            request,
            changes,
            syntax: self.syntax,
            theme: self.theme.clone(),
            cancelled: cancelled.clone(),
        };
        self.pending_commit_preparation.insert(
            request,
            PendingCommitPreparation {
                destination,
                detail,
                cancelled,
            },
        );
        if self.prepare_tx.send(job).is_err() {
            self.pending_commit_preparation.remove(&request);
            self.status = Some("commit diff preparation worker is unavailable".to_owned());
        }
    }

    /// Adopt one completed preparation only while its exact request and view remain live.
    pub(super) fn on_prepare_result(&mut self, result: prepare::PrepareResult) {
        let Some(pending) = self.pending_commit_preparation.remove(&result.request) else {
            return;
        };
        if pending.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        match pending.destination {
            CommitDest::Browser { view, hash }
                if pending.detail.hash == hash && self.all_tabs().any(|tab| tab.view == view) =>
            {
                self.fill_graph_detail(view, pending.detail, result.files);
            },
            CommitDest::Browser { .. } => {},
            CommitDest::Tab { view } => {
                self.fill_commit_tab(view, pending.detail, result.files);
            },
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
                        TabKind::Commit {
                            files_loading_since,
                            files_error,
                            ..
                        } => {
                            *files_loading_since = None;
                            *files_error = Some(message.to_string());
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
                        files_loading_since,
                        files_error,
                        ..
                    } = &mut tab.kind
                    {
                        let selected_hash = commits.get(*selected).map(|c| c.hash.as_str());
                        if selected_hash != Some(hash.as_str()) {
                            continue;
                        }
                        if detail.as_ref().is_some_and(|d| d.hash == hash) {
                            *files_loading_since = None;
                            *files_error = Some(message.to_string());
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
        changes: Vec<FileChange>,
    ) {
        if changes.is_empty() {
            self.status = Some(format!("no changes between {base_label} and {head_label}"));
        }
        let files = changes
            .into_iter()
            .map(|c| FileView::new(c, Section::Staged, self.syntax))
            .collect();
        self.push_tab(Tab::compare(base_label, head_label, merge_base, files));
    }

    /// Apply the forge's verification verdict to every open commit view for `hash` —
    /// both standalone commit tabs and the graph browser's shown detail.
    pub(super) fn apply_commit_verification(&mut self, hash: &str, status: GithubVerification) {
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Commit {
                    detail,
                    verification,
                    ..
                } if detail.hash == hash => *verification = Some(status.clone()),
                TabKind::CommitGraph {
                    detail: Some(detail),
                    verification,
                    ..
                } if detail.hash == hash => *verification = Some(status.clone()),
                _ => {},
            }
        }
    }
}
