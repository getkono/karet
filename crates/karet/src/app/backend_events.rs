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
        let spinner = (!self.pending_saves.is_empty()).then_some(Spinner::FRAME_INTERVAL);
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
        // Generation runs for seconds with no output to stream, so its spinner is
        // the only progress there is — it has to keep animating without input.
        let ai_commit = self.ai_commit_next_wake(now);
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
            ai_commit,
            operation,
            reveal,
            self.drag_autoscroll_wake(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Handle a backend event: correlate opens to tabs, surface save/progress status.
    pub(super) fn on_backend_event(&mut self, id: Option<RequestId>, event: SessionEvent) {
        self.on_backend_event_inner(id, event);
        // An answering event is what assigns the active tab its document id, so
        // this is where queued `--command` work becomes servable.
        self.run_startup_commands_when_ready();
    }

    fn on_backend_event_inner(&mut self, id: Option<RequestId>, event: SessionEvent) {
        if id.is_some_and(|request| self.cancelled_requests.contains(&request)) {
            return;
        }
        if let Some(request) = id {
            self.nested_repository_pending.remove(&request);
        }
        // A save's answering event clears its tab spinner. During "save all & quit",
        // only successful Saved responses may let the quit continue; a refused or
        // failed save keeps the app open with the dirty buffer intact. Which
        // document failed is part of the answer: a parked close owes writes only
        // for the documents it would drop.
        let mut failed_save: Option<DocumentId> = None;
        if let Some(req) = id
            && let Some(pending) = self.pending_saves.remove(&req)
        {
            let doc = pending.doc;
            if !matches!(event, SessionEvent::Saved { doc: saved } if saved == doc) {
                failed_save = Some(doc);
            }
            for tab in self.all_tabs_mut() {
                if matches!(&tab.kind, TabKind::Code { doc: Some(d), .. } if *d == doc) {
                    tab.saving_since = None;
                }
            }
        }
        // Cancel the parked close only for a write *it* would drop. A save deferred
        // on a formatter answers seconds later and for any document, so a global
        // "some save failed" read let document A's failure cancel the close of an
        // unrelated tab B whose own save had already landed. This is the mirror of
        // `saves_at_risk`, which scopes the *park* the same way.
        if let Some(doc) = failed_save
            && let Some(request) = self.saving_close
            && self.fully_dropped_docs(request).contains(&doc)
        {
            self.saving_close = None;
            let verb = if matches!(request, CloseRequest::Quit) {
                "quit"
            } else {
                "close"
            };
            // The batch is over, so retire its progress card here: unrelated saves
            // may still be in flight, and the blanket retirement below only fires
            // once *nothing* is pending.
            self.notifications.dismiss_tagged(Self::SAVE_BATCH_TAG);
            // Untagged, so retiring the batch's own card cannot take the reason the
            // batch was abandoned down with it.
            self.notify(
                Report::Failure,
                NotificationKind::Io,
                format!("{verb} cancelled: save failed"),
            );
        }
        // The branch switch stays scoped to every save: it rewrites the whole
        // worktree, so any unwritten buffer is a buffer it would clobber -- there is
        // no subset of documents it can be said not to own.
        if failed_save.is_some() && self.vcs_after_save.take().is_some() {
            self.notify(
                Report::Failure,
                NotificationKind::Io,
                "branch switch cancelled: save failed",
            );
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
            SessionEvent::InlayHints {
                doc,
                version,
                hints,
            } => self.on_inlay_hints(id, doc, version, hints),
            SessionEvent::HoverResult { hover } => self.on_hover_result(id, hover),
            SessionEvent::WakatimeStatus { text } => self.wakatime_status = Some(text),
            SessionEvent::DebugState {
                state,
                severity,
                detail,
            } => self.on_debug_state(state, severity, detail),
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
            SessionEvent::DebugStack { frames } => self.on_debug_stack(id, frames),
            SessionEvent::DebugScopes { frame, scopes } => self.on_debug_scopes(id, frame, scopes),
            SessionEvent::DebugVariables {
                reference,
                variables,
            } => self.on_debug_variables(id, reference, variables),
            SessionEvent::DebugEvaluated { result, .. } => self.on_debug_evaluated(id, result),
            SessionEvent::NotebookKernelStatus { severity, text, .. } => {
                let tier = Report::from_severity(severity);
                // Two feeds, two tags. Progress shares one so a thirty-cell run
                // rewrites a single card instead of stacking thirty; anything
                // that waits to be dismissed gets the other, so the next tick
                // cannot erase it and a re-run that keeps raising still collapses
                // to one card. The split is on whether the card expires, not on
                // the tier, because an alert is persistent too.
                let ends_the_run = tier.timeout().is_none();
                let tag = if ends_the_run {
                    // The run is over; a "running cell 3/4" left beside it is stale.
                    self.notifications.dismiss_tagged(Self::NOTEBOOK_KERNEL_TAG);
                    Self::NOTEBOOK_KERNEL_FAILURE_TAG
                } else {
                    Self::NOTEBOOK_KERNEL_TAG
                };
                self.notify_tagged(
                    tier,
                    NotificationKind::System,
                    format!("notebook: {text}"),
                    Some(tag.to_string()),
                );
            },
            SessionEvent::NotebookCellDone { .. } => {},
            SessionEvent::ManifestHints {
                doc,
                version,
                hints,
            } => {
                self.docs.manifest_hints.insert(doc, (version, hints));
                // The hints landing is the answer to an explicit re-check, so its
                // card goes with them. Unconditional: hints also arrive unasked
                // for, and dismissing a tag nobody raised is a no-op.
                self.notifications.dismiss_tagged(Self::DEPS_CHECK_TAG);
            },
            SessionEvent::Definitions { locations } => self.on_definitions(id, locations),
            SessionEvent::LanguageServerInstallRequired {
                server,
                language,
                enabled,
            } => {
                self.prompt_language_server_install(server, &language, enabled);
            },
            // No notification. karet cannot install these, so the old toast could
            // only tell the user to get the executable onto `PATH` themselves --
            // advice that named a condition rather than offering a way out of it,
            // and interrupted them to do it. The badge now carries the condition,
            // and clicking it opens the manager, whose row for this provider names
            // the SDK or toolchain required and the executable karet looked for.
            // All this event does is make the badge show up promptly.
            SessionEvent::LanguageServerManualInstallRequired { .. } => {
                self.refresh_language_server_inventory();
            },
            SessionEvent::LanguageServerStatus { servers } => {
                self.show_language_server_status(id, servers);
            },
            SessionEvent::LanguageServerInventoryStale => {
                self.language_server_inventory_stale();
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
            } => {
                self.update_language_server_runtime(server, root, state, error);
                // A provider that has just come up can answer things it could
                // not a moment ago. Anything already answered *because no
                // server was running* is not a real answer, and without this
                // the empty set returned during startup would be cached as
                // authoritative and never re-asked.
                self.invalidate_inlay_coverage();
            },
            SessionEvent::Saved { doc } => {
                self.reindex_saved_seam(doc);
                self.on_saved(doc);
            },
            // The fresh content arrives via the snapshot stream; just note it.
            SessionEvent::Reloaded { .. } => {
                self.notify(Report::Outcome, NotificationKind::Io, "reloaded from disk");
            },
            // An alert, not a refusal: nothing the user just did caused it, and an
            // unsaved-vs-disk conflict must wait to be read rather than expire.
            SessionEvent::ExternalConflict { .. } => {
                self.notify(
                    Report::Alert,
                    NotificationKind::Io,
                    "file changed on disk — you have unsaved changes",
                );
            },
            SessionEvent::NotUtf8 { path } => self.on_not_utf8(id, path),
            SessionEvent::FsChanged { paths } => self.on_fs_changed(&paths),
            SessionEvent::ConfigChanged { report } => self.on_config_changed(*report),
            // Language-server chatter (jdtls-style `language/status` during a long
            // first import). Tagged so successive ticks rewrite one card instead of
            // stacking, and on the self-expiring `Activity` tier because the stream
            // just stops — there is no event that says the import finished.
            SessionEvent::Progress { message, .. } => self.notify_tagged(
                Report::Activity,
                NotificationKind::Lsp,
                message,
                Some(Self::LSP_STATUS_TAG.to_string()),
            ),
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
            SessionEvent::CommitOutput { lines } => self.on_commit_output(lines),
            SessionEvent::Committed { oid } => self.on_committed(&oid),
            SessionEvent::CommitCancelled => self.commit_console_cancelled(),
            SessionEvent::CommitMessageGenerated { message } => {
                self.on_commit_message_generated(id, message);
            },
            SessionEvent::CommitMessageFailed { message } => {
                self.on_commit_message_failed(id, message);
            },
            SessionEvent::AiCommitAvailability { status } => {
                self.on_ai_commit_availability(*status);
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
                self.notify(
                    Report::Outcome,
                    NotificationKind::System,
                    format!("dependency graph: {count} package(s)"),
                );
            },
            SessionEvent::LoadedConfig { report } => self.open_loaded_config(*report),
            SessionEvent::SeamIndexed { summary, nodes } => {
                self.on_seam_indexed(id, summary, nodes);
            },
            SessionEvent::SeamPackageIndexed {
                order,
                root,
                nodes,
                unresolved_modules,
            } => self.on_seam_package_indexed(id, order, &root, nodes, unresolved_modules),
            SessionEvent::SeamIndexFinished {
                summary,
                parsed,
                files,
            } => self.on_seam_index_finished(id, summary, parsed, files),
            SessionEvent::SeamIndexFailed { message } => self.on_seam_index_failed(id, message),
            SessionEvent::SeamQueryResult { nodes, error, .. } => {
                self.on_seam_query_result(id, nodes, error);
            },
            SessionEvent::SeamNodeDetail {
                node,
                edges,
                preview,
            } => {
                self.on_seam_node_detail(id, node, edges, preview);
            },
            SessionEvent::SearchProgress {
                hits,
                files_scanned,
                matches_found,
            } => self.search_progress(id, hits, files_scanned, matches_found),
            SessionEvent::SearchFinished {
                files_scanned,
                matches_found,
                truncated,
                cancelled: _,
                error,
            } => self.search_finished(id, files_scanned, matches_found, truncated, error),
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
                self.confirm_action(
                    format!("Add “{word}” to the project dictionary?"),
                    format!(
                        "This workspace has no settings file yet. Accepting the \
                         word creates {} to hold it, which is checked in alongside \
                         the project.",
                        path.display()
                    ),
                    "Cancel",
                    "Create the file",
                    ConfirmAction::CreateProjectDictionary { word, path },
                );
            },
            SessionEvent::SearchReplaced {
                files_changed,
                replacements,
            } => {
                self.notify(
                    Report::Outcome,
                    NotificationKind::System,
                    format!("replaced {replacements} occurrence(s) in {files_changed} file(s)"),
                );
                // Refresh so the (now empty, unless the replacement re-matches)
                // results reflect the edited files.
                self.run_global_search();
            },
            // Events answering commands this client never sends (hover, workspace
            // symbols, rename) fall through here until the corresponding UI
            // exists.
            _ => {},
        }
        // A "save & close" runs the parked request once every issued save succeeds.
        // Either way the batch is over once nothing is pending, so its card goes —
        // the failure arms above already replaced it with their reason.
        if self.pending_saves.is_empty() {
            self.notifications.dismiss_tagged(Self::SAVE_BATCH_TAG);
        }
        // Release on the same set the park counted: the writes this request would
        // drop. Waiting on `pending_saves` globally held a tab close hostage to a
        // save for a document it never touches — a document whose formatter can
        // keep it in flight for seconds.
        if let Some(request) = self.saving_close
            && self.saves_at_risk(request) == 0
        {
            self.saving_close = None;
            // One card carries this tag, so a parked close's card replaced the
            // branch switch's if both were waiting. Retiring it here on the close
            // alone would leave the switch -- which rewrites the whole worktree --
            // running with no indicator at all, so restate what is still parked
            // rather than dismissing what is still true.
            if self.vcs_after_save.is_some() {
                self.notify_progress(
                    NotificationKind::Vcs,
                    Self::SAVE_BATCH_TAG.to_string(),
                    format!(
                        "saving {} editor(s) before switching…",
                        self.pending_saves.len()
                    ),
                    None,
                );
            } else {
                self.notifications.dismiss_tagged(Self::SAVE_BATCH_TAG);
            }
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
        self.notify(
            Report::Outcome,
            NotificationKind::System,
            "loaded settings opened",
        );
    }

    /// Arm the startup crash-recovery prompt for `swaps` left by a previous session.
    pub(super) fn arm_swap_recovery(&mut self, swaps: Vec<SwapInfo>) {
        if swaps.is_empty() {
            return;
        }
        let conflicts = swaps.iter().filter(|s| s.conflict).count();
        let files: Vec<PathBuf> = swaps.iter().map(|s| s.original.clone()).collect();
        let mut body = format!(
            "A previous session ended with unsaved changes to {}. Recovering \
             reopens each file with those changes; discarding deletes the backups.",
            describe_paths(&files, &self.root)
        );
        if conflicts > 0 {
            // The conflict is the whole reason this decision is not obvious: the
            // user has two versions and recovering silently drops one of them.
            body.push_str(&format!(
                " {conflicts} of them changed on disk since, so recovering those \
                 replaces the newer on-disk content."
            ));
        }
        // Open first, arm second — see `guarded_close`: opening declines the dialog
        // it replaces, and a decline is what clears these parked fields.
        self.confirm(ConfirmDialog::new(
            "Recover unsaved changes from a previous session?",
            body,
            vec![
                ConfirmChoice::custom("Decide later", Command::DismissSwaps),
                ConfirmChoice::custom("Recover them", Command::RecoverSwaps),
                ConfirmChoice::custom("Discard the backups", Command::DiscardSwaps),
            ],
        ));
        self.pending_swaps = Some(swaps);
    }
}
