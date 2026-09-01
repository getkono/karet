//! Opening GitHub selections and refreshing the GitHub page in front.

use super::*;

impl App {
    pub(super) fn refresh_active_github(&mut self) -> bool {
        enum Refresh {
            Dashboard,
            Issue(u64),
            PullRequest(u64),
            Actions,
            IssueMetadata,
        }
        let refresh = match self.github.active_page() {
            Some(GithubViewState::Dashboard(_)) => Refresh::Dashboard,
            Some(GithubViewState::Issue { number, .. }) => Refresh::Issue(*number),
            Some(GithubViewState::PullRequest(view)) => {
                Refresh::PullRequest(view.pull_request.number)
            },
            Some(GithubViewState::WorkflowRun { .. }) => Refresh::Actions,
            Some(GithubViewState::NewIssue { .. }) => Refresh::IssueMetadata,
            _ => return false,
        };
        if matches!(refresh, Refresh::Dashboard) {
            self.request_github_section();
            return true;
        }
        let command = match refresh {
            Refresh::Dashboard => return true,
            Refresh::Issue(number) => SessionCommand::GithubIssue { number },
            Refresh::PullRequest(number) => SessionCommand::GithubPullRequest { number },
            Refresh::Actions => SessionCommand::GithubActions { page: 1 },
            Refresh::IssueMetadata => SessionCommand::GithubIssueMetadata,
        };
        let request = self.send(command);
        if let Some(view) = self.github.active_page_mut() {
            match view {
                GithubViewState::Issue {
                    pending,
                    loading_since,
                    error,
                    ..
                } => {
                    *pending = request;
                    *loading_since = Pending::start();
                    *error = None;
                },
                GithubViewState::PullRequest(view) => {
                    view.pending = request;
                    view.loading_since = Pending::start();
                    view.error = None;
                },
                GithubViewState::NewIssue { form, .. } => {
                    form.metadata_pending = request;
                    form.error = None;
                },
                _ => {},
            }
        }
        true
    }
    pub(super) fn open_github_selection(&mut self) {
        enum Selection {
            Issue(GithubRepository, u64),
            PullRequest(GithubRepository, GithubPullRequest, bool),
            WorkflowRun(GithubRepository, Option<GithubWorkflow>, GithubWorkflowRun),
        }
        let selection = self.active_dashboard_mut().and_then(|dashboard| {
            let repository = dashboard.repository.clone();
            match dashboard.section {
                GithubSection::Issues => dashboard
                    .issues
                    .items
                    .get(dashboard.cursor)
                    .map(|issue| Selection::Issue(repository, issue.number)),
                GithubSection::PullRequests => dashboard
                    .pull_requests
                    .items
                    .get(dashboard.cursor)
                    .cloned()
                    .map(|pull_request| {
                        Selection::PullRequest(repository, pull_request, dashboard.auth.can_write)
                    }),
                GithubSection::Actions => {
                    dashboard
                        .runs
                        .items
                        .get(dashboard.cursor)
                        .cloned()
                        .map(|run| {
                            let workflow = dashboard
                                .workflows
                                .items
                                .iter()
                                .find(|workflow| workflow.id == run.workflow_id)
                                .cloned();
                            Selection::WorkflowRun(repository, workflow, run)
                        })
                },
            }
        });
        let Some(selection) = selection else {
            return;
        };
        // Focus an open page *before* sending. `push` drops the page it is handed when
        // one for the same resource is already open, and with it the request id that
        // page was carrying — so a request sent first would have no owner to correlate
        // its reply against, and its error would surface as a toast for something the
        // user never knowingly asked for. Re-opening therefore shows what is loaded;
        // `Ctrl+R` refetches it.
        match selection {
            Selection::Issue(_repository, number) => {
                if self.focus_open_github_page(&github_issue(number, None)) {
                    return;
                }
                let request = self.send(SessionCommand::GithubIssue { number });
                self.push_github_page(github_issue(number, request));
            },
            Selection::PullRequest(_repository, pull_request, can_write) => {
                if self.focus_open_github_page(&github_pull_request(
                    pull_request.clone(),
                    can_write,
                    None,
                )) {
                    return;
                }
                let request = self.send(SessionCommand::GithubPullRequest {
                    number: pull_request.number,
                });
                self.push_github_page(github_pull_request(pull_request, can_write, request));
            },
            Selection::WorkflowRun(repository, workflow, run) => {
                self.push_github_page(github_workflow_run(repository, workflow, run));
            },
        }
    }

    pub(super) fn open_github_creation_form(&mut self) {
        let Some((repository, section, can_write)) = self
            .active_dashboard_mut()
            .map(|d| (d.repository.clone(), d.section, d.auth.can_write))
        else {
            return;
        };
        if !can_write {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "GitHub sign-in is required to create items",
            );
            return;
        }
        match section {
            GithubSection::Issues => {
                let pending = self.send(SessionCommand::GithubIssueMetadata);
                self.push_github_page(github_new_issue(repository, pending));
            },
            GithubSection::PullRequests => {
                self.push_github_page(github_new_pull_request(repository));
            },
            GithubSection::Actions => {
                self.notify(
                    Report::Refusal,
                    NotificationKind::System,
                    "workflow dispatch is not available in this build",
                );
            },
        }
    }
}
