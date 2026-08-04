//! Stable, presentation-neutral GitHub models.

use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;

/// A GitHub user reduced to the fields repository workflows display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    /// Stable numeric GitHub account identifier.
    #[serde(default)]
    pub id: u64,
    /// Login name.
    pub login: String,
    /// Avatar URL.
    #[serde(default)]
    pub avatar_url: String,
    /// Profile URL.
    #[serde(default)]
    pub html_url: String,
}

/// A repository label.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// Label name.
    pub name: String,
    /// Six-digit RGB colour without `#`.
    #[serde(default)]
    pub color: String,
    /// Optional label description.
    #[serde(default)]
    pub description: Option<String>,
}

/// GitHub's dependency counts attached to an issue.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencySummary {
    /// Direct blockers returned on this page.
    #[serde(default)]
    pub blocked_by: u64,
    /// Directly blocked issues returned on this page.
    #[serde(default)]
    pub blocking: u64,
    /// Total blockers.
    #[serde(default)]
    pub total_blocked_by: u64,
    /// Total issues blocked by this issue.
    #[serde(default)]
    pub total_blocking: u64,
}

/// A row in the Issues table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSummary {
    /// Repository-local issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Raw Markdown body used to derive the description excerpt.
    #[serde(default)]
    pub body: Option<String>,
    /// GitHub state (`open` or `closed`).
    pub state: String,
    /// Creator, when the account still exists.
    #[serde(default)]
    pub user: Option<User>,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last update time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Applied labels.
    #[serde(default)]
    pub labels: Vec<Label>,
    /// Dependency counts when enabled for the repository.
    #[serde(default)]
    pub issue_dependencies_summary: Option<DependencySummary>,
    /// GitHub web URL.
    #[serde(default)]
    pub html_url: String,
    /// Whether this result is a pull request returned through the Issues API.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

impl IssueSummary {
    /// Whether another issue currently blocks this issue.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.issue_dependencies_summary
            .is_some_and(|summary| summary.total_blocked_by > 0 || summary.blocked_by > 0)
    }
}

/// A row in the Pull Requests table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestSummary {
    /// Repository-local pull-request number.
    pub number: u64,
    /// Pull-request title.
    pub title: String,
    /// Raw Markdown body used to derive the description excerpt.
    pub body: Option<String>,
    /// GitHub state.
    pub state: String,
    /// Creator login.
    pub creator: Option<String>,
    /// Stable numeric identifier of the creator account.
    #[serde(default)]
    pub creator_id: Option<u64>,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last update time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// Applied labels.
    pub labels: Vec<Label>,
    /// Whether this is a draft pull request.
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

/// One commit in a pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestCommit {
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckRun {
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

/// One non-comment pull-request timeline event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEvent {
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

/// One issue or pull-request timeline comment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// Stable API identifier.
    pub id: u64,
    /// Comment author.
    pub user: Option<User>,
    /// Raw Markdown body.
    pub body: String,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last update time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// GitHub web URL.
    pub html_url: String,
}

/// A complete issue page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueDetail {
    /// Summary/header data.
    #[serde(flatten)]
    pub summary: IssueSummary,
    /// Current assignees.
    #[serde(default)]
    pub assignees: Vec<User>,
    /// Comment count reported by GitHub.
    #[serde(default)]
    pub comments: u64,
    /// Whether the conversation is locked.
    #[serde(default)]
    pub locked: bool,
}

/// A GitHub Actions workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Repository-relative workflow file.
    pub path: String,
    /// GitHub workflow state.
    pub state: String,
    /// GitHub web URL.
    pub html_url: String,
    /// Last update time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// A GitHub Actions workflow run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    /// Run identifier.
    pub id: u64,
    /// Workflow identifier.
    pub workflow_id: u64,
    /// Display title.
    pub title: String,
    /// Repository branch, when available.
    pub branch: Option<String>,
    /// Head commit SHA.
    pub head_sha: String,
    /// Triggering event.
    pub event: String,
    /// Queued/in-progress/completed state.
    pub status: Option<String>,
    /// Final conclusion, when complete.
    pub conclusion: Option<String>,
    /// Actor login.
    pub actor: Option<String>,
    /// Repository-local run number.
    pub run_number: u64,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Last update time.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    /// GitHub web URL.
    pub html_url: String,
}

/// Rate-limit values returned with a response.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    /// Request allowance for the resource bucket.
    pub limit: Option<u64>,
    /// Requests remaining.
    pub remaining: Option<u64>,
    /// Unix reset timestamp.
    pub reset: Option<i64>,
}

/// One paginated result page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    /// Rows in this page.
    pub items: Vec<T>,
    /// Current one-based page.
    pub page: u32,
    /// Next page if GitHub supplied one.
    pub next_page: Option<u32>,
    /// Total result count when the endpoint supplies it.
    pub total_count: Option<u64>,
    /// Response rate-limit state.
    pub rate_limit: RateLimit,
}

/// Fields accepted when creating an issue.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewIssue {
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
    pub r#type: Option<String>,
    /// Issue field values; values are deliberately restricted to GitHub's documented scalars.
    pub issue_field_values: Vec<IssueFieldInput>,
}

/// One issue-field value used during issue creation or update.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IssueFieldInput {
    /// Issue-field identifier.
    pub field_id: u64,
    /// Typed field value.
    pub value: IssueFieldInputValue,
}

/// A request-safe issue-field value.
#[derive(Clone, Debug, PartialEq)]
pub enum IssueFieldInputValue {
    /// Text or single-select value.
    Text(String),
    /// Numeric value.
    Number(f64),
    /// Multi-select value.
    TextList(Vec<String>),
}

impl Serialize for IssueFieldInputValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Text(value) => value.serialize(serializer),
            Self::Number(value) => value.serialize(serializer),
            Self::TextList(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for IssueFieldInputValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(value) => Ok(Self::Text(value)),
            serde_json::Value::Number(value) => value
                .as_f64()
                .map(Self::Number)
                .ok_or_else(|| D::Error::custom("issue field number is outside f64 range")),
            serde_json::Value::Array(values) => values
                .into_iter()
                .map(|value| match value {
                    serde_json::Value::String(value) => Ok(value),
                    _ => Err(D::Error::custom(
                        "issue field arrays may contain only strings",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Self::TextList),
            _ => Err(D::Error::custom(
                "issue field value must be a string, number, or string array",
            )),
        }
    }
}

#[cfg(test)]
mod issue_field_tests {
    use super::IssueFieldInputValue;

    #[test]
    fn issue_field_values_round_trip_without_ambiguous_shapes() -> Result<(), serde_json::Error> {
        let values = [
            IssueFieldInputValue::Text("ready".to_string()),
            IssueFieldInputValue::Number(2.5),
            IssueFieldInputValue::TextList(vec!["one".to_string(), "two".to_string()]),
        ];
        for value in values {
            let encoded = serde_json::to_string(&value)?;
            let decoded: IssueFieldInputValue = serde_json::from_str(&encoded)?;
            assert_eq!(decoded, value);
        }
        assert!(serde_json::from_str::<IssueFieldInputValue>("[1]").is_err());
        Ok(())
    }
}

/// Fields accepted when creating a pull request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPullRequest {
    /// Pull-request title.
    pub title: String,
    /// Source branch or `owner:branch` reference.
    pub head: String,
    /// Destination branch.
    pub base: String,
    /// Markdown body.
    pub body: String,
    /// Whether to create the pull request as a draft.
    pub draft: bool,
    /// Whether maintainers may modify the source branch.
    pub maintainer_can_modify: bool,
}
