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

    /// Gather the repository/remote facts for `path`, synchronously (fast local
    /// reads on a short-lived repository handle, like blame). The `Err` side is a
    /// user-facing reason, doubling as a context-menu disabled note.
    pub(super) fn remote_facts(&self, path: &Path) -> Result<RemoteFacts, String> {
        // Absolutize first so discovery starts from the file's own directory (a
        // file may live in a different repository than the workspace root).
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let start = abs.parent().unwrap_or(&abs);
        let repo = karet_vcs::Repository::discover(start)
            .map_err(|_| "not in a git repository".to_string())?;
        let origin = repo
            .origin_url()
            .ok_or_else(|| "no origin remote configured".to_string())?;
        let remote = remote::parse_remote(&origin)
            .ok_or_else(|| format!("unrecognized origin remote URL: {origin}"))?;
        let rel_path = repo
            .path_in_worktree(&abs)
            .ok_or_else(|| "file is outside the repository worktree".to_string())?;
        // An unborn branch has no HEAD hash; file_at_rev then errors, reading as
        // untracked — both surface as accurate notes further down.
        let head = repo.head_hash().ok().flatten();
        let branch = repo.current_branch().ok().flatten();
        let tracked = repo.file_at_rev(&abs, "HEAD").ok().flatten().is_some();
        Ok(RemoteFacts {
            remote,
            head,
            branch,
            rel_path,
            tracked,
        })
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
        let facts = match self.remote_facts(&path) {
            Ok(facts) => facts,
            Err(reason) => {
                self.status = Some(reason);
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
            Err(reason) => self.status = Some(reason),
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
    /// or untracked at `HEAD` (which also covers an unborn branch) — or `None` when
    /// they do. Doubles as the pane menu's disabled note.
    pub(super) fn open_changes_note(&self, path: &Path) -> Option<String> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let start = abs.parent().unwrap_or(&abs);
        let Ok(repo) = karet_vcs::Repository::discover(start) else {
            return Some("not in a git repository".to_string());
        };
        if repo.file_at_rev(&abs, "HEAD").ok().flatten().is_none() {
            return Some("file is not tracked at HEAD".to_string());
        }
        None
    }

    /// Open a diff tab for the active file: old = its content at `rev`, new = the
    /// working text (the live buffer for a code tab, the file on disk otherwise).
    /// `label` names the old side in the tab title: `name (label ↔ working)`.
    pub(super) fn open_changes_with(&mut self, rev: &str, label: &str) {
        let Some((path, live)) = self.active_file_and_text() else {
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
        let old_bytes = match repo.file_at_rev(&abs, rev) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                self.status = Some(format!("open changes: file does not exist at {label}"));
                return;
            },
            Err(e) => {
                self.notify(
                    Severity::Error,
                    NotificationKind::Vcs,
                    format!("open changes: {e}"),
                );
                return;
            },
        };
        let new_text = live.or_else(|| {
            std::fs::read(&abs)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
        });
        let old_text = String::from_utf8(old_bytes).ok();
        // Either side non-text marks the change binary (both texts then empty),
        // matching the FileChange::is_binary contract.
        let is_binary = old_text.is_none() || new_text.is_none();
        let name = abs
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let change = FileChange {
            path: abs,
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
            },
        ));
    }

    /// How many commits the With Revision picker lists at most.
    const OPEN_CHANGES_HISTORY_CAP: usize = 200;

    /// Open the diff-target picker over the active file's commit history
    /// (newest first, capped), for "Open Changes: With Revision…".
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
