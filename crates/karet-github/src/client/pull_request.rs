//! Pull-request conversation reads and mutations.

use super::*;

impl GitHubClient {
    /// Fetch every commit currently contained in a pull request.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn pull_request_commits(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
    ) -> Result<Vec<PullRequestCommit>, GitHubError> {
        let mut commits = Vec::new();
        let mut page = 1_u32;
        loop {
            let response: Vec<PullRequestCommitResponse> = self
                .get_json(
                    &format!(
                        "/repos/{}/{}/pulls/{number}/commits",
                        repository.owner, repository.repo
                    ),
                    &[
                        ("per_page", PER_PAGE.to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let complete = response.len() < PER_PAGE as usize;
            commits.extend(response.into_iter().map(|commit| {
                let identity = commit.commit.author.or(commit.commit.committer);
                PullRequestCommit {
                    sha: commit.sha,
                    summary: commit
                        .commit
                        .message
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                    author: identity
                        .as_ref()
                        .map_or_else(|| "unknown".to_string(), |author| author.name.clone()),
                    committed_unix: identity.map_or(0, |author| author.date.unix_timestamp()),
                    parents: commit
                        .parents
                        .into_iter()
                        .map(|parent| parent.sha)
                        .collect(),
                    html_url: commit.html_url,
                }
            }));
            if complete {
                return Ok(commits);
            }
            page += 1;
        }
    }

    /// Fetch every check run attached to a commit reference.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn check_runs(
        &self,
        repository: &RepositoryIdentity,
        reference: &str,
    ) -> Result<Vec<CheckRun>, GitHubError> {
        let mut checks = Vec::new();
        let mut page = 1_u32;
        loop {
            let response: CheckRunsResponse = self
                .get_json(
                    &format!(
                        "/repos/{}/{}/commits/{reference}/check-runs",
                        repository.owner, repository.repo
                    ),
                    &[
                        ("per_page", PER_PAGE.to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            checks.extend(response.check_runs.into_iter().map(|check| CheckRun {
                id: check.id,
                name: check.name,
                status: check.status,
                conclusion: check.conclusion,
                html_url: check.html_url,
            }));
            if u64::try_from(checks.len()).unwrap_or(u64::MAX) >= response.total_count {
                return Ok(checks);
            }
            page += 1;
        }
    }

    /// Fetch non-comment activity for a pull request conversation.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error. GitHub may reject this endpoint
    /// for tokens without access to the repository's timeline surface.
    pub async fn pull_request_timeline(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
    ) -> Result<Vec<TimelineEvent>, GitHubError> {
        let mut events = Vec::new();
        let mut page = 1_u32;
        loop {
            let response: Vec<TimelineEventResponse> = self
                .get_json(
                    &format!(
                        "/repos/{}/{}/issues/{number}/timeline",
                        repository.owner, repository.repo
                    ),
                    &[
                        ("per_page", PER_PAGE.to_string()),
                        ("page", page.to_string()),
                    ],
                )
                .await?;
            let complete = response.len() < PER_PAGE as usize;
            events.extend(response.into_iter().filter_map(|event| {
                let kind = event.event?;
                // Comments are loaded from the dedicated comments endpoint and rendered
                // as conversation bubbles, so the activity rail must not duplicate them.
                (kind != "commented").then(|| TimelineEvent {
                    id: event.id,
                    kind,
                    actor: event.actor.map(|actor| actor.login),
                    commit_id: event.commit_id,
                    before: event.before,
                    after: event.after,
                    created_unix: event.created_at.map(|created| created.unix_timestamp()),
                })
            }));
            if complete {
                return Ok(events);
            }
            page += 1;
        }
    }

    /// Replace a pull request's Markdown body.
    ///
    /// # Errors
    /// Returns authentication or a generated-client error. The mutation is never retried.
    pub async fn update_pull_request_body(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
        body: String,
    ) -> Result<PullRequestSummary, GitHubError> {
        self.require_write()?;
        let request = generated::types::RequestBodyF1d64032 {
            title: None,
            body: Some(body),
            state: None,
            base: None,
            maintainer_can_modify: None,
        };
        let pull_number =
            i64::try_from(number).map_err(|error| GitHubError::Request(error.to_string()))?;
        let response = self
            .generated
            .pulls_update(
                repository.owner.clone(),
                repository.repo.clone(),
                pull_number,
                &request,
            )
            .await
            .map_err(generated_error)?;
        Ok(map_pull_request(response.into_inner()))
    }

    /// Add a Markdown comment to a pull request conversation.
    ///
    /// # Errors
    /// Returns authentication, transport, status, or decode errors. The mutation is
    /// issued exactly once and is never automatically retried.
    pub async fn create_pull_request_comment(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
        body: String,
    ) -> Result<Comment, GitHubError> {
        self.require_write()?;
        self.post_json(
            &format!(
                "/repos/{}/{}/issues/{number}/comments",
                repository.owner, repository.repo
            ),
            &CommentBody { body },
        )
        .await
    }

    /// Merge a pull request using the repository's ordinary merge method.
    ///
    /// # Errors
    /// Returns authentication or a generated-client error, including GitHub's reason
    /// when it accepts the request but refuses to merge.
    pub async fn merge_pull_request(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
        head_sha: String,
    ) -> Result<(), GitHubError> {
        self.require_write()?;
        let pull_number =
            i64::try_from(number).map_err(|error| GitHubError::Request(error.to_string()))?;
        let body = Some(generated::types::RequestBody87136dc8 {
            commit_title: None,
            commit_message: None,
            sha: Some(head_sha),
            merge_method: None,
        });
        let result = self
            .generated
            .pulls_merge(
                repository.owner.clone(),
                repository.repo.clone(),
                pull_number,
                &body,
            )
            .await
            .map_err(generated_error)?
            .into_inner();
        if result.merged {
            Ok(())
        } else {
            Err(GitHubError::Request(result.message))
        }
    }

    /// Convert a pull request to draft or mark it ready for review.
    ///
    /// # Errors
    /// Returns authentication, transport, status, or GraphQL decode errors. The
    /// mutation is issued exactly once and is never automatically retried.
    pub async fn set_pull_request_draft(
        &self,
        node_id: String,
        draft: bool,
    ) -> Result<(), GitHubError> {
        self.require_write()?;
        let query = if draft {
            "mutation($id: ID!) { convertPullRequestToDraft(input: {pullRequestId: $id}) { pullRequest { id } } }"
        } else {
            "mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { pullRequest { id } } }"
        };
        let response: GraphqlResponse = self
            .post_json(
                "/graphql",
                &GraphqlRequest {
                    query,
                    variables: GraphqlVariables { id: node_id },
                },
            )
            .await?;
        if response.errors.is_empty() {
            Ok(())
        } else {
            Err(GitHubError::Request(
                response
                    .errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; "),
            ))
        }
    }
}

#[derive(Serialize)]
struct CommentBody {
    body: String,
}

#[derive(Serialize)]
struct GraphqlRequest<'a> {
    query: &'a str,
    variables: GraphqlVariables,
}

#[derive(Serialize)]
struct GraphqlVariables {
    id: String,
}

#[derive(Deserialize)]
struct GraphqlResponse {
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

pub(super) fn map_pull_request(pull: generated::types::PullRequest) -> PullRequestSummary {
    PullRequestSummary {
        number: u64::try_from(pull.number).unwrap_or_default(),
        title: pull.title,
        body: pull.body,
        state: pull.state.to_string(),
        creator: Some(pull.user.login),
        creator_id: u64::try_from(pull.user.id).ok(),
        created_at: pull.created_at,
        updated_at: pull.updated_at,
        labels: pull
            .labels
            .into_iter()
            .map(|label| Label {
                name: label.name,
                color: label.color,
                description: label.description,
            })
            .collect(),
        draft: pull.draft.unwrap_or(false),
        node_id: pull.node_id,
        head_sha: pull.head.sha,
        base_sha: pull.base.sha,
        mergeable: pull.mergeable,
        merged: pull.merged,
        html_url: pull.html_url,
    }
}
