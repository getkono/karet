//! Application of asynchronous GitHub backend results.

use super::*;

/// The `operation` label the session gives a commit-signature lookup.
const VERIFICATION_OPERATION: &str = "commit verification";

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
        for open in self.github.pages_mut() {
            if let GithubViewState::Dashboard(dashboard) = open
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
        for open in self.github.pages_mut() {
            if let GithubViewState::Dashboard(dashboard) = open
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
        for open in self.github.pages_mut() {
            match open {
                GithubViewState::Dashboard(dashboard)
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
                GithubViewState::WorkflowRun { workflow, run, .. } => {
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
        for open in self.github.pages_mut() {
            let created = matches!(
                &open,
                GithubViewState::NewIssue { form, .. } if form.submitting == id
            );
            if created {
                let state = GithubViewState::Issue {
                    number: issue.number,
                    issue: Some(issue.clone()),
                    comments: comments.clone(),
                    pending: None,
                    loading_since: Pending::start(),
                    error: None,
                    scroll: 0,
                };
                *open = state;
            } else if let GithubViewState::Issue {
                pending,
                issue: loaded,
                comments: loaded_comments,
                error,
                ..
            } = open
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
        for open in self.github.pages_mut() {
            let created = matches!(
                &open,
                GithubViewState::NewPullRequest { form, .. } if form.submitting == id
            );
            if created {
                let state = GithubViewState::PullRequest(GithubPullRequestView {
                    pull_request: pull_request.clone(),
                    comments: comments.clone(),
                    commits: commits.clone(),
                    checks: checks.clone(),
                    activity: activity.clone(),
                    activity_error: activity_error.clone(),
                    can_write: true,
                    section: GithubPullRequestSection::Conversation,
                    pending: None,
                    loading_since: Pending::start(),
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
                });
                *open = state;
            } else if let GithubViewState::PullRequest(view) = open
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
        for open in self.github.pages_mut() {
            if let GithubViewState::NewIssue { form, .. } = open
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
        for open in self.github.pages_mut() {
            match open {
                GithubViewState::Dashboard(dashboard) if dashboard.login_pending == id => {
                    dashboard.error = Some(full.clone());
                    dashboard.login_pending = None;
                    dashboard.login_token.clear();
                    applied = true;
                },
                GithubViewState::Dashboard(dashboard) if dashboard.pending == id => {
                    dashboard.error = Some(full.clone());
                    dashboard.pending = None;
                    dashboard.loading_since = None;
                    applied = true;
                },
                GithubViewState::Issue { pending, error, .. } if *pending == id => {
                    *error = Some(full.clone());
                    *pending = None;
                    applied = true;
                },
                GithubViewState::PullRequest(view) if view.pending == id => {
                    view.error = Some(full.clone());
                    view.pending = None;
                    applied = true;
                },
                GithubViewState::NewIssue { form, .. }
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
                GithubViewState::NewPullRequest { form, .. } if form.submitting == id => {
                    form.error = Some(full.clone());
                    form.submitting = None;
                    applied = true;
                },
                _ => {},
            }
        }
        if !applied {
            // A page can be closed while its request is still in flight, and `Esc`
            // makes that a reflex rather than a decision. The reply is genuinely
            // nobody's now, so it must not surface as an error about something the
            // user has already backed out of.
            if self.github.was_abandoned(id) {
                return;
            }
            // The commit-signature lookup is a speculative enrichment fired for
            // whatever commit is on screen. A commit the forge does not know —
            // unpushed, on a fork, or in a repository with no GitHub remote —
            // is an ordinary outcome, and the detail pane reads fine without a
            // verdict, so it must not raise an error the user cannot act on.
            if operation == VERIFICATION_OPERATION {
                return;
            }
            self.notify(Severity::Error, NotificationKind::System, full);
        }
    }
}
