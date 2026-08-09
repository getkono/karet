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

    /// Ask the backend for the active file's content at `rev`; the answering
    /// [`SessionEvent::FileAtRev`] opens the diff tab (old = content at `rev`,
    /// new = the working text captured now — the live buffer for a code tab, the
    /// file on disk otherwise). `label` names the old side in the tab title.
    pub(super) fn open_changes_with(&mut self, rev: &str, label: &str) {
        let Some((path, live)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        self.pending_open_changes
            .insert((abs.clone(), rev.to_string()), (label.to_string(), live));
        self.send_command(SessionCommand::FileAtRev {
            path: abs,
            rev: rev.to_string(),
        });
    }

    /// Complete a parked Open Changes request with the backend's answer.
    pub(super) fn apply_file_at_rev(
        &mut self,
        path: PathBuf,
        rev: String,
        content: Result<Option<String>, String>,
    ) {
        let Some((label, live)) = self.pending_open_changes.remove(&(path.clone(), rev)) else {
            return; // stale: the request's tab intent is gone
        };
        let old_text = match content {
            Ok(Some(text)) => Some(text),
            Ok(None) => {
                self.status = Some(format!("open changes: file does not exist at {label}"));
                return;
            },
            Err(reason) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {reason}"),
                );
                return;
            },
        };
        let new_text = live.or_else(|| {
            std::fs::read(&path)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
        });
        // Either side non-text marks the change binary (both texts then empty),
        // matching the FileChange::is_binary contract.
        let is_binary = old_text.is_none() || new_text.is_none();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let change = FileChange {
            path,
            old_path: None,
            status: StatusKind::Modified,
            is_binary,
            old: if is_binary {
                String::new()
            } else {
                old_text.unwrap_or_default()
            },
            new: if is_binary {
                String::new()
            } else {
                new_text.unwrap_or_default()
            },
        };
        let file = FileView::new(change, Section::Working, self.syntax);
        self.push_tab(Tab::new(
            format!("{name} ({label} \u{2194} working)"),
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
                column: 0,
            },
        ));
    }

    /// How many commits the With Revision picker lists at most.
    const OPEN_CHANGES_HISTORY_CAP: usize = 200;

    /// Open the diff-target picker over the active file's commit history
    /// (newest first, capped), for "Open Changes: With Revision…".
    ///
    /// NOTE: the two picker flows below still read the repository directly —
    /// short, capped reads behind an explicit picker action. They are the last
    /// direct `karet-vcs` reads in the app and migrate behind the seam with the
    /// diff-preparation move.
    pub(super) fn open_changes_pick_revision(&mut self) {
        let Some((path, _)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let start = abs.parent().unwrap_or(&abs);
        let repo = match karet_vcs::Repository::discover(start) {
            Ok(repo) => repo,
            Err(_) => {
                self.status = Some("open changes: not in a git repository".to_string());
                return;
            },
        };
        let commits = match repo.file_history(&abs, 0, Self::OPEN_CHANGES_HISTORY_CAP) {
            Ok(commits) => commits,
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
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

    /// Open the diff-target picker over the repository's local branches, for
    /// "Open Changes: With Branch…".
    pub(super) fn open_changes_pick_branch(&mut self) {
        let Some((path, _)) = self.active_file_and_text() else {
            self.status = Some("open changes: no file".to_string());
            return;
        };
        let abs = std::path::absolute(&path).unwrap_or_else(|_| path.clone());
        let start = abs.parent().unwrap_or(&abs);
        let repo = match karet_vcs::Repository::discover(start) {
            Ok(repo) => repo,
            Err(_) => {
                self.status = Some("open changes: not in a git repository".to_string());
                return;
            },
        };
        let branches = match repo.branches() {
            Ok(branches) => branches,
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
        if branches.is_empty() {
            self.status = Some("open changes: no branches".to_string());
            return;
        }
        let items = branches
            .into_iter()
            .map(|b| {
                let display = if b.is_head {
                    format!("{} (current)", b.name)
                } else {
                    b.name.clone()
                };
                let target = DiffTarget {
                    rev: b.name.clone(),
                    label: b.name,
                };
                (display, target)
            })
            .collect();
        self.overlay = Some(Overlay::diff_target("Open Changes: With Branch", items));
    }
}
