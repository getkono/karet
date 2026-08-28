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
use karet_session::GithubAuth;
use karet_session::GithubCheckRun;
use karet_session::GithubComment;
use karet_session::GithubIssue;
use karet_session::GithubNewIssue;
use karet_session::GithubNewPullRequest;
use karet_session::GithubPage;
use karet_session::GithubPullRequest;
use karet_session::GithubPullRequestActivity;
use karet_session::GithubPullRequestCommit;
use karet_session::GithubRepository;
use karet_session::GithubWorkflow;
use karet_session::GithubWorkflowRun;
pub(crate) use state::GithubDashboard;
pub(crate) use state::GithubFormField;
pub(crate) use state::GithubPullRequestEditor;
pub(crate) use state::GithubPullRequestSection;
pub(crate) use state::GithubPullRequestSupplement;
pub(crate) use state::GithubPullRequestView;
pub(crate) use state::GithubSection;
pub(crate) use state::GithubViewState;
pub(crate) use surface::GithubSurface;
pub(crate) use surface::github_issue;
pub(crate) use surface::github_new_issue;
pub(crate) use surface::github_new_pull_request;
pub(crate) use surface::github_pull_request;
pub(crate) use surface::github_workflow_run;

use super::*;

/// Fixed visual height of each dashboard result card.
pub(crate) const DASHBOARD_ROW_HEIGHT: usize = 3;
