use super::*;

impl App {
    pub(super) fn toggle_commit_file(&mut self, file: usize) -> bool {
        let Some(TabKind::Commit { view, .. } | TabKind::Compare { view, .. }) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        else {
            return false;
        };
        if !view.toggled_files.remove(&file) {
            view.toggled_files.insert(file);
        }
        if let Some(anchor) = view.file_anchors.get(file) {
            view.scroll = *anchor;
        }
        true
    }

    /// The active tab's commit graph browser, if it is one.
    pub(super) fn active_commit_graph(&mut self) -> Option<&mut TabKind> {
        let tab = self.tabs.get_mut(self.active)?;
        matches!(tab.kind, TabKind::CommitGraph { .. }).then_some(&mut tab.kind)
    }

    /// Move the view's selection by `delta` (clamped).
    pub(super) fn graph_select(&mut self, delta: i32) {
        let Some(TabKind::CommitGraph {
            commits, selected, ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        let Some(last) = commits.len().checked_sub(1) else {
            return;
        };
        let next = (*selected as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
        self.graph_select_to(next);
    }

    /// Select commit `index` outright, scrolling it into view.
    ///
    /// Selection and the viewport are independent here — the wheel and the scrollbar
    /// pan the graph without moving the cursor — so a *selection* move is the one thing
    /// that has to drag the viewport along with it.
    pub(super) fn graph_select_to(&mut self, index: usize) {
        let Some(TabKind::CommitGraph {
            commits,
            selected,
            list_offset,
            list_rect,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        if commits.is_empty() {
            return;
        }
        *selected = index.min(commits.len() - 1);
        let offset = crate::ui::commit::list::keep_visible(
            *selected,
            usize::from(*list_offset),
            usize::from(list_rect.height),
        );
        *list_offset = u16::try_from(offset).unwrap_or(u16::MAX);
        self.graph_prefetch();
    }

    /// Pan the graph vertically without disturbing the selection.
    pub(super) fn graph_scroll_to(&mut self, offset: usize) {
        let Some(TabKind::CommitGraph {
            commits,
            has_more,
            list_offset,
            list_rect,
            ..
        }) = self.active_commit_graph()
        else {
            return;
        };
        // One trailing row exists while more history remains (the "more" affordance),
        // so the last commit can still be scrolled to the top of the viewport.
        let rows = commits.len() + usize::from(*has_more);
        let max = rows.saturating_sub(usize::from(list_rect.height).max(1));
        *list_offset = u16::try_from(offset.min(max)).unwrap_or(u16::MAX);
        self.graph_prefetch();
    }

    /// Keep loaded history well ahead of the viewport.
    ///
    /// The point is that the graph renders "as far as the eye can see": a page is
    /// requested while the viewport is still [`GRAPH_PREFETCH_SCREENS`] screens short of
    /// the end, and each arriving page re-runs this, so fetching chains forward on its
    /// own. The trailing "more" row is then only reachable by outrunning the fetch.
    pub(crate) fn graph_prefetch(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let view = tab.view;
        let TabKind::CommitGraph {
            history_path,
            commits,
            has_more,
            loading,
            list_offset,
            list_rect,
            ..
        } = &tab.kind
        else {
            return;
        };
        if !*has_more || *loading {
            return;
        }
        let height = usize::from(list_rect.height).max(1);
        let margin = height * GRAPH_PREFETCH_SCREENS;
        if usize::from(*list_offset) + height + margin < commits.len() {
            return;
        }
        let loaded = commits.len();
        let command = match history_path.clone() {
            Some(path) => SessionCommand::FileHistory {
                path,
                skip: loaded,
                limit: GRAPH_LOG_PAGE,
            },
            None => SessionCommand::VcsLog {
                skip: loaded,
                limit: GRAPH_LOG_PAGE,
            },
        };
        let Some(id) = self.send(command) else {
            return;
        };
        self.graph_log_reqs.insert(id, view);
        if let Some(TabKind::CommitGraph {
            loading,
            loading_since,
            ..
        }) = self.active_commit_graph()
        {
            *loading = true;
            *loading_since = Some(Pending::start());
        }
    }

    /// Select and open the commit under `point` in the graph view, if the click landed
    /// on a commit row. Reports whether it consumed the click.
    pub(super) fn graph_click(&mut self, point: (u16, u16)) -> bool {
        let Some(TabKind::CommitGraph {
            commits,
            list_offset,
            list_rect,
            ..
        }) = self.active_commit_graph()
        else {
            return false;
        };
        if !rect_contains(*list_rect, point) {
            return false;
        }
        let row = usize::from(*list_offset) + usize::from(point.1 - list_rect.y);
        if row >= commits.len() {
            // The trailing "more" row: paging is automatic, so there is nothing to do.
            return true;
        }
        self.graph_select_to(row);
        self.graph_open_selected();
        true
    }

    /// Open the graph view's selected commit as a standalone commit tab.
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
        self.notify_progress(
            NotificationKind::Vcs,
            Self::DIFF_COMPUTE_TAG.to_string(),
            "computing diff…",
            None,
        );
        self.send_command(command);
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
            self.notify(
                Report::Outcome,
                NotificationKind::Vcs,
                format!("compare base marked: {short} (select another, then compare)"),
            );
        }
    }

    /// Compare the browser's marked base commit against the current selection (a two-dot
    /// `base..selected` diff). Refuses when no base has been marked yet.
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
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "mark a compare base first (Commit Graph: Mark Compare Base)",
            );
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

    /// Apply a fetched history page to the graph view that asked for it: replace on the
    /// first page, append otherwise.
    ///
    /// The page is routed by [`ViewId`], never broadcast — several graph views can be
    /// open at once (a whole-repo log beside a file history), and each owns its own
    /// paging cursor.
    pub(super) fn apply_graph_log(
        &mut self,
        view: ViewId,
        skip: usize,
        commits: Vec<Commit>,
        has_more: bool,
    ) {
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            let TabKind::CommitGraph {
                commits: loaded,
                rails,
                has_more: more,
                loading,
                loading_since,
                selected,
                ..
            } = &mut tab.kind
            else {
                continue;
            };
            *loading = false;
            *loading_since = None;
            *more = has_more;
            if skip == 0 {
                *loaded = commits;
                *selected = 0;
            } else if skip == loaded.len() {
                loaded.extend(commits);
            } else {
                // A page that neither replaces nor continues the loaded run is stale.
                return;
            }
            *rails = crate::ui::commit::list::commit_rails(loaded);
            break;
        }
        // Chain the next page while the viewport is still short of the end.
        self.graph_prefetch();
    }

    /// Build and open a commit tab from a resolved [`CommitDetail`] and its
    /// backend-prepared changes, then fire the lazy GitHub verification fetch to
    /// upgrade the signature badge.
    pub(super) fn open_commit_tab(
        &mut self,
        detail: Box<CommitDetail>,
        changes: Vec<PreparedChange>,
    ) {
        let files = CommitFiles::ready(commit_file_views(changes));
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, files));
        let view = self.tabs[self.active].view;
        self.request_commit_verification(view, hash);
    }

    /// Open a standalone commit tab with metadata visible while changed files are still
    /// loading. Used for unsolicited commit-detail events.
    pub(super) fn open_commit_metadata_tab(&mut self, detail: Box<CommitDetail>) {
        let hash = detail.hash.clone();
        self.push_tab(Tab::commit(detail, CommitFiles::loading()));
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
                    TabKind::CommitLoading { pager, .. } => pager.scroll,
                    TabKind::Commit { view, .. } => view.scroll,
                    _ => 0,
                };
                tab.kind = TabKind::Commit {
                    detail,
                    files: CommitFiles::loading(),
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
        prepared: Vec<PreparedChange>,
    ) {
        let mut files = Some(commit_file_views(prepared));
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
                            ..
                        } if slot.hash == hash => {
                            *slot = detail;
                            let verification = current_files.verification.take();
                            *current_files = CommitFiles {
                                verification,
                                ..CommitFiles::ready(files)
                            };
                        },
                        _ => {
                            tab.kind = TabKind::Commit {
                                detail,
                                files: CommitFiles::ready(files),
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
            self.send(SessionCommand::FetchCommitVerification { hash: hash.clone() })
        {
            self.pending_commit_verification
                .insert(request, (view, hash));
        }
    }

    /// Mark a pending commit-detail request as failed and clear any visible loading
    /// placeholder tied to that request.
    pub(super) fn fail_pending_commit_detail(&mut self, request: RequestId, message: &str) {
        let Some(view) = self.pending_commit_detail.remove(&request) else {
            return;
        };
        for tab in self.all_tabs_mut() {
            if tab.view != view {
                continue;
            }
            match &mut tab.kind {
                TabKind::CommitLoading { error, .. } => {
                    *error = Some(message.to_string());
                },
                TabKind::Commit { files, .. } => {
                    files.loading_since = None;
                    files.error = Some(message.to_string());
                },
                _ => {},
            }
            break;
        }
    }

    /// Build and open a compare tab from a resolved range and its changes. An empty
    /// range (identical endpoints) opens with a "no changes" state rather than nothing.
    pub(super) fn open_compare_tab(
        &mut self,
        base_label: String,
        head_label: String,
        merge_base: bool,
        changes: Vec<PreparedChange>,
    ) {
        // The compare tab is the answer to "computing diff…", so it retires that
        // card whether or not the range turned out to be empty.
        if changes.is_empty() {
            self.notify_tagged(
                Report::Refusal,
                NotificationKind::Vcs,
                format!("no changes between {base_label} and {head_label}"),
                Some(Self::DIFF_COMPUTE_TAG.to_string()),
            );
        } else {
            self.notifications.dismiss_tagged(Self::DIFF_COMPUTE_TAG);
        }
        let files = CommitFiles::ready(commit_file_views(changes));
        self.push_tab(Tab::compare(base_label, head_label, merge_base, files));
    }

    /// Apply the forge's verification verdict to every open commit tab for `hash`.
    pub(super) fn apply_commit_verification(&mut self, hash: &str, status: GithubVerification) {
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Commit { detail, files, .. } if detail.hash == hash => {
                    files.verification = Some(status.clone());
                },
                _ => {},
            }
        }
    }
}

/// Wrap backend-prepared commit/range changes for display.
fn commit_file_views(changes: Vec<PreparedChange>) -> Vec<FileView> {
    changes.into_iter().map(FileView::new).collect()
}

impl App {
    /// The hash of the graph browser's selected commit.
    fn selected_graph_commit(&self) -> Option<String> {
        match &self.tabs.get(self.active)?.kind {
            TabKind::CommitGraph {
                commits, selected, ..
            } => commits.get(*selected).map(|commit| commit.hash.clone()),
            _ => None,
        }
    }

    /// Open the action menu for the selected commit (`.` in the graph).
    pub(super) fn commit_graph_menu(&mut self) {
        let Some(hash) = self.selected_graph_commit() else {
            return;
        };
        let short: String = hash.chars().take(7).collect();
        self.overlay = Some(Overlay::commands(
            format!("Commit {short}"),
            vec![
                Command::CommitGraphTag,
                Command::CommitGraphCherryPick,
                Command::CommitGraphRevert,
                Command::CommitGraphCheckout,
                Command::CommitGraphResetSoft,
                Command::CommitGraphResetMixed,
                Command::CommitGraphResetHard,
                Command::CommitGraphInteractiveRebase,
                Command::CommitGraphCopyIssueUrls,
                Command::CommitGraphMarkBase,
                Command::CommitGraphCompare,
            ],
        ));
    }

    /// Prompt for a tag name on the selected commit.
    pub(super) fn commit_graph_tag(&mut self) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.overlay = Some(Overlay::text(
            format!("Tag name for {short}"),
            crate::overlay::TextPurpose::TagCreate { rev },
        ));
    }

    /// Run one non-destructive graph operation on the selected commit.
    pub(super) fn commit_graph_op(&mut self, verb: &str, action: impl FnOnce(String) -> VcsAction) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.notify_progress(
            NotificationKind::Vcs,
            Self::VCS_OPERATION_TAG.to_string(),
            format!("{verb}: {short}"),
            None,
        );
        self.run_vcs_action(action(rev));
    }

    /// Hard reset discards local changes, so it always asks first.
    pub(super) fn commit_graph_reset_hard(&mut self) {
        let Some(rev) = self.selected_graph_commit() else {
            return;
        };
        let short: String = rev.chars().take(7).collect();
        self.confirm_action(
            format!("Hard-reset to {short}?"),
            "Moves the branch and the worktree to this commit, throwing away every \
             uncommitted change. This cannot be undone.",
            "Cancel",
            format!("Reset --hard to {short}"),
            ConfirmAction::ResetHard(rev),
        );
    }

    /// Open the interactive-rebase plan editor for the commits between the
    /// selection (exclusive, the new base) and `HEAD`, oldest first.
    pub(super) fn commit_graph_interactive_rebase(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::CommitGraph {
            commits, selected, ..
        } = &tab.kind
        else {
            return;
        };
        if *selected == 0 {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "select the commit to rebase onto (below HEAD)",
            );
            return;
        }
        let Some(onto) = commits.get(*selected).map(|commit| commit.hash.clone()) else {
            return;
        };
        // The rows above the selection are the commits being replayed; git
        // applies oldest first, so reverse the newest-first page order.
        let steps: Vec<(String, String, String)> = commits[..*selected]
            .iter()
            .rev()
            .map(|commit| {
                (
                    commit.hash.clone(),
                    commit.short_hash.clone(),
                    commit.summary.clone(),
                )
            })
            .collect();
        self.overlay = Some(Overlay::rebase_todo(onto, steps));
    }

    /// Copy the issue URLs referenced (`#123`) by the selected commit's
    /// summary, resolved through `git.issueUrl` or a GitHub origin.
    pub(super) fn commit_graph_copy_issue_urls(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::CommitGraph {
            commits, selected, ..
        } = &tab.kind
        else {
            return;
        };
        let Some(summary) = commits.get(*selected).map(|commit| commit.summary.clone()) else {
            return;
        };
        let template = self.settings.git.issue_url.clone().or_else(|| {
            let origin = self.scm.repository.as_ref().and_then(|snapshot| {
                snapshot
                    .remotes
                    .iter()
                    .find(|remote| remote.name == "origin")
                    .or_else(|| snapshot.remotes.first())
                    .and_then(|remote| remote.url.clone())
            })?;
            let remote = crate::remote::parse_remote(&origin)?;
            matches!(remote.kind, crate::remote::ForgeKind::GitHub)
                .then(|| format!("https://{}/{}/issues/$1", remote.host, remote.repo_path))
        });
        let Some(template) = template else {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "no issue URL template (set git.issueUrl or use a GitHub origin)",
            );
            return;
        };
        let urls: Vec<String> = issue_refs(&summary)
            .into_iter()
            .map(|number| template.replace("$1", &number))
            .collect();
        if urls.is_empty() {
            self.notify(
                Report::Refusal,
                NotificationKind::Vcs,
                "no #123 issue references in this commit",
            );
            return;
        }
        let count = urls.len();
        let _ = self.clipboard.set(&urls.join("\n"));
        self.notify(
            Report::Outcome,
            NotificationKind::Vcs,
            format!(
                "copied {count} issue URL{}",
                if count == 1 { "" } else { "s" }
            ),
        );
    }

    /// Fetch (and prune) every remote the snapshot knows.
    pub(super) fn scm_fetch(&mut self) {
        let remotes: Vec<String> = self
            .scm
            .repository
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .remotes
                    .iter()
                    .map(|remote| remote.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        if remotes.is_empty() {
            self.notify(Report::Refusal, NotificationKind::Vcs, "fetch: no remotes");
            return;
        }
        let mut sent = false;
        for remote in remotes {
            sent |= self
                .send(SessionCommand::VcsAction {
                    action: VcsAction::Fetch { remote },
                })
                .is_some();
        }
        // Only once at least one fetch is actually out: the card is retired by the
        // answering operation, which a closed backend will never send.
        if sent {
            self.notify_progress(
                NotificationKind::Vcs,
                Self::VCS_OPERATION_TAG.to_string(),
                "fetching…",
                None,
            );
        }
    }
}

/// The `#123` issue numbers referenced in `text`, in order, deduplicated.
fn issue_refs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '#'
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
        {
            let digits: String = chars[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            i += 1 + digits.len();
            if !out.contains(&digits) {
                out.push(digits);
            }
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod issue_ref_tests {
    use super::issue_refs;

    #[test]
    fn refs_parse_with_boundaries_and_dedup() {
        assert_eq!(issue_refs("fix #12 and #345 (refs #12)"), vec!["12", "345"]);
        assert!(issue_refs("sha1#123 and c#").is_empty());
        assert!(issue_refs("no refs").is_empty());
    }
}
