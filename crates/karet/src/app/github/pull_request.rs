//! Pull-request-specific keyboard and mouse interaction.

use super::*;

impl App {
    pub(super) fn github_pull_request_key(&mut self, key: KeyEvent) -> bool {
        let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        else {
            return false;
        };
        if let Some(editor) = view.editor {
            if key.code == KeyCode::Esc {
                if editor == GithubPullRequestEditor::Body {
                    view.body_edit = None;
                }
                view.editor = None;
                view.preview = false;
                return true;
            }
            if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
                view.preview = !view.preview;
                return true;
            }
            if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.submit_pull_request_editor(editor);
                return true;
            }
            if view.preview {
                return true;
            }
            let target = match editor {
                GithubPullRequestEditor::Body => view.body_edit.as_mut(),
                GithubPullRequestEditor::Comment => Some(&mut view.comment_edit),
            };
            if let Some(target) = target {
                match key.code {
                    KeyCode::Backspace => {
                        target.pop();
                    },
                    KeyCode::Enter => target.push('\n'),
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        target.push(character);
                    },
                    _ => {},
                }
            }
            return true;
        }

        match key.code {
            KeyCode::Char('1') => {
                self.set_pull_request_section(GithubPullRequestSection::Conversation)
            },
            KeyCode::Char('2') => self.set_pull_request_section(GithubPullRequestSection::Commits),
            KeyCode::Char('3') => {
                self.set_pull_request_section(GithubPullRequestSection::FilesChanged)
            },
            KeyCode::Down | KeyCode::Char('j')
                if view.section == GithubPullRequestSection::Commits =>
            {
                view.commit_cursor =
                    (view.commit_cursor + 1).min(view.commits.len().saturating_sub(1));
            },
            KeyCode::Up | KeyCode::Char('k')
                if view.section == GithubPullRequestSection::Commits =>
            {
                view.commit_cursor = view.commit_cursor.saturating_sub(1);
            },
            KeyCode::Enter if view.section == GithubPullRequestSection::Commits => {
                if let Some(commit) = view.commits.get(view.commit_cursor) {
                    let hash = commit.sha.clone();
                    self.open_commit(hash);
                }
            },
            KeyCode::Char('c') if view.section == GithubPullRequestSection::Conversation => {
                view.editor = Some(GithubPullRequestEditor::Comment);
                view.preview = false;
            },
            KeyCode::Char('m') if view.section == GithubPullRequestSection::Conversation => {
                self.merge_pull_request();
            },
            KeyCode::Char('d') if view.section == GithubPullRequestSection::Conversation => {
                self.toggle_pull_request_draft();
            },
            _ => return false,
        }
        true
    }

    fn set_pull_request_section(&mut self, section: GithubPullRequestSection) {
        let range = self
            .tabs
            .get_mut(self.active)
            .and_then(|tab| match &mut tab.kind {
                TabKind::Github(GithubViewState::PullRequest(view)) => {
                    view.section = section;
                    view.scroll = 0;
                    (section == GithubPullRequestSection::FilesChanged).then(|| {
                        (
                            view.pull_request.base_sha.clone(),
                            view.pull_request.head_sha.clone(),
                        )
                    })
                },
                _ => None,
            });
        if let Some((base, head)) = range {
            if base.is_empty() || head.is_empty() {
                self.status = Some("pull request revisions are still loading".to_string());
                return;
            }
            self.open_range(SessionCommand::RangeChanges {
                spec: karet_session::RangeSpec::Between {
                    base,
                    head,
                    merge_base: true,
                },
            });
        }
    }

    fn submit_pull_request_editor(&mut self, editor: GithubPullRequestEditor) {
        let submission = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Github(GithubViewState::PullRequest(view)) => {
                let body = match editor {
                    GithubPullRequestEditor::Body => view.body_edit.clone().unwrap_or_default(),
                    GithubPullRequestEditor::Comment => view.comment_edit.clone(),
                };
                Some((view.pull_request.number, body))
            },
            _ => None,
        });
        let Some((number, body)) = submission else {
            return;
        };
        if editor == GithubPullRequestEditor::Comment && body.trim().is_empty() {
            self.status = Some("comment cannot be empty".to_string());
            return;
        }
        let command = match editor {
            GithubPullRequestEditor::Body => {
                SessionCommand::GithubUpdatePullRequestBody { number, body }
            },
            GithubPullRequestEditor::Comment => {
                SessionCommand::GithubCommentPullRequest { number, body }
            },
        };
        let request = self.send_command_id(command);
        if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            view.pending = request;
            view.loading_since = Instant::now();
            view.error = None;
        }
    }

    fn merge_pull_request(&mut self) {
        let values = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Github(GithubViewState::PullRequest(view))
                if view.can_write
                    && view.pending.is_none()
                    && !view.pull_request.draft
                    && !view.pull_request.merged
                    && view.pull_request.mergeable != Some(false) =>
            {
                Some((view.pull_request.number, view.pull_request.head_sha.clone()))
            },
            _ => None,
        });
        let Some((number, head_sha)) = values else {
            self.status = Some("this pull request is not ready to merge".to_string());
            return;
        };
        let request =
            self.send_command_id(SessionCommand::GithubMergePullRequest { number, head_sha });
        self.set_pull_request_pending(request);
    }

    fn toggle_pull_request_draft(&mut self) {
        let values = self.tabs.get(self.active).and_then(|tab| match &tab.kind {
            TabKind::Github(GithubViewState::PullRequest(view))
                if view.can_write
                    && view.pending.is_none()
                    && !view.pull_request.merged
                    && !view.pull_request.node_id.is_empty() =>
            {
                Some((
                    view.pull_request.node_id.clone(),
                    view.pull_request.number,
                    !view.pull_request.draft,
                ))
            },
            _ => None,
        });
        let Some((node_id, number, draft)) = values else {
            self.status = Some("pull request readiness cannot be changed".to_string());
            return;
        };
        let request = self.send_command_id(SessionCommand::GithubSetPullRequestDraft {
            node_id,
            number,
            draft,
        });
        self.set_pull_request_pending(request);
    }

    fn set_pull_request_pending(&mut self, request: Option<RequestId>) {
        if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
            self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
            view.pending = request;
            view.loading_since = Instant::now();
            view.error = None;
        }
    }

    pub(super) fn github_pull_request_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row);
        enum Hit {
            Section(GithubPullRequestSection),
            Body,
            Comment,
            Merge,
            Draft,
            Check(String),
            Commit(usize),
            Content,
            None,
        }
        let hit =
            self.tabs
                .get(self.active)
                .map_or(Hit::None, |tab| match &tab.kind {
                    TabKind::Github(GithubViewState::PullRequest(view)) => {
                        if let Some(section) =
                            view.section_hits.iter().find_map(|(section, rect)| {
                                rect_contains(*rect, point).then_some(*section)
                            })
                        {
                            Hit::Section(section)
                        } else if rect_contains(view.body_rect, point) {
                            Hit::Body
                        } else if rect_contains(view.comment_rect, point) {
                            Hit::Comment
                        } else if rect_contains(view.merge_rect, point) {
                            Hit::Merge
                        } else if rect_contains(view.draft_rect, point) {
                            Hit::Draft
                        } else if let Some(url) = view.check_hits.iter().find_map(|(url, rect)| {
                            rect_contains(*rect, point).then(|| url.clone())
                        }) {
                            Hit::Check(url)
                        } else if rect_contains(view.commits_rect, point) {
                            Hit::Commit(
                                usize::from(view.commit_offset)
                                    + usize::from(mouse.row.saturating_sub(view.commits_rect.y)),
                            )
                        } else {
                            Hit::Content
                        }
                    },
                    _ => Hit::None,
                });
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                {
                    if view.section == GithubPullRequestSection::Commits {
                        view.commit_cursor =
                            (view.commit_cursor + 3).min(view.commits.len().saturating_sub(1));
                    } else {
                        view.scroll = view.scroll.saturating_add(3);
                    }
                }
                return true;
            },
            MouseEventKind::ScrollUp => {
                if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                {
                    if view.section == GithubPullRequestSection::Commits {
                        view.commit_cursor = view.commit_cursor.saturating_sub(3);
                    } else {
                        view.scroll = view.scroll.saturating_sub(3);
                    }
                }
                return true;
            },
            MouseEventKind::Down(MouseButton::Left) => {},
            _ => return !matches!(hit, Hit::None),
        }
        match hit {
            Hit::Section(section) => self.set_pull_request_section(section),
            Hit::Body => {
                if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                    && view.can_write
                {
                    if view.body_edit.is_none() {
                        view.body_edit = Some(view.pull_request.body.clone().unwrap_or_default());
                    }
                    view.editor = Some(GithubPullRequestEditor::Body);
                    view.preview = false;
                }
            },
            Hit::Comment => {
                if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                    && view.can_write
                {
                    view.editor = Some(GithubPullRequestEditor::Comment);
                    view.preview = false;
                }
            },
            Hit::Merge => self.merge_pull_request(),
            Hit::Draft => self.toggle_pull_request_draft(),
            Hit::Check(url) => self.open_web_link(&url),
            Hit::Commit(index) => {
                let open = self.click_streak(mouse.column, mouse.row) >= 2;
                let hash = if let Some(TabKind::Github(GithubViewState::PullRequest(view))) =
                    self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
                    && index < view.commits.len()
                {
                    view.commit_cursor = index;
                    open.then(|| view.commits[index].sha.clone())
                } else {
                    None
                };
                if let Some(hash) = hash {
                    self.open_commit(hash);
                }
            },
            Hit::Content | Hit::None => {},
        }
        true
    }

    fn open_web_link(&mut self, url: &str) {
        let result = if cfg!(target_os = "windows") {
            std::process::Command::new("cmd")
                .args(["/C", "start", "", url])
                .spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(url).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(url).spawn()
        };
        if let Err(error) = result {
            self.copy_to_clipboard(url.to_string(), "check link");
            self.status = Some(format!(
                "could not open the check link ({error}); copied it instead"
            ));
        }
    }
}
