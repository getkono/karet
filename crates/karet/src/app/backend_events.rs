use super::*;

/// An edit waiting for the configured automatic-save trigger.
#[derive(Clone, Copy)]
pub(super) struct PendingAutoSave {
    /// Newest document version covered by this trigger.
    pub(super) version: u64,
    /// Debounce deadline, or `None` when waiting for an editor-focus change.
    pub(super) deadline: Option<Instant>,
}

/// One save request in flight.
#[derive(Clone, Copy)]
pub(super) struct PendingSave {
    pub(super) doc: DocumentId,
}

impl App {
    /// The soonest the event loop should wake for time-based UI: notification expiry,
    /// save-spinner animation, graphical-caret blink, delayed loading states, or an
    /// expiring hover reveal.
    /// `None` when the loop can park on its event sources alone.
    pub(super) fn next_wake(&self) -> Option<Duration> {
        let now = Instant::now();
        let notif = self.notifications.next_deadline(now);
        let spinner = (!self.pending_saves.is_empty()).then(|| Duration::from_millis(100));
        let auto_save = self
            .auto_save_pending
            .iter()
            .filter(|(doc, _)| !self.pending_saves.values().any(|save| save.doc == **doc))
            .filter_map(|(_, pending)| pending.deadline)
            .map(|deadline| deadline.saturating_duration_since(now))
            .min();
        let caret = self.graphics_caret_next_wake(now);
        let loading = self.loading_reveal_wake(now);
        let outline = self
            .active_outline_loading_since()
            .and_then(|since| loading_delay_remaining(since, now));
        let nested_repositories = self.nested_repository_next_wake(now);
        let operation = self
            .operation_blocker
            .as_ref()
            .map(|blocker| blocker.deadline.saturating_duration_since(now));
        // Wake to repaint (hiding the tooltip) when the commit-badge reveal expires.
        let reveal = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Commit {
                explain_since: Some(since),
                ..
            }) => COMMIT_REVEAL.checked_sub(since.elapsed()),
            _ => None,
        };
        [
            notif,
            spinner,
            auto_save,
            caret,
            loading,
            outline,
            nested_repositories,
            operation,
            reveal,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub(super) fn loading_reveal_wake(&self, now: Instant) -> Option<Duration> {
        let sidebar = (self.sidebar_visible && self.sidebar_panel == SidebarPanel::SourceControl)
            .then_some(self.scm.log_loading_since)
            .flatten()
            .and_then(|since| loading_delay_remaining(since, now));
        let repository = (self.sidebar_visible
            && self.sidebar_panel == SidebarPanel::SourceControl
            && self.scm.repository.is_none())
        .then_some(self.scm.repository_loading_since)
        .flatten()
        .and_then(|since| loading_delay_remaining(since, now));
        let tabs = self.all_tabs().filter_map(|tab| match &tab.kind {
            TabKind::CommitLoading {
                loading_since,
                error,
                ..
            } => error
                .is_none()
                .then(|| loading_delay_remaining(*loading_since, now))
                .flatten(),
            TabKind::Commit {
                files_loading_since,
                ..
            } => files_loading_since.and_then(|since| loading_delay_remaining(since, now)),
            TabKind::CommitGraph {
                loading_since,
                detail_loading_since,
                files_loading_since,
                ..
            } => [
                loading_since.and_then(|since| loading_delay_remaining(since, now)),
                detail_loading_since.and_then(|since| loading_delay_remaining(since, now)),
                files_loading_since.and_then(|since| loading_delay_remaining(since, now)),
            ]
            .into_iter()
            .flatten()
            .min(),
            _ => None,
        });
        [sidebar, repository]
            .into_iter()
            .flatten()
            .chain(tabs)
            .min()
    }

    /// Push a notification onto the center. Errors and warnings persist until
    /// dismissed; info and success auto-expire after a few seconds.
    pub(super) fn notify(
        &mut self,
        severity: Severity,
        kind: NotificationKind,
        title: impl Into<String>,
    ) {
        let timeout = match severity {
            Severity::Error | Severity::Warning => None,
            // Info, success (Hint), and any future severity auto-dismiss.
            _ => Some(Duration::from_secs(4)),
        };
        self.notifications.push(
            Notification {
                id: NotificationId(0),
                severity,
                kind,
                title: title.into(),
                body: None,
                tag: None,
                timeout,
                dismissable: true,
            },
            Instant::now(),
        );
    }

    /// Surface a dropped backend-submission error as a persistent notification, so a
    /// closed or wedged backend never fails silently.
    pub(super) fn notify_backend_error(&mut self, error: BackendError) {
        self.notify(
            Severity::Error,
            NotificationKind::System,
            format!("backend: {error}"),
        );
    }

    /// Handle a backend event: correlate opens to tabs, surface save/progress status.
    pub(super) fn on_backend_event(&mut self, id: Option<RequestId>, event: SessionEvent) {
        if id.is_some_and(|request| self.cancelled_requests.contains(&request)) {
            return;
        }
        if let Some(request) = id {
            self.nested_repository_pending.remove(&request);
        }
        // A save's answering event clears its tab spinner. During "save all & quit",
        // only successful Saved responses may let the quit continue; a refused or
        // failed save keeps the app open with the dirty buffer intact.
        let mut save_failed = false;
        if let Some(req) = id
            && let Some(pending) = self.pending_saves.remove(&req)
        {
            let doc = pending.doc;
            save_failed = !matches!(event, SessionEvent::Saved { doc: saved } if saved == doc);
            for tab in self.all_tabs_mut() {
                if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                    tab.saving_since = None;
                }
            }
        }
        if save_failed && let Some(request) = self.saving_close.take() {
            let verb = if matches!(request, CloseRequest::Quit) {
                "quit"
            } else {
                "close"
            };
            self.status = Some(format!("{verb} cancelled: save failed"));
        }
        if save_failed && self.vcs_after_save.take().is_some() {
            self.status = Some("branch switch cancelled: save failed".to_string());
        }
        match event {
            SessionEvent::Opened { doc, .. } => {
                self.open_docs.insert(doc);
                if let Some(req) = id
                    && let Some(path) = self.pending_open.remove(&req)
                {
                    for tab in self.all_tabs_mut() {
                        let bound = match &mut tab.kind {
                            TabKind::Code {
                                path: p, doc: d, ..
                            } => Some((p, d)),
                            _ => None,
                        };
                        if let Some((p, d)) = bound
                            && d.is_none()
                            && *p == path
                        {
                            *d = Some(doc);
                        }
                    }
                }
            },
            SessionEvent::DocumentSettingsChanged { doc, settings } => {
                self.document_settings.insert(doc, settings);
            },
            SessionEvent::DiagnosticsPublished { doc, diagnostics } => {
                if diagnostics.is_empty() {
                    self.document_diagnostics.remove(&doc);
                } else {
                    self.document_diagnostics.insert(doc, diagnostics);
                }
            },
            SessionEvent::Closed { doc } => {
                self.document_settings.remove(&doc);
                self.document_diagnostics.remove(&doc);
                self.document_symbols.remove(&doc);
                self.outline_versions.remove(&doc);
                self.outline_loading.remove(&doc);
            },
            SessionEvent::Symbols { doc, symbols } => {
                let version = self
                    .outline_loading
                    .remove(&doc)
                    .map(|(version, _)| version)
                    .or_else(|| {
                        self.all_tabs().find_map(|tab| match &tab.kind {
                            TabKind::Code {
                                doc: Some(candidate),
                                buffer,
                                ..
                            } if *candidate == doc => Some(buffer.version()),
                            _ => None,
                        })
                    });
                self.document_symbols.insert(doc, symbols);
                if let Some(version) = version {
                    self.outline_versions.insert(doc, version);
                }
                self.sync_outline_selection();
            },
            SessionEvent::Completions {
                doc,
                version,
                items,
            } => self.on_completions(id, doc, version, items),
            SessionEvent::Saved { doc } => {
                for tab in self.all_tabs_mut() {
                    if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                        tab.dirty = false;
                    }
                }
                self.status = Some("saved".to_string());
            },
            // The fresh content arrives via the snapshot stream; just note it.
            SessionEvent::Reloaded { .. } => {
                self.notify(
                    Severity::Information,
                    NotificationKind::Io,
                    "reloaded from disk",
                );
            },
            // A persistent warning: a transient status hint would vanish on the next
            // keystroke, but an unsaved-vs-disk conflict must not be missed.
            SessionEvent::ExternalConflict { .. } => {
                self.notify(
                    Severity::Warning,
                    NotificationKind::Io,
                    "file changed on disk — you have unsaved changes",
                );
            },
            // Full non-UTF-8 editing isn't supported: the tab requested a document
            // that will never arrive (no `Opened` follows), so leaving it as a
            // `doc: None` code tab would make every keystroke silently no-op. Fall
            // back to the same read-only hex view a corrupt CBOR file already uses.
            SessionEvent::NotUtf8 { path } => {
                if let Some(req) = id {
                    self.pending_open.remove(&req);
                }
                for tab in self.all_tabs_mut() {
                    let is_pending_for_path =
                        matches!(&tab.kind, TabKind::Code { path: p, doc: None, .. } if *p == path);
                    if is_pending_for_path && let Ok(bytes) = std::fs::read(&path) {
                        tab.kind = TabKind::Hex {
                            path: path.clone(),
                            bytes,
                            scroll: 0,
                        };
                        tab.markdown_preview = None;
                    }
                }
                self.notify(
                    Severity::Warning,
                    NotificationKind::Io,
                    format!("opened {} read-only: not valid UTF-8", path.display()),
                );
            },
            // Keep a live workspace search current: re-run it (which also
            // refreshes open-pane highlights) whenever something changes on
            // disk. No extra debouncing needed here — the watcher already
            // debounces at the source, and the result cap keeps a re-run cheap.
            SessionEvent::FsChanged { paths } => {
                self.invalidate_nested_repository_statuses(&paths);
                if !self.search.query.is_empty() {
                    self.run_global_search();
                }
            },
            SessionEvent::ConfigChanged { report } => {
                let report = *report;
                self.apply_loaded_config(report.clone(), false);
                for tab in self.all_tabs_mut() {
                    if let TabKind::LoadedConfig {
                        report: open_report,
                        ..
                    } = &mut tab.kind
                    {
                        *open_report = report.clone();
                    }
                }
                for diag in std::mem::take(&mut self.config_diagnostics) {
                    self.notify(
                        diag.severity,
                        NotificationKind::System,
                        format!("config: {}", diag.message),
                    );
                }
                let graphical_cursor_requested = self.tabs.get(self.active).is_some_and(|tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .graphical_cursor()
                        == Some(true)
                });
                if graphical_cursor_requested && !self.graphical_cursor_compatible() {
                    self.notify(
                        Severity::Error,
                        NotificationKind::System,
                        "graphical cursor is not compatible with this terminal",
                    );
                }
                let completion_enabled = self.tabs.get(self.active).is_some_and(|tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .completion()
                        .enabled()
                });
                if !completion_enabled {
                    self.dismiss_completion();
                }
            },
            SessionEvent::Progress { message, .. } => self.status = Some(message),
            // The single high-up funnel: every backend-reported condition becomes a
            // notification, so nothing is silently dropped.
            SessionEvent::Notification {
                severity,
                kind,
                message,
            } => {
                if id.is_some() && id == self.commit_input.pending {
                    self.commit_input.pending = None;
                }
                if id.is_some() && id == self.scm.repository_request {
                    self.scm.repository_request = None;
                    self.scm.repository_loading_since = None;
                }
                if id.is_some() && id == self.pending_pull_requests {
                    self.pending_pull_requests = None;
                    self.pull_request_items.clear();
                    self.pull_request_remote = None;
                }
                if let Some(pending) = self.pending_blame.filter(|pending| Some(pending.0) == id) {
                    self.pending_blame = None;
                    self.failed_blame = Some((pending.1, pending.2, pending.3));
                }
                if let Some(req) = id {
                    self.fail_pending_commit_detail(req, &message);
                }
                self.notify(severity, kind, message);
            },
            SessionEvent::VcsStatus { staged, working } => {
                self.live_blame = None;
                self.pending_blame = None;
                self.failed_blame = None;
                self.apply_vcs_status(staged, working);
            },
            SessionEvent::RepositorySnapshot { snapshot } => {
                self.scm.repository = Some(*snapshot);
                self.scm.repository_loading_since = None;
                self.scm.repository_request = None;
            },
            SessionEvent::NestedRepositoryStatus { path, summary } => {
                self.nested_repository_status.insert(path, summary);
            },
            SessionEvent::VcsOperationStarted { action } => {
                self.scm.operation = Some(action);
            },
            SessionEvent::VcsOperationFinished {
                action,
                outcome,
                error,
            } => {
                self.scm.operation = None;
                let resume_quit = self.operation_blocker.take().is_some();
                if let Some(error) = error {
                    match action {
                        VcsAction::SwitchBranch(target)
                            if error.contains("local changes")
                                || error.contains("would be overwritten") =>
                        {
                            self.overlay = Some(Overlay::text(
                                "Switch blocked · type stash to stash changes and retry",
                                TextPurpose::StashAndSwitch { target },
                            ));
                        },
                        VcsAction::UndoCommit {
                            allow_upstream: false,
                        } if error.contains("already present upstream") => {
                            self.overlay = Some(Overlay::text(
                                "Commit is upstream · type undo to confirm soft reset",
                                TextPurpose::ConfirmPublishedUndo,
                            ));
                        },
                        _ => self.notify(Severity::Error, NotificationKind::Vcs, error),
                    }
                } else if let Some(outcome) = outcome {
                    match outcome {
                        VcsOutcome::NeedsPublish => {
                            self.publish_current_branch();
                        },
                        VcsOutcome::PullRequestUpdated => {
                            self.status = Some("pull request branch updated".to_string());
                        },
                        VcsOutcome::PullRequestCheckedOut { branch } => {
                            self.status = Some(format!("switched to {branch}"));
                        },
                        VcsOutcome::CommitUndone { commit, .. } => {
                            let short: String = commit.chars().take(7).collect();
                            self.status = Some(format!("undid commit {short}"));
                        },
                        VcsOutcome::StashCreated(true) => {
                            self.status = Some("stashed local changes".to_string());
                        },
                        VcsOutcome::StashCreated(false) => {
                            self.status = Some("stash: no local changes".to_string());
                        },
                        VcsOutcome::StashPreview { reference, patch } => {
                            self.push_tab(Tab::stash_preview(reference, patch));
                        },
                        VcsOutcome::Completed => {
                            self.status = Some("source control operation completed".to_string());
                        },
                        _ => {},
                    }
                }
                if resume_quit {
                    self.guarded_close(CloseRequest::Quit);
                }
            },
            SessionEvent::BlameResult {
                doc,
                version,
                line,
                attribution,
            } => {
                let matches = self.pending_blame.as_ref().is_some_and(|pending| {
                    Some(pending.0) == id
                        && pending.1 == doc
                        && pending.2 == version
                        && pending.3 == line
                });
                if matches {
                    self.pending_blame = None;
                    self.failed_blame = None;
                    let current = self.tabs.get(self.active).is_some_and(|tab| {
                        matches!(&tab.kind, TabKind::Code { doc: Some(active), buffer, .. }
                            if *active == doc
                                && buffer.version() == version
                                && tab.editor.cursor().line == line)
                    });
                    if current {
                        self.live_blame = Some(LiveBlame {
                            doc,
                            version,
                            line,
                            attribution,
                        });
                    }
                }
            },
            SessionEvent::PullRequests {
                remote,
                items,
                next_page,
            } => {
                if id.is_some() && id == self.pending_pull_requests {
                    self.pending_pull_requests = None;
                    self.pull_request_items.extend(items);
                    if let Some(page) = next_page {
                        self.pending_pull_requests =
                            self.send_command_id(SessionCommand::PullRequests {
                                remote,
                                page,
                                per_page: 100,
                            });
                    } else if self.pull_request_items.is_empty() {
                        self.pull_request_remote = None;
                        self.status = Some(format!("{remote}: no open pull requests"));
                    } else {
                        let items = std::mem::take(&mut self.pull_request_items);
                        let remote = self.pull_request_remote.take().unwrap_or(remote);
                        self.overlay = Some(Overlay::pull_requests(remote, items));
                    }
                }
            },
            SessionEvent::VcsLog {
                skip,
                commits,
                has_more,
            } => {
                // A page requested by the graph browser fills it; anything else is the
                // sidebar log.
                if id.is_some_and(|request| {
                    self.graph_log_req
                        .is_some_and(|(pending, _)| pending == request)
                }) {
                    self.graph_log_req = None;
                    self.apply_graph_log(skip, commits, has_more);
                } else {
                    self.apply_vcs_log(skip, commits, has_more);
                }
            },
            SessionEvent::FileHistory {
                skip,
                commits,
                has_more,
                ..
            } => {
                // File history only ever fills the graph browser it was opened for.
                if id.is_some_and(|request| {
                    self.graph_log_req
                        .is_some_and(|(pending, _)| pending == request)
                }) {
                    self.graph_log_req = None;
                    self.apply_graph_log(skip, commits, has_more);
                }
            },
            SessionEvent::VcsCommitsPrepended { commits } => {
                self.apply_vcs_commits_prepended(commits);
            },
            SessionEvent::Committed { oid } => {
                self.commit_input = CommitInput::default();
                let short: String = oid.chars().take(7).collect();
                self.notify(
                    Severity::Information,
                    NotificationKind::Vcs,
                    format!("committed {short}"),
                );
            },
            SessionEvent::CommitMessageGenerated { message } => {
                self.commit_input.text = message;
                self.commit_input.cursor = self.commit_input.text.len();
                self.commit_input.scroll = 0;
                self.status = Some("commit message generated".to_string());
            },
            SessionEvent::SwapsFound { swaps } => self.arm_swap_recovery(swaps),
            SessionEvent::CommitDetailReady { detail } => {
                let dest = id.and_then(|i| self.pending_commit_detail.get(&i).cloned());
                match dest {
                    Some(CommitDest::Browser { hash, .. }) if detail.hash == hash => {
                        self.fill_graph_metadata(detail);
                    },
                    Some(CommitDest::Browser { .. }) => {},
                    Some(CommitDest::Tab { view }) => self.fill_commit_metadata(view, detail),
                    None if id.is_none() => self.open_commit_metadata_tab(detail),
                    _ => {},
                }
            },
            SessionEvent::CommitReady { detail, changes } => {
                match id.and_then(|i| self.pending_commit_detail.remove(&i)) {
                    Some(CommitDest::Browser { hash, .. }) if detail.hash == hash => {
                        self.fill_graph_detail(detail, changes);
                    },
                    Some(CommitDest::Browser { .. }) => {},
                    Some(CommitDest::Tab { view }) => self.fill_commit_tab(view, detail, changes),
                    None if id.is_none() => self.open_commit_tab(detail, changes),
                    _ => {},
                }
            },
            SessionEvent::RangeReady {
                base_label,
                head_label,
                merge_base,
                changes,
            } => self.open_compare_tab(base_label, head_label, merge_base, changes),
            SessionEvent::CommitVerification { hash, status } => {
                self.apply_commit_verification(&hash, status);
            },
            SessionEvent::GraphReady { title, view, .. } => {
                let count = view.nodes.len();
                self.push_tab(Tab::graph(title, view));
                self.status = Some(format!("dependency graph: {count} package(s)"));
            },
            SessionEvent::LoadedConfig { report } => self.open_loaded_config(*report),
            _ => {},
        }
        // A "save & close" runs the parked request once every issued save succeeds.
        if self.saving_close.is_some()
            && self.pending_saves.is_empty()
            && let Some(request) = self.saving_close.take()
        {
            self.execute_close(request);
        }
        if self.pending_saves.is_empty()
            && let Some(action) = self.vcs_after_save.take()
        {
            self.run_vcs_action(action);
        }
        self.request_live_blame();
    }

    pub(super) fn open_loaded_config(&mut self, report: LoadedConfig) {
        self.push_tab(Tab::loaded_config(report));
        self.status = Some("loaded settings opened".to_string());
    }

    /// Arm the startup crash-recovery prompt for `swaps` left by a previous session.
    pub(super) fn arm_swap_recovery(&mut self, swaps: Vec<SwapInfo>) {
        if swaps.is_empty() {
            return;
        }
        let conflicts = swaps.iter().filter(|s| s.conflict).count();
        let suffix = if conflicts > 0 {
            format!(" ({conflicts} changed on disk)")
        } else {
            String::new()
        };
        self.status = Some(format!(
            "recovered {} unsaved file(s) from a previous session{suffix} — \
             press r to recover, d to discard, any other key to dismiss",
            swaps.len()
        ));
        self.pending_swaps = Some(swaps);
    }

    /// Apply a document snapshot to the matching code tab(s): the snapshot is the
    /// render source of truth (buffer, highlights, the search text, and the
    /// unsaved-changes flag).
    pub(super) fn on_snapshot(&mut self, doc: DocumentId, snap: &DocSnapshot) {
        for tab in self.all_tabs_mut() {
            let matches = matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc);
            if !matches {
                continue;
            }
            if let TabKind::Code {
                buffer,
                highlights,
                semantic_blocks,
                folds,
                folded,
                text,
                next_version,
                syntax_errors,
                ..
            } = &mut tab.kind
            {
                // A slow-arriving snapshot must not regress a tab that has since
                // advanced further via `submit_edit`'s local speculative apply —
                // only the buffer/text catch up when the snapshot is at least as
                // new as what's already applied locally.
                if snap.version >= buffer.version() {
                    *buffer = snap.buffer.clone();
                    *text = snap.buffer.text();
                }
                *highlights = (*snap.highlights).clone();
                *semantic_blocks = (*snap.semantic_blocks).clone();
                *folds = (*snap.folds).clone();
                *syntax_errors = snap.syntax_error_lines.as_ref().clone();
                *next_version = (*next_version).max(snap.version);
                // Drop collapsed markers whose fold no longer starts where it did (an
                // edit shifted or removed it), so stale hidden lines can't linger.
                let starts: HashSet<u32> = folds.regions().iter().map(|r| r.start).collect();
                folded.retain(|line| starts.contains(line));
            }
            // The clean→dirty transition permanently promotes a preview tab (VS
            // Code behavior): once edited, it survives being navigated away from
            // instead of getting silently replaced by the next preview-opened file.
            if snap.dirty && !tab.dirty {
                tab.is_preview = false;
            }
            tab.dirty = snap.dirty;
            // Undo/redo snapshots carry the caret to jump to; ordinary edits carry
            // `None` so the optimistic placement from `submit_edit` is preserved.
            if let Some(cursor) = &snap.cursor {
                let heads: Vec<LineCol> = cursor.selections.iter().map(|s| s.head).collect();
                if !heads.is_empty() {
                    tab.editor.set_carets(&heads);
                    tab.editor.scroll_to(cursor.primary().head);
                }
            }
        }
        if snap.dirty {
            self.schedule_auto_save(doc, snap.version, Instant::now());
        } else if self
            .auto_save_pending
            .get(&doc)
            .is_some_and(|pending| pending.version <= snap.version)
        {
            self.auto_save_pending.remove(&doc);
        }
        self.request_active_outline();
        // If the find bar is open, an edit (e.g. a replace) just changed the buffer,
        // so recompute the match highlights against the fresh text.
        if self.find_open {
            self.run_find();
        }
        // Likewise for global search matches: a newly-opened or just-edited tab
        // should show its highlights immediately, not only after the next
        // explicit search re-run.
        if !self.search.query.is_empty() {
            self.refresh_search_decorations();
        }
        // An undo/redo snapshot may have moved the caret away from the popup's
        // anchor; re-validate it.
        self.reconcile_completion();
        self.request_live_blame();
    }
}
