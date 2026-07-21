//! Application of asynchronous GitHub backend results.

use super::*;

fn human_github_error(operation: &str, message: &str) -> String {
    let action = match operation {
        "actions" => "Could not load GitHub Actions",
        "authentication" | "sign in" => "Could not sign in to GitHub",
        "create issue" => "Could not create the issue",
        "create pull request" => "Could not create the pull request",
        "issue detail" => "Could not load the issue",
        "issue metadata" => "Could not load issue options",
        "issues" => "Could not load issues",
        "pull request detail" => "Could not load the pull request",
        "update pull request body" => "Could not update the pull request description",
        "comment on pull request" => "Could not add the pull request comment",
        "merge pull request" => "Could not merge the pull request",
        "update pull request readiness" => "Could not update the pull request readiness",
        "pull requests" => "Could not load pull requests",
        "refresh" => "Could not refresh GitHub",
        _ => "GitHub request failed",
    };
    format!("{action}. {message}")
}

impl App {
    pub(crate) fn apply_github_issues(
        &mut self,
        id: Option<RequestId>,
        page: GithubPage<GithubIssue>,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::Github(GithubViewState::Dashboard(dashboard)) = &mut tab.kind
                && (dashboard.pending == id || id.is_none())
            {
                dashboard.issues = page.clone();
                dashboard.loading_since = None;
                dashboard.pending = None;
                dashboard.cursor = dashboard
                    .cursor
                    .min(dashboard.row_count().saturating_sub(1));
            }
        }
    }

    pub(crate) fn apply_github_pull_requests(
        &mut self,
        id: Option<RequestId>,
        page: GithubPage<GithubPullRequest>,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::Github(GithubViewState::Dashboard(dashboard)) = &mut tab.kind
                && (dashboard.pending == id || id.is_none())
            {
                dashboard.pull_requests = page.clone();
                dashboard.loading_since = None;
                dashboard.pending = None;
                dashboard.cursor = dashboard
                    .cursor
                    .min(dashboard.row_count().saturating_sub(1));
            }
        }
    }

    pub(crate) fn apply_github_actions(
        &mut self,
        id: Option<RequestId>,
        workflows: GithubPage<GithubWorkflow>,
        runs: GithubPage<GithubWorkflowRun>,
    ) {
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Github(GithubViewState::Dashboard(dashboard))
                    if dashboard.pending == id || id.is_none() =>
                {
                    dashboard.workflows = workflows.clone();
                    dashboard.runs = runs.clone();
                    dashboard.loading_since = None;
                    dashboard.pending = None;
                    dashboard.cursor = dashboard
                        .cursor
                        .min(dashboard.row_count().saturating_sub(1));
                },
                TabKind::Github(GithubViewState::WorkflowRun { workflow, run, .. }) => {
                    if let Some(updated) = runs.items.iter().find(|updated| updated.id == run.id) {
                        *run = updated.clone();
                    }
                    *workflow = workflows
                        .items
                        .iter()
                        .find(|updated| updated.id == run.workflow_id)
                        .cloned();
                },
                _ => {},
            }
        }
    }

    pub(crate) fn apply_github_issue(
        &mut self,
        id: Option<RequestId>,
        issue: GithubIssue,
        comments: GithubPage<GithubComment>,
    ) {
        for tab in self.all_tabs_mut() {
            let created = matches!(
                &tab.kind,
                TabKind::Github(GithubViewState::NewIssue { form, .. }) if form.submitting == id
            );
            if created {
                let repository = match &tab.kind {
                    TabKind::Github(GithubViewState::NewIssue { repository, .. }) => {
                        repository.clone()
                    },
                    _ => continue,
                };
                tab.title = format!("Issue #{}", issue.number);
                tab.kind = TabKind::Github(GithubViewState::Issue {
                    repository,
                    number: issue.number,
                    issue: Some(issue.clone()),
                    comments: comments.clone(),
                    pending: None,
                    loading_since: Instant::now(),
                    error: None,
                    scroll: 0,
                });
            } else if let TabKind::Github(GithubViewState::Issue {
                pending,
                issue: loaded,
                comments: loaded_comments,
                error,
                ..
            }) = &mut tab.kind
                && (*pending == id || id.is_none())
            {
                *loaded = Some(issue.clone());
                *loaded_comments = comments.clone();
                *pending = None;
                *error = None;
            }
        }
    }

    pub(crate) fn apply_github_pull_request(
        &mut self,
        id: Option<RequestId>,
        pull_request: GithubPullRequest,
        comments: GithubPage<GithubComment>,
        supplement: GithubPullRequestSupplement,
    ) {
        let GithubPullRequestSupplement {
            commits,
            checks,
            activity,
            activity_error,
        } = supplement;
        for tab in self.all_tabs_mut() {
            let created_repository = match &tab.kind {
                TabKind::Github(GithubViewState::NewPullRequest { repository, form })
                    if form.submitting == id =>
                {
                    Some(repository.clone())
                },
                _ => None,
            };
            if let Some(repository) = created_repository {
                tab.title = format!("Pull Request #{}", pull_request.number);
                tab.kind = TabKind::Github(GithubViewState::PullRequest(GithubPullRequestView {
                    repository,
                    pull_request: pull_request.clone(),
                    comments: comments.clone(),
                    commits: commits.clone(),
                    checks: checks.clone(),
                    activity: activity.clone(),
                    activity_error: activity_error.clone(),
                    can_write: true,
                    section: GithubPullRequestSection::Conversation,
                    pending: None,
                    loading_since: Instant::now(),
                    error: None,
                    scroll: 0,
                    commit_cursor: 0,
                    commit_offset: 0,
                    body_edit: None,
                    comment_edit: String::new(),
                    editor: None,
                    preview: false,
                    section_hits: Vec::new(),
                    body_rect: Rect::default(),
                    comment_rect: Rect::default(),
                    merge_rect: Rect::default(),
                    draft_rect: Rect::default(),
                    check_hits: Vec::new(),
                    commits_rect: Rect::default(),
                }));
            } else if let TabKind::Github(GithubViewState::PullRequest(view)) = &mut tab.kind
                && (view.pending == id || id.is_none())
            {
                view.pull_request = pull_request.clone();
                view.comments = comments.clone();
                view.commits = commits.clone();
                view.checks = checks.clone();
                view.activity = activity.clone();
                view.activity_error = activity_error.clone();
                view.pending = None;
                view.error = None;
                view.body_edit = None;
                view.comment_edit.clear();
                view.editor = None;
                view.preview = false;
                view.commit_cursor = view.commit_cursor.min(view.commits.len().saturating_sub(1));
            }
        }
    }

    pub(crate) fn apply_github_issue_metadata(
        &mut self,
        id: Option<RequestId>,
        assignees: Vec<String>,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::Github(GithubViewState::NewIssue { form, .. }) = &mut tab.kind
                && form.metadata_pending == id
            {
                form.assignee_options = assignees.clone();
                form.assignee_cursor = 0;
                form.metadata_pending = None;
                form.error = None;
            }
        }
    }

    pub(crate) fn apply_github_error(
        &mut self,
        id: Option<RequestId>,
        operation: String,
        message: String,
    ) {
        let full = human_github_error(&operation, &message);
        let mut applied = false;
        for tab in self.all_tabs_mut() {
            match &mut tab.kind {
                TabKind::Github(GithubViewState::Dashboard(dashboard))
                    if dashboard.login_pending == id =>
                {
                    dashboard.error = Some(full.clone());
                    dashboard.login_pending = None;
                    dashboard.login_token.clear();
                    applied = true;
                },
                TabKind::Github(GithubViewState::Dashboard(dashboard))
                    if dashboard.pending == id =>
                {
                    dashboard.error = Some(full.clone());
                    dashboard.pending = None;
                    dashboard.loading_since = None;
                    applied = true;
                },
                TabKind::Github(GithubViewState::Issue { pending, error, .. })
                    if *pending == id =>
                {
                    *error = Some(full.clone());
                    *pending = None;
                    applied = true;
                },
                TabKind::Github(GithubViewState::PullRequest(view)) if view.pending == id => {
                    view.error = Some(full.clone());
                    view.pending = None;
                    applied = true;
                },
                TabKind::Github(GithubViewState::NewIssue { form, .. })
                    if form.submitting == id || form.metadata_pending == id =>
                {
                    form.error = Some(full.clone());
                    if form.submitting == id {
                        form.submitting = None;
                    }
                    if form.metadata_pending == id {
                        form.metadata_pending = None;
                    }
                    applied = true;
                },
                TabKind::Github(GithubViewState::NewPullRequest { form, .. })
                    if form.submitting == id =>
                {
                    form.error = Some(full.clone());
                    form.submitting = None;
                    applied = true;
                },
                _ => {},
            }
        }
        if !applied {
            self.notify(Severity::Error, NotificationKind::System, full);
        }
    }
}
