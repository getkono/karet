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
                let labels = repo.ref_labels().unwrap_or_default();
                self.emit(
                    id,
                    Event::VcsLog {
                        skip,
                        commits,
                        has_more,
                        labels,
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

    /// Ask the VCS worker to recompute the source-control status and emit it. A
    /// requested refresh (`id` set) always emits; a spontaneous one (from a
    /// filesystem event) emits only when the status changed (the worker keeps the
    /// last emitted status), collapsing event bursts and absorbing the feedback
    /// from the session's own index writes.
    pub(super) fn emit_vcs_status(&mut self, id: Option<RequestId>) {
        if self.vcs.is_none() {
            return;
        }
        let _ = self
            .vcs_worker
            .send(crate::vcs_worker::VcsJob::Status { id });
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
            Ok(()) => self.emit_vcs_status(Some(id)),
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

    /// The AI-commit settings a run may actually use.
    ///
    /// Every field is the merged configuration except `binary`, which is dropped
    /// when it came from the project layer — `$GIT_ROOT/.karet/setting.jsonc`, a
    /// file that arrives with the repository. A repository may reasonably say
    /// which *model* to summarise its diffs with; letting it also name the
    /// *executable* would turn cloning it into running it, since the agents are
    /// probed and driven without further confirmation.
    ///
    /// The user and system layers keep the override: those are the machine
    /// owner's own files.
    pub(super) fn ai_commit_options(&self) -> crate::config::schema::AiCommit {
        let mut options = self.config.settings.git.ai_commit.clone();
        let from_project = self
            .config
            .loaded_config
            .explicit
            .get("git.aiCommit.binary")
            .is_some_and(|layer| *layer == crate::config::ConfigLayer::Project);
        if from_project {
            options.binary = None;
        }
        options
    }

    /// Generate a commit message from the staged diff, answering with
    /// [`Event::CommitMessageGenerated`] or [`Event::CommitMessageFailed`].
    ///
    /// Runs as a task rather than on the actor thread or the serialized VCS
    /// worker: the agent round-trip takes seconds, and neither the editor nor
    /// blame/log/diff may queue behind it. Reading the staged diff is blocking
    /// `git`, so it goes to a blocking thread too.
    ///
    /// Cancellation is cooperative on the way in and by drop on the way out —
    /// the agent adapters kill their child process when the future is dropped —
    /// and a cancelled request answers with nothing at all.
    #[cfg(feature = "aicommit")]
    pub(super) fn generate_commit_message(&mut self, id: RequestId) {
        let cfg = self.ai_commit_options();
        if !cfg.enabled {
            self.emit(
                Some(id),
                Event::CommitMessageFailed {
                    message: "AI commit messages are disabled (git.aiCommit.enabled)".to_string(),
                },
            );
            return;
        }
        let Some(root) = self.config.roots.first().cloned() else {
            self.emit(
                Some(id),
                Event::CommitMessageFailed {
                    message: "no workspace repository is open".to_string(),
                },
            );
            return;
        };
        // Same reason the availability probe checks: without a reactor there is
        // nothing to run the agent on, and answering is better than panicking.
        if tokio::runtime::Handle::try_current().is_err() {
            self.emit(
                Some(id),
                Event::CommitMessageFailed {
                    message: "this session has no async runtime to generate on".to_string(),
                },
            );
            return;
        }

        // A newer request supersedes an older one: one agent process per session,
        // never a pile of them racing to fill the same box.
        if let Some(previous) = self.ai_commit_request.replace(id) {
            self.cancellations.cancel(previous);
        }
        let cancel = self.cancellations.register(id);
        let events = self.events.clone();
        tokio::spawn(async move {
            let diff = tokio::task::spawn_blocking(move || {
                karet_vcs::Repository::discover(&root)
                    .and_then(|repo| repo.staged_diff())
                    .map_err(|error| error.to_string())
            })
            .await;
            let diff = match diff {
                Ok(Ok(diff)) => diff,
                Ok(Err(message)) => {
                    fail(&events, id, &cancel, message);
                    return;
                },
                Err(error) => {
                    fail(
                        &events,
                        id,
                        &cancel,
                        format!("reading the staged diff: {error}"),
                    );
                    return;
                },
            };
            if diff.file_count == 0 || diff.patch.trim().is_empty() {
                fail(&events, id, &cancel, "stage changes first".to_string());
                return;
            }
            // Reading the diff shells out to `git`, which is long enough to be
            // cancelled during. Check before launching the agent rather than
            // relying on `select!` to drop it: that would spawn the process only
            // to kill it a moment later.
            if cancel.is_cancelled() {
                return;
            }

            tokio::select! {
                // Dropping the generation future kills the agent process with it.
                () = cancel.cancelled() => {},
                result = crate::aicommit::generate(&diff, &cfg) => match result {
                    Ok(message) => {
                        if !cancel.is_cancelled() {
                            let _ = events.send((Some(id), Event::CommitMessageGenerated { message }));
                        }
                    },
                    Err(message) => fail(&events, id, &cancel, message),
                },
            }
        });
    }

    /// Without the `aicommit` feature there is nothing to run — say so where the
    /// request was made, and let [`Session::emit_ai_commit_availability`] report
    /// the same fact ahead of time.
    #[cfg(not(feature = "aicommit"))]
    pub(super) fn generate_commit_message(&mut self, id: RequestId) {
        self.emit(
            Some(id),
            Event::CommitMessageFailed {
                message: "this build has no AI commit support".to_string(),
            },
        );
    }

    /// Probe the agent CLIs and push [`Event::AiCommitAvailability`].
    ///
    /// Runs as a task: a probe launches a process, which must not stall the
    /// actor. `id` tags an explicitly requested probe and is `None` for the
    /// pushes that follow startup and settings reloads.
    ///
    /// Without a reactor there is nothing to launch a process with, so the
    /// configuration is reported unprobed rather than panicking — a session
    /// driven outside a runtime still gets a truthful answer.
    ///
    /// A probe *executes* the configured binary, and `binary` can be set by the
    /// project settings layer — a file inside the repository being opened. So it
    /// is not run for a workspace whose settings turn the feature off: opening a
    /// repository must never be enough, on its own, to run a program that
    /// repository named.
    #[cfg(feature = "aicommit")]
    pub(super) fn emit_ai_commit_availability(&mut self, id: Option<RequestId>) {
        let options = self.ai_commit_options();
        let events = self.events.clone();
        if !options.enabled || tokio::runtime::Handle::try_current().is_err() {
            self.emit_unprobed_ai_commit_availability(id);
            return;
        }
        tokio::spawn(async move {
            let mut agents = Vec::new();
            for agent in crate::config::schema::AiCommitAgent::ALL {
                // Probe each agent as *it* would be configured, so a picker can
                // report on one the user has not selected yet. A binary override
                // belongs to the selected agent alone.
                let probed = crate::config::schema::AiCommit {
                    agent,
                    binary: (agent == options.agent)
                        .then(|| options.binary.clone())
                        .flatten(),
                    ..options.clone()
                };
                let result = crate::aicommit::probe(&probed).await;
                agents.push(crate::api::AiCommitAgentStatus {
                    agent,
                    available: result.available,
                    detail: result.detail,
                });
            }
            let effort_conflict = options.effort_conflict().map(|effort| {
                format!(
                    "{} does not support {} effort; using the model default",
                    options.agent.as_str(),
                    effort.as_str()
                )
            });
            let status = crate::api::AiCommitAvailability {
                supported: true,
                enabled: options.enabled,
                options,
                agents,
                effort_conflict,
            };
            let _ = events.send((
                id,
                Event::AiCommitAvailability {
                    status: Box::new(status),
                },
            ));
        });
    }

    /// Report that this build cannot generate commit messages at all.
    #[cfg(not(feature = "aicommit"))]
    pub(super) fn emit_ai_commit_availability(&mut self, id: Option<RequestId>) {
        self.emit_unprobed_ai_commit_availability(id);
    }

    /// Emit the configuration without probing the agents.
    ///
    /// Two callers, one meaning — "here is the configuration, but nothing was
    /// launched to verify it": a build without generation support, where there
    /// is nothing to verify, and a session running without a reactor, where
    /// there is no way to. An empty `agents` list is what makes
    /// [`crate::api::AiCommitAvailability::ready`] report not-ready rather than
    /// implying an agent was found.
    pub(super) fn emit_unprobed_ai_commit_availability(&mut self, id: Option<RequestId>) {
        let options = self.config.settings.git.ai_commit.clone();
        let status = crate::api::AiCommitAvailability {
            supported: cfg!(feature = "aicommit"),
            enabled: options.enabled,
            options,
            agents: Vec::new(),
            effort_conflict: None,
        };
        self.emit(
            id,
            Event::AiCommitAvailability {
                status: Box::new(status),
            },
        );
    }

    /// Persist `git.aiCommit.*` to the user settings layer, then re-probe.
    pub(super) fn set_ai_commit_options(
        &mut self,
        id: RequestId,
        options: crate::config::schema::AiCommit,
    ) {
        match crate::config::set_user_ai_commit(&options) {
            Ok(path) => {
                // Apply in memory too: the settings watcher will reload the file,
                // but the client must not see its own change bounce back stale.
                self.config.settings.git.ai_commit = options;
                self.emit(
                    Some(id),
                    Event::Notification {
                        severity: Severity::Information,
                        kind: NotificationKind::Vcs,
                        message: format!("AI commit settings saved to {}", path.display()),
                    },
                );
                self.emit_ai_commit_availability(Some(id));
            },
            Err(error) => self.emit_vcs_notice(id, Severity::Error, error.to_string()),
        }
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

/// Report a generation failure, unless the request was cancelled.
///
/// A cancelled request answers with nothing: the client already moved on, and a
/// late "failed" would contradict the state it is showing.
#[cfg(feature = "aicommit")]
fn fail(
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
    id: RequestId,
    cancel: &crate::cancellation::Cancellation,
    message: String,
) {
    if cancel.is_cancelled() {
        return;
    }
    let _ = events.send((Some(id), Event::CommitMessageFailed { message }));
}
