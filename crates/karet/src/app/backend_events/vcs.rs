//! VCS-domain backend-event handlers: status, operations, blame, logs, and
//! the commit family. Called only from the [`App::on_backend_event`] router.

use super::*;

impl App {
    /// Adopt a fresh working-tree status, dropping every cache it invalidates.
    pub(super) fn on_vcs_status(
        &mut self,
        staged: Vec<ChangeSummary>,
        working: Vec<ChangeSummary>,
    ) {
        // Commits and branch switches change every cached repository
        // fact (head, tracked-ness); drop them and re-resolve on demand.
        self.remote_facts.clear();

        self.live_blame = None;
        self.pending_blame = None;
        self.failed_blame = None;
        self.apply_vcs_status(staged, working);
    }

    /// Fill a reserved conflict view's two committed sides.
    pub(super) fn on_merge_conflict_ready(
        &mut self,
        id: Option<RequestId>,
        path: &Path,
        current: String,
        incoming: String,
    ) {
        let destination = id.and_then(|request| self.pending_merge_conflicts.remove(&request));
        if let Some((view, expected)) = destination
            && expected == path
            && let Some(tab) = self.all_tabs_mut().find(|tab| tab.view == view)
            && let Some(conflict) = tab.merge_conflict.as_mut()
        {
            conflict.finish(current, incoming);
        }
    }

    /// Finish a repository mutation: route recoverable failures to their
    /// confirmation prompts, report outcomes, and resume a parked quit.
    pub(super) fn on_vcs_operation_finished(
        &mut self,
        action: VcsAction,
        outcome: Option<VcsOutcome>,
        error: Option<String>,
    ) {
        self.scm.operation = None;
        let resume_quit = self.operation_blocker.take().is_some();
        if let Some(error) = error {
            match action {
                VcsAction::SwitchBranch(target)
                    if error.contains("local changes")
                        || error.contains("would be overwritten") =>
                {
                    self.confirm_action(
                        "Switch blocked by local changes",
                        "Git refused the switch because uncommitted changes would \
                         be overwritten. Stashing sets them aside, switches, and \
                         leaves the stash for you to pop.",
                        "Stay here",
                        "Stash and switch",
                        ConfirmAction::StashAndSwitch(target),
                    );
                },
                VcsAction::UndoCommit {
                    allow_upstream: false,
                } if error.contains("already present upstream") => {
                    self.confirm_action(
                        "Undo a commit that is already pushed?",
                        "This commit exists on the remote. Undoing it here rewrites \
                         local history, so the branch will need a force-push and \
                         anyone who pulled it will diverge.",
                        "Keep the commit",
                        "Undo anyway",
                        ConfirmAction::UndoPublishedCommit,
                    );
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
                    self.push_tab(Tab::stash_preview(&reference, patch));
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
    }

    /// Adopt a blame answer if it still matches the pending request and the
    /// active document, version, and cursor line.
    pub(super) fn on_blame_result(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        line: u32,
        attribution: Option<BlameAttribution>,
    ) {
        let matches = self.pending_blame.as_ref().is_some_and(|pending| {
            Some(pending.0) == id && pending.1 == doc && pending.2 == version && pending.3 == line
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
    }

    /// Accumulate open-pull-request pages; the last page opens the picker.
    pub(super) fn on_pull_requests(
        &mut self,
        id: Option<RequestId>,
        remote: String,
        items: Vec<PullRequestSummary>,
        next_page: Option<u32>,
    ) {
        if id.is_some() && id == self.pending_pull_requests {
            self.pending_pull_requests = None;
            self.pull_request_items.extend(items);
            if let Some(page) = next_page {
                self.pending_pull_requests = self.send(SessionCommand::PullRequests {
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
    }

    /// A page requested by the graph browser fills it; anything else is the
    /// sidebar log.
    pub(super) fn on_vcs_log(
        &mut self,
        id: Option<RequestId>,
        skip: usize,
        commits: Vec<Commit>,
        has_more: bool,
        labels: std::collections::HashMap<String, Vec<karet_vcs::RefLabel>>,
    ) {
        self.scm.ref_labels = labels;
        if let Some(view) = id.and_then(|request| self.graph_log_reqs.remove(&request)) {
            self.apply_graph_log(view, skip, commits, has_more);
        } else {
            self.apply_vcs_log(skip, commits, has_more);
        }
    }

    /// File history fills exactly the surface that asked: the graph browser,
    /// or the With Revision diff-target picker.
    pub(super) fn on_file_history(
        &mut self,
        id: Option<RequestId>,
        skip: usize,
        commits: Vec<Commit>,
        has_more: bool,
    ) {
        if let Some(view) = id.and_then(|request| self.graph_log_reqs.remove(&request)) {
            self.apply_graph_log(view, skip, commits, has_more);
        } else if id.is_some() && id == self.pending_history_picker {
            self.pending_history_picker = None;
            self.apply_history_picker(commits);
        }
    }

    /// Reset the commit editor and report a completed commit.
    pub(crate) fn on_committed(&mut self, oid: &str) {
        self.commit_input = CommitInput::default();
        // The commit landed, so the box is empty and the draft that a generated
        // message once replaced belongs to a message that is now history.
        // Leaving the undo armed would let Ctrl+Z resurrect it into the *next*
        // commit's box.
        self.ai_commit.undo = None;
        self.ai_commit.state = crate::app::scm::aicommit::AiCommitState::Idle;
        let short: String = oid.chars().take(7).collect();
        self.notify(
            Severity::Information,
            NotificationKind::Vcs,
            format!("committed {short}"),
        );
    }

    /// Route resolved commit metadata to the surface that asked for it.
    pub(super) fn on_commit_detail_ready(
        &mut self,
        id: Option<RequestId>,
        detail: Box<CommitDetail>,
    ) {
        match id.and_then(|i| self.pending_commit_detail.get(&i).copied()) {
            Some(view) => self.fill_commit_metadata(view, detail),
            None if id.is_none() => self.open_commit_metadata_tab(detail),
            None => {},
        }
    }

    /// Route a fully prepared commit to the surface that asked for it.
    pub(super) fn on_commit_ready(
        &mut self,
        id: Option<RequestId>,
        detail: Box<CommitDetail>,
        changes: Vec<PreparedChange>,
    ) {
        let commit_hash = detail.hash.clone();
        match id.and_then(|request| self.pending_commit_detail.remove(&request)) {
            Some(view) => self.fill_commit_tab(view, detail, changes),
            None if id.is_none() => self.open_commit_tab(detail, changes),
            None => {},
        }
        self.apply_review_flags(&commit_hash);
    }

    /// Apply a forge verdict if its owning view still shows the commit.
    pub(super) fn on_commit_verification(
        &mut self,
        id: Option<RequestId>,
        hash: &str,
        status: GithubVerification,
    ) {
        let owner = id.and_then(|request| self.pending_commit_verification.remove(&request));
        if owner.is_some_and(|(view, expected)| {
            expected == hash && self.all_tabs().any(|tab| tab.view == view)
        }) {
            self.apply_commit_verification(hash, status);
        }
    }
}
