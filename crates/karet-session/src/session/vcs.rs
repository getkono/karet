use super::*;

impl Session {
    /// Fetch a page of the commit log and emit it. Requests one extra commit to
    /// detect whether more remain, then trims to `limit`. A no-op without a repo.
    /// A requested page tags the answering event with `id`; a spontaneous reload
    /// (`id` is `None`) makes the client reset its loaded log to this first page.
    pub(super) fn emit_vcs_log(&mut self, id: Option<RequestId>, skip: usize, limit: usize) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match repo.log(skip, limit.saturating_add(1)) {
            Ok(mut commits) => {
                let has_more = commits.len() > limit;
                commits.truncate(limit);
                self.emit(
                    id,
                    Event::VcsLog {
                        skip,
                        commits,
                        has_more,
                    },
                );
            },
            Err(e) => self.emit(
                id,
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Lazily fetch a commit's GitHub "Verified" status through the shared async
    /// GitHub manager. A no-op when the workspace is ineligible or the feature is
    /// disabled.
    #[cfg(feature = "github")]
    pub(super) fn fetch_commit_verification(&mut self, id: RequestId, hash: String) {
        if self.github_repository.is_none() {
            return;
        }
        self.send_cancellable_github(id, super::github::GithubJob::Verification { hash });
    }

    /// Without the `github` feature, commit verification is unavailable — a no-op.
    #[cfg(not(feature = "github"))]
    pub(super) fn fetch_commit_verification(&mut self, _id: RequestId, _hash: String) {}
    /// Reconcile the commit log after a filesystem event. Reads the (cheap) `HEAD`
    /// hash; if the tip moved, prepends only the new commits, falling back to a fresh
    /// first page when history was rewritten or too many commits arrived at once.
    pub(super) fn reconcile_vcs_log(&mut self) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        let head = repo.head_hash().ok().flatten();
        if head == self.last_head {
            return; // The tip is unchanged — nothing to do.
        }
        let prev = self.last_head.take();
        self.last_head = head.clone();
        // The branch became unborn (e.g. a hard reset to before the first commit):
        // there is nothing to prepend, and the client's next open will refetch.
        if head.is_none() {
            return;
        }
        match repo.commits_since(prev.as_deref(), LOG_RECONCILE_CAP) {
            // A clean, bounded set of new commits anchored on a known tip → prepend.
            Ok(commits)
                if prev.is_some() && !commits.is_empty() && commits.len() < LOG_RECONCILE_CAP =>
            {
                self.emit(None, Event::VcsCommitsPrepended { commits });
            },
            // No prior anchor, or history was rewritten / a large batch arrived:
            // emit a fresh first page so the client resets its log cleanly.
            Ok(commits) if !commits.is_empty() => self.emit_vcs_log(None, 0, LOG_RELOAD_PAGE),
            // Tip moved but no newer commits (e.g. checkout to an ancestor): refresh.
            Ok(_) => self.emit_vcs_log(None, 0, LOG_RELOAD_PAGE),
            Err(_) => {},
        }
    }

    /// Compute the current `(staged, working)` change sets, or `None` when there is
    /// no repository. A read failure yields empty sets rather than erroring.
    pub(super) fn compute_vcs(&self) -> Option<(Vec<FileChange>, Vec<FileChange>)> {
        let repo = self.vcs.as_ref()?;
        let staged = repo.changes(VcsSelection::Staged, None).unwrap_or_default();
        let working = repo
            .changes(VcsSelection::Unstaged, None)
            .unwrap_or_default();
        Some((staged, working))
    }

    /// Recompute the source-control status and emit it. A requested refresh (`id`
    /// set) always emits; a spontaneous one (from a filesystem event) emits only
    /// when the status changed, collapsing event bursts and absorbing the feedback
    /// from the session's own index writes.
    pub(super) fn emit_vcs_status(&mut self, id: Option<RequestId>) {
        let Some(status) = self.compute_vcs() else {
            return;
        };
        if id.is_none() && self.last_vcs.as_ref() == Some(&status) {
            return;
        }
        let (staged, working) = status.clone();
        self.last_vcs = Some(status);
        self.emit(id, Event::VcsStatus { staged, working });
    }

    /// Run a write action against the repository, then force a fresh status (so the
    /// user always sees the result of their action). Failures surface as an
    /// [`Event::Notification`].
    pub(super) fn vcs_write(
        &mut self,
        id: RequestId,
        action: impl FnOnce(&Repository) -> Result<(), VcsError>,
    ) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match action(repo) {
            Ok(()) => {
                self.last_vcs = None;
                self.emit_vcs_status(Some(id));
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Commit the staged changes, emitting [`Event::Committed`] then a fresh status,
    /// or a [`Event::Notification`] on failure (e.g. conflicts or no identity).
    pub(super) fn commit(&mut self, id: RequestId, message: &str) {
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        match repo.commit(message) {
            Ok(oid) => {
                self.emit(Some(id), Event::Committed { oid });
                self.last_vcs = None;
                self.emit_vcs_status(Some(id));
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: e.to_string(),
                },
            ),
        }
    }

    /// Generate a commit message from the staged diff and emit it as an
    /// [`Event::CommitMessageGenerated`]. The generation is blocking (it shells out
    /// to the `claude` CLI), so it runs on a worker thread; failures — nothing
    /// staged, a disabled setting, or a generator error — surface as an
    /// [`Event::Notification`]. A no-op notification when the `aicommit` feature is off.
    #[cfg(feature = "aicommit")]
    pub(super) fn generate_commit_message(&mut self, id: RequestId) {
        let cfg = self.config.settings.git.ai_commit.clone();
        if !cfg.enabled {
            self.emit_vcs_notice(
                id,
                Severity::Warning,
                "AI commit messages are disabled (git.aiCommit.enabled)".to_string(),
            );
            return;
        }
        let Some(repo) = self.vcs.as_ref() else {
            return;
        };
        let diff = match repo.staged_diff() {
            Ok(diff) => diff,
            Err(e) => {
                self.emit_vcs_notice(id, Severity::Error, e.to_string());
                return;
            },
        };
        if diff.file_count == 0 || diff.patch.trim().is_empty() {
            self.emit_vcs_notice(
                id,
                Severity::Warning,
                "commit message: stage changes first".to_string(),
            );
            return;
        }

        let events = self.events.clone();
        // Off the actor thread: the CLI round-trip can take seconds. Fire-and-forget.
        std::thread::spawn(move || {
            let event = match crate::aicommit::generate(&diff, &cfg) {
                Ok(message) => Event::CommitMessageGenerated { message },
                Err(message) => Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::Vcs,
                    message: format!("commit message generation failed: {message}"),
                },
            };
            events.send((Some(id), event)).ok();
        });
    }

    /// Without the `aicommit` feature, message generation is unavailable — report it.
    #[cfg(not(feature = "aicommit"))]
    pub(super) fn generate_commit_message(&mut self, id: RequestId) {
        self.emit_vcs_notice(
            id,
            Severity::Warning,
            "AI commit messages are unavailable (built without the `aicommit` feature)".to_string(),
        );
    }

    /// Emit a source-control [`Event::Notification`] tagged with `id`.
    pub(super) fn emit_vcs_notice(&mut self, id: RequestId, severity: Severity, message: String) {
        self.emit(
            Some(id),
            Event::Notification {
                severity,
                kind: NotificationKind::Vcs,
                message,
            },
        );
    }
}
