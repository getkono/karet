mod documents;
mod shell;
mod vcs;

use super::*;

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
        let loading = self
            .pendings()
            .into_iter()
            .filter_map(|pending| pending.wake(now))
            .min();
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
            nested_repositories,
            operation,
            reveal,
        ]
        .into_iter()
        .flatten()
        .min()
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
            SessionEvent::Opened { doc, .. } => self.on_opened(id, doc),
            SessionEvent::DocumentSettingsChanged { doc, settings } => {
                self.docs.settings.insert(doc, settings);
            },
            SessionEvent::DiagnosticsPublished { doc, diagnostics } => {
                self.on_diagnostics_published(doc, diagnostics);
            },
            SessionEvent::LatexBuildFinished {
                doc,
                pdf,
                diagnostics,
                error,
                ..
            } => self.finish_latex_build(id, doc, pdf, diagnostics, error),
            SessionEvent::Closed { doc } => self.on_document_closed(doc),
            SessionEvent::Symbols { doc, symbols } => self.on_symbols(doc, symbols),
            SessionEvent::Completions {
                doc,
                version,
                items,
            } => self.on_completions(id, doc, version, items),
            SessionEvent::HoverResult { hover } => self.on_hover_result(id, hover),
            SessionEvent::WakatimeStatus { text } => self.wakatime_status = Some(text),
            SessionEvent::DebugState { state, detail } => self.on_debug_state(state, detail),
            SessionEvent::DebugStopped {
                reason,
                thread: _,
                path,
                line,
            } => self.on_debug_stopped(&reason, path, line),
            SessionEvent::DebugContinued => {},
            SessionEvent::DebugOutput { category, text } => self.on_debug_output(category, text),
            SessionEvent::DebugBreakpoints { path, breakpoints } => {
                self.on_debug_breakpoints(path, &breakpoints);
            },
            SessionEvent::ManifestHints {
                doc,
                version,
                hints,
            } => {
                self.docs.manifest_hints.insert(doc, (version, hints));
            },
            SessionEvent::Definitions { locations } => self.on_definitions(id, locations),
            SessionEvent::LanguageServerInstallRequired { server } => {
                self.prompt_language_server_install(server);
            },
            SessionEvent::LanguageServerStatus { servers } => {
                self.show_language_server_status(id, servers);
            },
            SessionEvent::LanguageServerUpdatePlan { plan, changes } => {
                self.prompt_language_server_updates(id, plan, changes);
            },
            SessionEvent::LanguageServerProgress {
                server,
                downloaded,
                total,
            } => {
                self.show_language_server_progress(server, downloaded, total);
            },
            SessionEvent::LanguageServerChanged {
                server,
                version,
                restart_required,
            } => {
                self.finish_language_server_change(id, server, version, restart_required);
            },
            SessionEvent::LanguageServerRemoved {
                server,
                cleanup_pending,
            } => self.finish_language_server_remove(id, server, cleanup_pending),
            SessionEvent::LanguageServerRuntimeChanged {
                server,
                root,
                state,
                error,
            } => self.update_language_server_runtime(server, root, state, error),
            SessionEvent::Saved { doc } => self.on_saved(doc),
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
            SessionEvent::NotUtf8 { path } => self.on_not_utf8(id, path),
            SessionEvent::FsChanged { paths } => self.on_fs_changed(&paths),
            SessionEvent::ConfigChanged { report } => self.on_config_changed(*report),
            SessionEvent::Progress { message, .. } => self.status = Some(message),
            // The single high-up funnel: every backend-reported condition becomes a
            // notification, so nothing is silently dropped.
            SessionEvent::Notification {
                severity,
                kind,
                message,
            } => self.on_notification(id, severity, kind, message),
            SessionEvent::VcsStatus { staged, working } => self.on_vcs_status(staged, working),
            SessionEvent::MergeConflictReady {
                path,
                current,
                incoming,
            } => self.on_merge_conflict_ready(id, &path, current, incoming),
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
            } => self.on_vcs_operation_finished(action, outcome, error),
            SessionEvent::BlameResult {
                doc,
                version,
                line,
                attribution,
            } => self.on_blame_result(id, doc, version, line, attribution),
            SessionEvent::PullRequests {
                remote,
                items,
                next_page,
            } => self.on_pull_requests(id, remote, items, next_page),
            SessionEvent::VcsLog {
                skip,
                commits,
                has_more,
                labels,
            } => self.on_vcs_log(id, skip, commits, has_more, labels),
            SessionEvent::FileHistory {
                skip,
                commits,
                has_more,
                ..
            } => self.on_file_history(id, skip, commits, has_more),
            SessionEvent::VcsCommitsPrepended { commits } => {
                self.apply_vcs_commits_prepended(commits);
            },
            SessionEvent::Committed { oid } => self.on_committed(&oid),
            SessionEvent::CommitMessageGenerated { message } => {
                self.on_commit_message_generated(message);
            },
            SessionEvent::SwapsFound { swaps } => self.arm_swap_recovery(swaps),
            SessionEvent::CommitDetailReady { detail } => self.on_commit_detail_ready(id, detail),
            SessionEvent::CommitReady { detail, changes } => {
                self.on_commit_ready(id, detail, changes);
            },
            SessionEvent::RangeReady {
                base_label,
                head_label,
                merge_base,
                changes,
            } => self.open_compare_tab(base_label, head_label, merge_base, changes),
            SessionEvent::CommitVerification { hash, status } => {
                self.on_commit_verification(id, &hash, status);
            },
            SessionEvent::GithubAvailability { repository, auth } => {
                self.apply_github_availability(repository, auth);
            },
            SessionEvent::GithubIssues { page } => self.apply_github_issues(id, page),
            SessionEvent::GithubPullRequests { page } => {
                self.apply_github_pull_requests(id, page);
            },
            SessionEvent::GithubActions { workflows, runs } => {
                self.apply_github_actions(id, workflows, runs);
            },
            SessionEvent::GithubIssueMetadataReady { assignees } => {
                self.apply_github_issue_metadata(id, assignees);
            },
            SessionEvent::GithubIssueReady { issue, comments } => {
                self.apply_github_issue(id, issue, comments);
            },
            SessionEvent::GithubPullRequestReady {
                pull_request,
                comments,
                commits,
                checks,
                activity,
                activity_error,
            } => {
                self.apply_github_pull_request(
                    id,
                    pull_request,
                    comments,
                    crate::app::github::GithubPullRequestSupplement {
                        commits,
                        checks,
                        activity,
                        activity_error,
                    },
                );
            },
            SessionEvent::GithubError { operation, message } => {
                self.apply_github_error(id, operation, message);
            },
            SessionEvent::GraphReady { title, view, .. } => {
                let count = view.nodes.len();
                self.push_tab(Tab::graph(title, view));
                self.status = Some(format!("dependency graph: {count} package(s)"));
            },
            SessionEvent::LoadedConfig { report } => self.open_loaded_config(*report),
            SessionEvent::SearchResults { hits } => self.apply_search_results(hits),
            SessionEvent::SpellingScanProgress {
                hits,
                files_scanned,
            } => self.spelling_scan_progress(id, hits, files_scanned),
            SessionEvent::TodoScanProgress {
                hits,
                files_scanned,
            } => self.todo_scan_progress(id, hits, files_scanned),
            SessionEvent::SpellingUpdated { path, hits } => self.spelling_updated(&path, hits),
            SessionEvent::SpellingScanFinished {
                files_scanned,
                truncated,
                ..
            } => self.spelling_scan_finished(id, files_scanned, truncated),
            SessionEvent::TodoScanFinished {
                files_scanned,
                truncated,
                ..
            } => self.todo_scan_finished(id, files_scanned, truncated),
            SessionEvent::RemoteFacts { path, facts } => self.apply_remote_facts(path, facts),
            SessionEvent::ChangePrepared {
                path,
                staged,
                result,
            } => self.apply_change_prepared(&path, staged, result),
            SessionEvent::DiffPrepared { result, .. } => self.apply_diff_prepared(id, result),
            SessionEvent::DocumentConverted { path, markdown } => {
                self.apply_document_converted(id, &path, markdown);
            },
            SessionEvent::DictionaryWordAdded { word, path } => {
                self.dictionary_word_added(&word, &path);
            },
            SessionEvent::ProjectSettingsCreationRequired { word, path } => {
                self.overlay = Some(Overlay::text(
                    format!("Type create to add “{word}” and create {}", path.display()),
                    TextPurpose::ConfirmCreateProjectSettings { word, path },
                ));
            },
            SessionEvent::SearchReplaced {
                files_changed,
                replacements,
            } => {
                self.notify(
                    Severity::Information,
                    NotificationKind::System,
                    format!("replaced {replacements} occurrence(s) in {files_changed} file(s)"),
                );
                // Refresh so the (now empty, unless the replacement re-matches)
                // results reflect the edited files.
                self.run_global_search();
            },
            // Events answering commands this client never sends (hover, workspace
            // symbols, rename, format-on-save) fall through here until the
            // corresponding UI exists.
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
}
