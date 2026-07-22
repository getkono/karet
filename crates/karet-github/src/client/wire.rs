//! Hand-reviewed GitHub response shapes whose RFC-3339 timestamps are not
//! represented correctly by the generated client's default `time` adapters.

use serde::Deserialize;

use crate::models::Label;
use crate::models::PullRequestSummary;
use crate::models::RateLimit;
use crate::models::Workflow;
use crate::models::WorkflowRun;

#[derive(Deserialize)]
pub(super) struct SearchResponse<T> {
    pub(super) total_count: u64,
    pub(super) items: Vec<T>,
    #[serde(skip, default)]
    pub(super) rate_limit: RateLimit,
}

#[derive(Deserialize)]
pub(super) struct PullRequestResponse {
    number: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    user: Option<crate::models::User>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    #[serde(default)]
    labels: Vec<Label>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    head: PullRequestRef,
    #[serde(default)]
    base: PullRequestRef,
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    merged: bool,
    html_url: String,
}

#[derive(Default, Deserialize)]
struct PullRequestRef {
    sha: String,
}

impl PullRequestResponse {
    pub(super) fn into_summary(self) -> PullRequestSummary {
        let (creator, creator_id) = self
            .user
            .map(|user| (Some(user.login), Some(user.id)))
            .unwrap_or_default();
        PullRequestSummary {
            number: self.number,
            title: self.title,
            body: self.body,
            state: self.state,
            creator,
            creator_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            labels: self.labels,
            draft: self.draft,
            node_id: self.node_id,
            head_sha: self.head.sha,
            base_sha: self.base.sha,
            mergeable: self.mergeable,
            merged: self.merged,
            html_url: self.html_url,
        }
    }
}

#[derive(Deserialize)]
pub(super) struct PullRequestCommitResponse {
    pub(super) sha: String,
    pub(super) commit: PullRequestCommitData,
    #[serde(default)]
    pub(super) parents: Vec<CommitParent>,
    pub(super) html_url: String,
}

#[derive(Deserialize)]
pub(super) struct PullRequestCommitData {
    pub(super) message: String,
    pub(super) author: Option<GitIdentity>,
    pub(super) committer: Option<GitIdentity>,
}

#[derive(Deserialize)]
pub(super) struct GitIdentity {
    pub(super) name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(super) date: time::OffsetDateTime,
}

#[derive(Deserialize)]
pub(super) struct CommitParent {
    pub(super) sha: String,
}

#[derive(Deserialize)]
pub(super) struct CheckRunsResponse {
    pub(super) total_count: u64,
    pub(super) check_runs: Vec<CheckRunResponse>,
}

#[derive(Deserialize)]
pub(super) struct CheckRunResponse {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) status: String,
    #[serde(default)]
    pub(super) conclusion: Option<String>,
    pub(super) html_url: String,
}

#[derive(Deserialize)]
pub(super) struct TimelineEventResponse {
    #[serde(default)]
    pub(super) id: Option<u64>,
    #[serde(default)]
    pub(super) event: Option<String>,
    #[serde(default)]
    pub(super) actor: Option<crate::models::User>,
    #[serde(default)]
    pub(super) commit_id: Option<String>,
    #[serde(default)]
    pub(super) before: Option<String>,
    #[serde(default)]
    pub(super) after: Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) created_at: Option<time::OffsetDateTime>,
}

#[derive(Deserialize)]
pub(super) struct WorkflowResponse {
    pub(super) total_count: u64,
    pub(super) workflows: Vec<Workflow>,
}

#[derive(Deserialize)]
pub(super) struct WorkflowRunsResponse {
    pub(super) total_count: u64,
    pub(super) workflow_runs: Vec<WorkflowRunResponse>,
}

#[derive(Deserialize)]
pub(super) struct WorkflowRunResponse {
    id: u64,
    workflow_id: u64,
    display_title: String,
    #[serde(default)]
    head_branch: Option<String>,
    head_sha: String,
    event: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    actor: Option<crate::models::User>,
    pub(super) run_number: u64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: time::OffsetDateTime,
    html_url: String,
}

impl WorkflowRunResponse {
    pub(super) fn into_workflow_run(self) -> WorkflowRun {
        WorkflowRun {
            id: self.id,
            workflow_id: self.workflow_id,
            title: self.display_title,
            branch: self.head_branch,
            head_sha: self.head_sha,
            event: self.event,
            status: self.status,
            conclusion: self.conclusion,
            actor: self.actor.map(|actor| actor.login),
            run_number: self.run_number,
            created_at: self.created_at,
            updated_at: self.updated_at,
            html_url: self.html_url,
        }
    }
}
