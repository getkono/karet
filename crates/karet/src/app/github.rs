//! GitHub dashboard, detail, and creation-form application state.

mod availability;
mod dashboard;
mod events;
mod forms;
mod keys;
mod mouse;
mod pull_request;
mod scroll;
mod selection;
mod state;
mod surface;

pub(crate) use forms::GithubIssueForm;
pub(crate) use forms::GithubPullRequestForm;
pub(crate) use forms::auth_label;
pub(crate) use state::GithubDashboard;
pub(crate) use state::GithubFormField;
pub(crate) use state::GithubPullRequestEditor;
pub(crate) use state::GithubPullRequestSection;
pub(crate) use state::GithubPullRequestSupplement;
pub(crate) use state::GithubPullRequestView;
pub(crate) use state::GithubSection;
pub(crate) use state::GithubViewState;
pub(crate) use surface::GithubSurface;
#[cfg(test)]
pub(crate) use surface::github_issue;
#[cfg(test)]
pub(crate) use surface::github_new_issue;
#[cfg(test)]
pub(crate) use surface::github_pull_request;
#[cfg(test)]
pub(crate) use surface::github_workflow_run;

/// Fixed visual height of each dashboard result card.
pub(crate) const DASHBOARD_ROW_HEIGHT: usize = 3;
