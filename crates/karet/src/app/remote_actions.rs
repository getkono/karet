use super::*;

impl App {
    /// Copy `text` to the clipboard, reporting the outcome in the status bar.
    pub(super) fn copy_to_clipboard(&mut self, text: String, what: &str) {
        self.status = Some(match self.clipboard.set(&text) {
            Ok(()) => format!("copied {what}"),
            Err(e) => format!("copy failed: {e}"),
        });
    }

    /// Copy the active code tab's selection, or its cursor line when nothing is
    /// selected (VS Code behavior).
    pub(super) fn copy_selection(&mut self) {
        if matches!(
            self.input_context().modal,
            Some(Modal::SearchInput | Modal::CommitInput)
        ) {
            if let Some(text) = self.modal_selection_text() {
                self.copy_to_clipboard(text, "selection");
            } else {
                self.status = Some("copy: no text selected".to_string());
            }
            return;
        }
        if self.focus_target() == FocusTarget::Explorer {
            self.explorer_copy_files();
            return;
        }
        let text = match self.tabs.get(self.active) {
            Some(Tab {
                kind: TabKind::Code { buffer, text, .. },
                editor,
                ..
            }) => editor.selection_range().map_or_else(
                || {
                    buffer
                        .line(editor.cursor().line as usize)
                        .map(|l| format!("{l}\n"))
                },
                |range| selection_text(buffer, text, range),
            ),
            _ => None,
        };
        match text {
            Some(text) => self.copy_to_clipboard(text, "selection"),
            None => self.status = Some("copy: open a text file".to_string()),
        }
    }

    /// Copy the active file's path (absolute or workspace-relative) to the clipboard.
    pub(super) fn copy_path(&mut self, relative: bool) {
        let Some(path) = self.tabs.get(self.active).and_then(Tab::path) else {
            self.status = Some("copy path: no file".to_string());
            return;
        };
        let path = if relative {
            path.strip_prefix(&self.root).unwrap_or(path)
        } else {
            path
        };
        let text = path.to_string_lossy().into_owned();
        self.copy_to_clipboard(text, "path");
    }

    /// Reveal the active tab's file in the explorer.
    pub(super) fn reveal_active_in_explorer(&mut self) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("reveal: no file".to_string());
            return;
        };
        self.reveal_in_explorer(&path);
    }

    /// The cached repository/remote facts for `path`. A miss fires one backend
    /// request (the reads run on the VCS worker, never this thread) and returns
    /// `None` — menus show a resolving note and parked actions complete when
    /// [`SessionEvent::RemoteFacts`] answers.
    pub(super) fn cached_remote_facts(
        &mut self,
        path: &Path,
    ) -> Option<&Result<RemoteFacts, String>> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.remote_facts.contains_key(&abs) {
            self.request_remote_facts(&abs);
            return None;
        }
        self.remote_facts.get(&abs)
    }

    /// Ask the backend for `abs`'s repository facts unless a request is already
    /// in flight.
    pub(super) fn request_remote_facts(&mut self, abs: &Path) {
        if self.remote_facts_pending.contains(abs) {
            return;
        }
        self.remote_facts_pending.insert(abs.to_path_buf());
        self.send_command(SessionCommand::RemoteFacts {
            path: abs.to_path_buf(),
        });
    }

    /// Adopt a backend facts answer: cache it (parsing the origin URL into the
    /// presentation-side remote model) and complete any parked actions for the
    /// path.
    pub(super) fn apply_remote_facts(
        &mut self,
        path: PathBuf,
        facts: Result<karet_session::RemoteFacts, String>,
    ) {
        self.remote_facts_pending.remove(&path);
        let adopted = facts.and_then(|facts| {
            let remote = remote::parse_remote(&facts.origin_url)
                .ok_or_else(|| format!("unrecognized origin remote URL: {}", facts.origin_url))?;
            Ok(RemoteFacts {
                remote,
                head: facts.head,
                branch: facts.branch,
                rel_path: facts.rel_path,
                tracked: facts.tracked,
            })
        });
        self.remote_facts.insert(path.clone(), adopted);
        let parked: Vec<PendingRemoteAction> = {
            let (ready, rest) = std::mem::take(&mut self.pending_remote_actions)
                .into_iter()
                .partition(|action| match action {
                    PendingRemoteAction::CopyLink { path: p, .. } => *p == path,
                });
            self.pending_remote_actions = rest;
            ready
        };
        for action in parked {
            match action {
                PendingRemoteAction::CopyLink { kind, path, line } => {
                    self.copy_remote_link_for(kind, &path, line);
                },
            }
        }
    }

    /// Copy the `kind` web link for the active file, or surface why it cannot be
    /// built (mirroring the pane menu's disabled notes exactly — both sides run
    /// the same [`remote::link`]).
    pub(super) fn copy_remote_link(&mut self, kind: remote::LinkKind) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            self.status = Some("copy link: no file".to_string());
            return;
        };
        // The caret line only anchors a permalink over a code tab (1-based).
        let line = match (kind, self.tabs.get(self.active)) {
            (remote::LinkKind::GithubPermalink, Some(tab))
                if matches!(tab.kind, TabKind::Code { .. }) =>
            {
                Some(tab.editor.cursor().line.saturating_add(1))
            },
            _ => None,
        };
        self.copy_remote_link_for(kind, &path, line);
    }

    /// Build and copy `kind` for `path` from cached facts, or park the action on
    /// the facts request so one keystroke still completes it when the backend
    /// answers.
    pub(super) fn copy_remote_link_for(
        &mut self,
        kind: remote::LinkKind,
        path: &Path,
        line: Option<u32>,
    ) {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let Some(facts) = self.cached_remote_facts(&abs) else {
            self.pending_remote_actions
                .push(PendingRemoteAction::CopyLink {
                    kind,
                    path: abs,
                    line,
                });
            self.status = Some("resolving repository remote…".to_string());
            return;
        };
        let facts = match facts {
            Ok(facts) => facts,
            Err(reason) => {
                self.status = Some(reason.clone());
                return;
            },
        };
        match remote::link(&facts.link_target(), kind, line) {
            Ok(url) => {
                let what = match kind {
                    remote::LinkKind::RemoteFile => "remote file URL",
                    remote::LinkKind::GithubPermalink => "GitHub permalink",
                    remote::LinkKind::GithubHeadLink => "GitHub head link",
                };
                self.copy_to_clipboard(url, what);
            },
            Err(reason) => self.status = Some(reason.clone()),
        }
    }

    /// The active tab's file path and, for a code tab, its live buffer text.
    pub(super) fn active_file_and_text(&self) -> Option<(PathBuf, Option<String>)> {
        let tab = self.tabs.get(self.active)?;
        let path = tab.path()?.to_path_buf();
        let live = match &tab.kind {
            TabKind::Code { text, .. } => Some(text.clone()),
            _ => None,
        };
        Some((path, live))
    }

    /// Why the Open Changes actions do not apply to `path` — outside a repository,
    /// untracked at `HEAD` (which also covers an unborn branch), or the facts are
    /// still resolving — or `None` when they do. Doubles as the pane menu's
    /// disabled note, read from the same per-path facts cache as the link rows.
    pub(super) fn open_changes_note(&mut self, path: &Path) -> Option<String> {
        match self.cached_remote_facts(path)? {
            Err(reason) if reason == "no origin remote configured" => None,
            Err(reason) => Some(reason.clone()),
            Ok(facts) if !facts.tracked => Some("file is not tracked at HEAD".to_string()),
            Ok(_) => None,
        }
    }

    /// Reserve a diff tab and ask the backend to diff the active file at `rev`
    /// against its current content (the live buffer for a code tab, the file on
    /// disk otherwise); the answering [`SessionEvent::DiffPrepared`] fills the
    /// tab. `label` names the old side in the tab title.
    pub(super) fn open_changes_with(&mut self, rev: &str, label: &str) {
        let Some((path, live)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        self.push_tab(Tab::diff(
            format!("{name} ({label} \u{2194} working)"),
            abs.clone(),
            Section::Working,
            None,
            self.diff_layout,
        ));
        let view = self.tabs[self.active].view;
        match self.send_command_id(SessionCommand::DiffWithRev {
            path: abs,
            rev: rev.to_string(),
            live,
        }) {
            Some(request) => {
                self.pending_prepared_diffs.insert(request, view);
            },
            None => self.fail_diff_tab(view, "diff backend is unavailable"),
        }
    }

    /// How many commits the With Revision picker lists at most.
    const OPEN_CHANGES_HISTORY_CAP: usize = 200;

    /// Ask the backend for the active file's commit history (newest first,
    /// capped); the answering [`SessionEvent::FileHistory`] opens the
    /// diff-target picker, for "Open Changes: With Revision…".
    pub(super) fn open_changes_pick_revision(&mut self) {
        let Some((path, _)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        self.pending_history_picker = self.send_command_id(SessionCommand::FileHistory {
            path: abs,
            skip: 0,
            limit: Self::OPEN_CHANGES_HISTORY_CAP,
        });
        if self.pending_history_picker.is_none() {
            self.status = Some("open changes: backend is unavailable".to_string());
        }
    }

    /// Open the With Revision diff-target picker from the backend's file history.
    pub(super) fn apply_history_picker(&mut self, commits: Vec<karet_vcs::Commit>) {
        if commits.is_empty() {
            self.status = Some("open changes: no commits touch this file".to_string());
            return;
        }
        let items = commits
            .into_iter()
            .map(|c| {
                let display = format!(
                    "{} {} \u{2014} {}",
                    c.short_hash,
                    c.summary,
                    ui::relative_time(c.time)
                );
                let target = DiffTarget {
                    rev: c.hash,
                    label: c.short_hash,
                };
                (display, target)
            })
            .collect();
        self.overlay = Some(Overlay::diff_target("Open Changes: With Revision", items));
    }

    /// Open the diff-target picker over the workspace repository's local
    /// branches (from the latest [`RepositorySnapshot`]), for
    /// "Open Changes: With Branch…".
    pub(super) fn open_changes_pick_branch(&mut self) {
        let Some(snapshot) = self.scm.repository.as_ref() else {
            // Fetch the snapshot so a retry (or the SCM panel) has it.
            self.request_repository_snapshot();
            self.status = Some("open changes: loading branches, try again".to_string());
            return;
        };
        if snapshot.branches.is_empty() {
            self.status = Some("open changes: no branches".to_string());
            return;
        }
        let items = snapshot
            .branches
            .iter()
            .map(|b| {
                let display = if b.is_head {
                    format!("{} (current)", b.name)
                } else {
                    b.name.clone()
                };
                let target = DiffTarget {
                    rev: b.name.clone(),
                    label: b.name.clone(),
                };
                (display, target)
            })
            .collect();
        self.overlay = Some(Overlay::diff_target("Open Changes: With Branch", items));
    }
}
