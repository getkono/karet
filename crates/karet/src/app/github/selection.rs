//! Opening GitHub selections and refreshing the active GitHub tab.

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
        let refresh = match self.tabs.get(self.active).map(|tab| &tab.kind) {
            Some(TabKind::Github(GithubViewState::Dashboard(_))) => Refresh::Dashboard,
            Some(TabKind::Github(GithubViewState::Issue { number, .. })) => Refresh::Issue(*number),
            Some(TabKind::Github(GithubViewState::PullRequest(view))) => {
                Refresh::PullRequest(view.pull_request.number)
            },
            Some(TabKind::Github(GithubViewState::WorkflowRun { .. })) => Refresh::Actions,
            Some(TabKind::Github(GithubViewState::NewIssue { .. })) => Refresh::IssueMetadata,
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
        if let Some(TabKind::Github(view)) = self.tabs.get_mut(self.active).map(|tab| &mut tab.kind)
        {
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
        match selection {
            Selection::Issue(_repository, number) => {
                let request = self.send(SessionCommand::GithubIssue { number });
                self.push_tab(Tab::github_issue(number, request));
            },
            Selection::PullRequest(_repository, pull_request, can_write) => {
                let request = self.send(SessionCommand::GithubPullRequest {
                    number: pull_request.number,
                });
                self.push_tab(Tab::github_pull_request(pull_request, can_write, request));
            },
            Selection::WorkflowRun(repository, workflow, run) => {
                self.push_tab(Tab::github_workflow_run(repository, workflow, run));
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
            self.status = Some("GitHub sign-in is required to create items".to_string());
            return;
        }
        match section {
            GithubSection::Issues => {
                let pending = self.send(SessionCommand::GithubIssueMetadata);
                self.push_tab(Tab::github_new_issue(repository, pending));
            },
            GithubSection::PullRequests => self.push_tab(Tab::github_new_pull_request(repository)),
            GithubSection::Actions => {
                self.status = Some("workflow dispatch is not available in this build".to_string());
            },
        }
    }
}
