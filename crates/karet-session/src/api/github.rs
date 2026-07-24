/// A forge's verification verdict for a commit signature (see
/// [`super::Event::CommitVerification`]). Mirrors GitHub's `commit.verification`;
/// defined here (rather than re-exported from `karet-github`) so the seam stays stable
/// whether or not the `github` feature is compiled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubVerification {
    /// Whether the forge considers the signature verified.
    pub verified: bool,
    /// The forge's machine reason (`valid`, `unsigned`, `unknown_key`, …).
    pub reason: String,
    /// The signer the forge attributes the commit to, when present.
    pub signer: Option<String>,
}

/// A public GitHub repository selected for the session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubRepository {
    /// Owner or organization login.
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

/// Credential source, with no secret material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GithubAuthSource {
    /// Anonymous public-read mode.
    Anonymous,
    /// `GITHUB_TOKEN`.
    GithubToken,
    /// `GH_TOKEN`.
    GhToken,
    /// GitHub CLI credential.
    GithubCli,
    /// Explicit embedding/test credential.
    Explicit,
}

/// Safe authentication/capability state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubAuth {
    /// Credential source.
    pub source: GithubAuthSource,
    /// Whether mutation controls may be enabled.
    pub can_write: bool,
    /// Stable numeric identifier for the authenticated account, when known.
    #[serde(default)]
    pub viewer_id: Option<u64>,
    /// Login name of the authenticated account, when known.
    #[serde(default)]
    pub viewer_login: Option<String>,
}

/// A transient GitHub token whose debug representation never exposes the secret.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubToken(String);

impl GithubToken {
    /// Wrap a token received from an interactive presentation.
    #[must_use]
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// Consume the wrapper for immediate authentication.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for GithubToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GithubToken(***)")
    }
}

/// A GitHub label used in issue and pull-request tables.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubLabel {
    /// Label name.
    pub name: String,
    /// Six-digit RGB colour without `#`.
    pub color: String,
}

/// An issue table row and detail header.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubIssue {
    /// Repository-local issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Raw Markdown body.
    pub body: Option<String>,
    /// Open/closed state.
    pub state: String,
    /// Creator login.
    pub creator: Option<String>,
    /// Stable numeric identifier of the creator account.
    #[serde(default)]
    pub creator_id: Option<u64>,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// Applied labels.
    pub labels: Vec<GithubLabel>,
    /// Whether another issue blocks this issue.
    pub blocked: bool,
    /// GitHub web URL.
    pub html_url: String,
}

/// One issue or pull-request timeline comment.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubComment {
    /// Stable API identifier.
    pub id: u64,
    /// Author login.
    pub creator: Option<String>,
    /// Raw Markdown body.
    pub body: String,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// GitHub web URL.
    pub html_url: String,
}

/// A pull-request table row.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequest {
    /// Repository-local pull-request number.
    pub number: u64,
    /// Pull-request title.
    pub title: String,
    /// Raw Markdown body.
    pub body: Option<String>,
    /// Open/closed state.
    pub state: String,
    /// Creator login.
    pub creator: Option<String>,
    /// Stable numeric identifier of the creator account.
    #[serde(default)]
    pub creator_id: Option<u64>,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
    /// Applied labels.
    pub labels: Vec<GithubLabel>,
    /// Whether this is a draft.
    pub draft: bool,
    /// GraphQL node identifier used by draft/readiness mutations.
    #[serde(default)]
    pub node_id: String,
    /// Head commit SHA.
    #[serde(default)]
    pub head_sha: String,
    /// Base commit SHA.
    #[serde(default)]
    pub base_sha: String,
    /// Whether GitHub currently considers the pull request mergeable.
    #[serde(default)]
    pub mergeable: Option<bool>,
    /// Whether the pull request has already been merged.
    #[serde(default)]
    pub merged: bool,
    /// GitHub web URL.
    pub html_url: String,
}

/// One commit in a GitHub pull request.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequestCommit {
    /// Full commit SHA.
    pub sha: String,
    /// First line of the commit message.
    pub summary: String,
    /// Author display name.
    pub author: String,
    /// Commit timestamp in Unix seconds.
    pub committed_unix: i64,
    /// Parent commit SHAs.
    pub parents: Vec<String>,
    /// GitHub web URL.
    pub html_url: String,
}

/// One check run attached to a pull request head.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubCheckRun {
    /// Stable check-run identifier.
    pub id: u64,
    /// Check name.
    pub name: String,
    /// Queued/in-progress/completed state.
    pub status: String,
    /// Final result when complete.
    pub conclusion: Option<String>,
    /// GitHub web URL for the check details.
    pub html_url: String,
}

/// One non-comment event in a pull-request conversation timeline.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPullRequestActivity {
    /// Stable event identifier when GitHub supplies one.
    pub id: Option<u64>,
    /// GitHub event kind, such as `committed` or `head_ref_force_pushed`.
    pub kind: String,
    /// Actor login when present.
    pub actor: Option<String>,
    /// Commit involved in the event when present.
    pub commit_id: Option<String>,
    /// Previous head SHA for a force-push event.
    pub before: Option<String>,
    /// New head SHA for a force-push event.
    pub after: Option<String>,
    /// Event timestamp in Unix seconds when present.
    pub created_unix: Option<i64>,
}

/// A GitHub Actions workflow.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubWorkflow {
    /// Workflow identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Repository-relative workflow path.
    pub path: String,
    /// GitHub workflow state.
    pub state: String,
    /// Last-update timestamp in Unix seconds.
    pub updated_unix: i64,
}

/// A GitHub Actions workflow run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubWorkflowRun {
    /// Run identifier.
    pub id: u64,
    /// Workflow identifier.
    pub workflow_id: u64,
    /// Display title.
    pub title: String,
    /// Branch, when present.
    pub branch: Option<String>,
    /// Head SHA.
    pub head_sha: String,
    /// Trigger event.
    pub event: String,
    /// Queued/in-progress/completed state.
    pub status: Option<String>,
    /// Final conclusion.
    pub conclusion: Option<String>,
    /// Actor login.
    pub actor: Option<String>,
    /// Repository-local run number.
    pub run_number: u64,
    /// Creation timestamp in Unix seconds.
    pub created_unix: i64,
    /// GitHub web URL.
    pub html_url: String,
}

/// A generic GitHub result page.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubPage<T> {
    /// Rows in this page.
    pub items: Vec<T>,
    /// Current one-based page.
    pub page: u32,
    /// Next page, when supplied.
    pub next_page: Option<u32>,
    /// Total result count, when supplied.
    pub total_count: Option<u64>,
}

/// Primary fields for issue creation.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubNewIssue {
    /// Issue title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Assignee logins.
    pub assignees: Vec<String>,
    /// Label names.
    pub labels: Vec<String>,
    /// Milestone number.
    pub milestone: Option<u64>,
    /// Repository issue-type identifier.
    pub issue_type: Option<String>,
}

/// Primary fields for pull-request creation.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GithubNewPullRequest {
    /// Pull-request title.
    pub title: String,
    /// Source branch or owner-qualified source branch.
    pub head: String,
    /// Destination branch.
    pub base: String,
    /// Markdown body.
    pub body: String,
    /// Whether to create a draft.
    pub draft: bool,
    /// Whether maintainers may modify the source branch.
    pub maintainer_can_modify: bool,
}
