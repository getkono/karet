//! Authenticated asynchronous GitHub transport and curated API operations.

mod pull_request;
use pull_request::map_pull_request;
#[cfg(test)]
mod tests;
mod wire;

use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::ACCEPT;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::header::USER_AGENT;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use serde::Deserialize;
use serde::Serialize;
use wire::*;

use crate::Verification;
use crate::generated;
use crate::models::CheckRun;
use crate::models::Comment;
use crate::models::IssueDetail;
use crate::models::IssueSummary;
use crate::models::Label;
use crate::models::NewIssue;
use crate::models::NewPullRequest;
use crate::models::Page;
use crate::models::PullRequestCommit;
use crate::models::PullRequestSummary;
use crate::models::RateLimit;
use crate::models::TimelineEvent;
use crate::models::Workflow;
use crate::models::WorkflowRun;
use crate::remote::RepositoryIdentity;

const API_BASE: &str = "https://api.github.com";
const API_VERSION: &str = "2026-03-10";
const PER_PAGE: u32 = 50;

/// Where the active GitHub credential came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AuthSource {
    /// No credential; only public reads are available.
    Anonymous,
    /// `GITHUB_TOKEN`.
    GitHubToken,
    /// `GH_TOKEN`.
    GhToken,
    /// The authenticated GitHub CLI credential.
    GitHubCli,
    /// A credential supplied directly by the embedding application or a test.
    Explicit,
}

/// Authentication and write-capability state safe to expose to presentation code.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuthState {
    /// Credential source.
    pub source: AuthSource,
    /// Whether mutating controls may be enabled.
    pub can_write: bool,
    /// Stable numeric identifier for the authenticated account, when known.
    #[serde(default)]
    pub viewer_id: Option<u64>,
    /// Login name of the authenticated account, when known.
    #[serde(default)]
    pub viewer_login: Option<String>,
}

/// Errors returned by the curated GitHub client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GitHubError {
    /// A request could not be constructed.
    #[error("invalid GitHub request: {0}")]
    Request(String),
    /// A GitHub search tried to escape the active repository or object type.
    #[error("query qualifier `{0}` is controlled by the GitHub view")]
    QueryScope(String),
    /// Network or TLS failure.
    #[error("GitHub transport failed: {0}")]
    Transport(String),
    /// GitHub did not answer before the configured timeout.
    #[error("GitHub request timed out")]
    Timeout,
    /// GitHub returned a non-success status.
    #[error("GitHub returned HTTP {status}: {message}")]
    Status {
        /// HTTP status code.
        status: u16,
        /// Safe response message.
        message: String,
        /// GitHub request identifier for support/debugging.
        request_id: Option<String>,
    },
    /// A successful response did not match the reviewed schema.
    #[error("GitHub response did not match its schema: {0}")]
    Decode(String),
    /// A mutation was attempted without a credential.
    #[error("GitHub authentication is required for this action")]
    AuthenticationRequired,
    /// A spargen-generated operation failed.
    #[error("generated GitHub operation failed: {0}")]
    Generated(String),
}

/// A cloneable asynchronous GitHub client.
#[derive(Clone)]
pub struct GitHubClient {
    http: reqwest::Client,
    generated: Arc<generated::Client>,
    auth: AuthState,
}

impl GitHubClient {
    /// Build a client using environment variables, then `gh auth token`, then
    /// anonymous public-read mode.
    ///
    /// # Errors
    /// Returns [`GitHubError::Request`] when the HTTP client cannot be configured.
    pub async fn discover() -> Result<Self, GitHubError> {
        let (token, source) = discover_token().await;
        let mut client = Self::with_auth(token, source)?;
        if client.auth.can_write {
            client.load_viewer().await;
        }
        Ok(client)
    }

    /// Build and validate a client from a token entered by an interactive host.
    ///
    /// # Errors
    /// Returns an authentication, transport, status, or decode error when GitHub
    /// cannot resolve the token to an account.
    pub async fn authenticate(token: String) -> Result<Self, GitHubError> {
        let mut client = Self::with_auth(Some(SecretString::from(token)), AuthSource::Explicit)?;
        let viewer = client.get_json::<crate::models::User>("/user", &[]).await?;
        client.auth.viewer_id = (viewer.id != 0).then_some(viewer.id);
        client.auth.viewer_login = Some(viewer.login);
        Ok(client)
    }

    /// Build a client with an explicit token. Intended for embedding and deterministic
    /// transport tests; the token is kept in secret memory and never exposed again.
    ///
    /// # Errors
    /// Returns [`GitHubError::Request`] when the HTTP client cannot be configured.
    pub fn with_token(token: Option<SecretString>) -> Result<Self, GitHubError> {
        let source = if token.is_some() {
            AuthSource::Explicit
        } else {
            AuthSource::Anonymous
        };
        Self::with_auth(token, source)
    }

    fn with_auth(token: Option<SecretString>, source: AuthSource) -> Result<Self, GitHubError> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("karet"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(API_VERSION),
        );
        if let Some(token) = token.as_ref() {
            let mut value = HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                .map_err(|error| GitHubError::Request(error.to_string()))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| GitHubError::Request(error.to_string()))?;
        let generated = generated::Client::with_client(http.clone(), API_BASE)
            .map_err(|error| GitHubError::Generated(format!("{error:?}")))?;
        Ok(Self {
            http,
            generated: Arc::new(generated),
            auth: AuthState {
                source,
                can_write: token.is_some(),
                viewer_id: None,
                viewer_login: None,
            },
        })
    }

    /// Current authentication state without credential material.
    #[must_use]
    pub fn auth_state(&self) -> AuthState {
        self.auth.clone()
    }

    async fn load_viewer(&mut self) {
        if let Ok(viewer) = self.get_json::<crate::models::User>("/user", &[]).await {
            self.auth.viewer_id = (viewer.id != 0).then_some(viewer.id);
            self.auth.viewer_login = Some(viewer.login);
        }
    }

    /// Search issues in exactly `repository` using GitHub query syntax.
    ///
    /// # Errors
    /// Returns a scoped-query, transport, status, or decode error.
    pub async fn search_issues(
        &self,
        repository: &RepositoryIdentity,
        query: &str,
        page: u32,
    ) -> Result<Page<IssueSummary>, GitHubError> {
        let scoped = scoped_query(repository, SearchKind::Issue, query)?;
        let response: SearchResponse<IssueSummary> = self
            .get_json(
                "/search/issues",
                &[
                    ("q", scoped),
                    ("per_page", PER_PAGE.to_string()),
                    ("page", page.max(1).to_string()),
                ],
            )
            .await?;
        Ok(Page {
            items: response.items,
            page: page.max(1),
            next_page: next_page(page, response.total_count),
            total_count: Some(response.total_count),
            rate_limit: response.rate_limit,
        })
    }

    /// Search pull requests in exactly `repository` using GitHub query syntax.
    ///
    /// # Errors
    /// Returns a scoped-query, transport, status, or decode error.
    pub async fn search_pull_requests(
        &self,
        repository: &RepositoryIdentity,
        query: &str,
        page: u32,
    ) -> Result<Page<PullRequestSummary>, GitHubError> {
        let scoped = scoped_query(repository, SearchKind::PullRequest, query)?;
        let response: SearchResponse<IssueSummary> = self
            .get_json(
                "/search/issues",
                &[
                    ("q", scoped),
                    ("per_page", PER_PAGE.to_string()),
                    ("page", page.max(1).to_string()),
                ],
            )
            .await?;
        let items = response
            .items
            .into_iter()
            .map(|issue| {
                let (creator, creator_id) = issue
                    .user
                    .map(|user| (Some(user.login), Some(user.id)))
                    .unwrap_or_default();
                PullRequestSummary {
                    number: issue.number,
                    title: issue.title,
                    body: issue.body,
                    state: issue.state,
                    creator,
                    creator_id,
                    created_at: issue.created_at,
                    updated_at: issue.updated_at,
                    labels: issue.labels,
                    draft: false,
                    node_id: String::new(),
                    head_sha: String::new(),
                    base_sha: String::new(),
                    mergeable: None,
                    merged: false,
                    html_url: issue.html_url,
                }
            })
            .collect();
        Ok(Page {
            items,
            page: page.max(1),
            next_page: next_page(page, response.total_count),
            total_count: Some(response.total_count),
            rate_limit: response.rate_limit,
        })
    }

    /// Fetch one issue.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn issue(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
    ) -> Result<IssueDetail, GitHubError> {
        self.get_json(
            &format!(
                "/repos/{}/{}/issues/{number}",
                repository.owner, repository.repo
            ),
            &[],
        )
        .await
    }

    /// Fetch one pull request through the reviewed RFC-3339 wire adapter.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn pull_request(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
    ) -> Result<PullRequestSummary, GitHubError> {
        let response: PullRequestResponse = self
            .get_json(
                &format!(
                    "/repos/{}/{}/pulls/{number}",
                    repository.owner, repository.repo
                ),
                &[],
            )
            .await?;
        Ok(response.into_summary())
    }

    /// Fetch GitHub's signature-verification verdict for a commit.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn commit_verification(
        &self,
        repository: &RepositoryIdentity,
        sha: &str,
    ) -> Result<Verification, GitHubError> {
        let response: CommitResponse = self
            .get_json(
                &format!(
                    "/repos/{}/{}/commits/{sha}",
                    repository.owner, repository.repo
                ),
                &[],
            )
            .await?;
        Ok(Verification {
            verified: response.commit.verification.verified,
            reason: response.commit.verification.reason,
            signer: response
                .author
                .or(response.committer)
                .map(|user| user.login),
        })
    }

    /// Fetch an issue's comments.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn issue_comments(
        &self,
        repository: &RepositoryIdentity,
        number: u64,
        page: u32,
    ) -> Result<Page<Comment>, GitHubError> {
        let items: Vec<Comment> = self
            .get_json(
                &format!(
                    "/repos/{}/{}/issues/{number}/comments",
                    repository.owner, repository.repo
                ),
                &[
                    ("per_page", PER_PAGE.to_string()),
                    ("page", page.max(1).to_string()),
                ],
            )
            .await?;
        let next = (items.len() == PER_PAGE as usize).then_some(page.max(1) + 1);
        Ok(Page {
            items,
            page: page.max(1),
            next_page: next,
            total_count: None,
            rate_limit: RateLimit::default(),
        })
    }

    /// List users who may be assigned to issues in `repository`.
    ///
    /// # Errors
    /// Returns a generated-client transport, status, or decode error.
    pub async fn issue_assignees(
        &self,
        repository: &RepositoryIdentity,
    ) -> Result<Vec<crate::models::User>, GitHubError> {
        const PAGE_SIZE: i64 = 100;
        let mut page = 1_i64;
        let mut assignees = Vec::new();
        loop {
            let params = generated::IssuesListAssigneesParams::default()
                .per_page(PAGE_SIZE)
                .page(page);
            let response = self
                .generated
                .issues_list_assignees(
                    repository.owner.clone(),
                    repository.repo.clone(),
                    Some(params),
                )
                .await
                .map_err(generated_error)?;
            let users = response.into_inner();
            let complete = users.len() < PAGE_SIZE as usize;
            assignees.extend(users.into_iter().map(|user| crate::models::User {
                id: u64::try_from(user.id).unwrap_or_default(),
                login: user.login,
                avatar_url: user.avatar_url,
                html_url: user.html_url,
            }));
            if complete {
                return Ok(assignees);
            }
            page += 1;
        }
    }

    /// Create an issue and return its complete primary resource.
    ///
    /// # Errors
    /// Returns authentication, transport, status, or decode errors. This mutation is
    /// issued exactly once and is never automatically retried.
    pub async fn create_issue(
        &self,
        repository: &RepositoryIdentity,
        issue: &NewIssue,
    ) -> Result<IssueDetail, GitHubError> {
        self.require_write()?;
        self.post_json(
            &format!("/repos/{}/{}/issues", repository.owner, repository.repo),
            issue,
        )
        .await
    }

    /// Create a pull request. The returned summary is produced by the spargen-generated
    /// operation, proving the build-generated surface is exercised in production.
    ///
    /// # Errors
    /// Returns authentication or a generated-client error. The mutation is never retried.
    pub async fn create_pull_request(
        &self,
        repository: &RepositoryIdentity,
        request: &NewPullRequest,
    ) -> Result<PullRequestSummary, GitHubError> {
        self.require_write()?;
        let body = generated::types::RequestBodyE714d37e {
            title: Some(request.title.clone()),
            head: request.head.clone(),
            head_repo: None,
            base: request.base.clone(),
            body: Some(request.body.clone()),
            maintainer_can_modify: Some(request.maintainer_can_modify),
            draft: Some(request.draft),
            issue: None,
        };
        let response = self
            .generated
            .pulls_create(repository.owner.clone(), repository.repo.clone(), &body)
            .await
            .map_err(generated_error)?;
        Ok(map_pull_request(response.into_inner()))
    }

    /// List repository workflows through the reviewed RFC-3339 wire adapter.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn workflows(
        &self,
        repository: &RepositoryIdentity,
        page: u32,
    ) -> Result<Page<Workflow>, GitHubError> {
        let page = page.max(1);
        let body: WorkflowResponse = self
            .get_json(
                &format!(
                    "/repos/{}/{}/actions/workflows",
                    repository.owner, repository.repo
                ),
                &[
                    ("per_page", PER_PAGE.to_string()),
                    ("page", page.to_string()),
                ],
            )
            .await?;
        Ok(Page {
            items: body.workflows,
            page,
            next_page: next_page(page, body.total_count),
            total_count: Some(body.total_count),
            rate_limit: RateLimit::default(),
        })
    }

    /// List repository workflow runs through the reviewed RFC-3339 wire adapter.
    ///
    /// # Errors
    /// Returns a transport, status, or decode error.
    pub async fn workflow_runs(
        &self,
        repository: &RepositoryIdentity,
        page: u32,
    ) -> Result<Page<WorkflowRun>, GitHubError> {
        let page = page.max(1);
        let body: WorkflowRunsResponse = self
            .get_json(
                &format!(
                    "/repos/{}/{}/actions/runs",
                    repository.owner, repository.repo
                ),
                &[
                    ("per_page", PER_PAGE.to_string()),
                    ("page", page.to_string()),
                ],
            )
            .await?;
        Ok(Page {
            items: body
                .workflow_runs
                .into_iter()
                .map(WorkflowRunResponse::into_workflow_run)
                .collect(),
            page,
            next_page: next_page(page, body.total_count),
            total_count: Some(body.total_count),
            rate_limit: RateLimit::default(),
        })
    }

    fn require_write(&self) -> Result<(), GitHubError> {
        if self.auth.can_write {
            Ok(())
        } else {
            Err(GitHubError::AuthenticationRequired)
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, GitHubError> {
        let url = format!("{API_BASE}{path}");
        let query: Vec<(&str, &str)> = query
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let response = self
            .http
            .get(url)
            .query(&query)
            .send()
            .await
            .map_err(map_reqwest)?;
        decode_response(response).await
    }

    async fn post_json<T, B>(&self, path: &str, body: &B) -> Result<T, GitHubError>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        let response = self
            .http
            .post(format!("{API_BASE}{path}"))
            .json(body)
            .send()
            .await
            .map_err(map_reqwest)?;
        decode_response(response).await
    }
}

#[derive(Clone, Copy)]
enum SearchKind {
    Issue,
    PullRequest,
}

fn scoped_query(
    repository: &RepositoryIdentity,
    kind: SearchKind,
    query: &str,
) -> Result<String, GitHubError> {
    for token in query_tokens(query) {
        let lower = token.to_ascii_lowercase();
        if lower.starts_with("repo:")
            || lower.starts_with("org:")
            || lower.starts_with("user:")
            || matches!(lower.as_str(), "is:issue" | "is:pr" | "is:pull-request")
        {
            return Err(GitHubError::QueryScope(token));
        }
    }
    let kind = match kind {
        SearchKind::Issue => "is:issue",
        SearchKind::PullRequest => "is:pr",
    };
    let user_query = if query.trim().is_empty() {
        "is:open sort:updated-desc"
    } else {
        query.trim()
    };
    Ok(format!(
        "repo:{} {kind} {user_query}",
        repository.full_name()
    ))
}

fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in query.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            current.push(character);
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
            current.push(character);
        } else if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

async fn discover_token() -> (Option<SecretString>, AuthSource) {
    for (name, source) in [
        ("GITHUB_TOKEN", AuthSource::GitHubToken),
        ("GH_TOKEN", AuthSource::GhToken),
    ] {
        if let Ok(token) = std::env::var(name) {
            let token = token.trim();
            if !token.is_empty() {
                return (Some(SecretString::from(token.to_string())), source);
            }
        }
    }

    let mut command = tokio::process::Command::new("gh");
    command.args(["auth", "token", "--hostname", "github.com"]);
    command.kill_on_drop(true);
    if let Ok(Ok(output)) = tokio::time::timeout(Duration::from_secs(3), command.output()).await
        && output.status.success()
        && let Ok(token) = String::from_utf8(output.stdout)
    {
        let token = token.trim();
        if !token.is_empty() {
            return (
                Some(SecretString::from(token.to_string())),
                AuthSource::GitHubCli,
            );
        }
    }
    (None, AuthSource::Anonymous)
}

async fn decode_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, GitHubError> {
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get("x-github-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.bytes().await.map_err(map_reqwest)?;
        let mut message = api_error_message(&bytes);
        if let Some(id) = request_id.as_deref() {
            message.push_str(&format!(" (GitHub request ID: {id})"));
        }
        return Err(GitHubError::Status {
            status,
            message,
            request_id,
        });
    }
    let bytes = response.bytes().await.map_err(map_reqwest)?;
    serde_json::from_slice(&bytes).map_err(|error| {
        GitHubError::Decode(format!(
            "{error}; response: {}",
            compact_response_excerpt(&bytes)
        ))
    })
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: String,
    #[serde(default)]
    documentation_url: Option<String>,
    #[serde(default)]
    errors: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CommitResponse {
    commit: CommitBody,
    author: Option<crate::models::User>,
    committer: Option<crate::models::User>,
}

#[derive(Deserialize)]
struct CommitBody {
    verification: CommitVerification,
}

#[derive(Deserialize)]
struct CommitVerification {
    verified: bool,
    reason: String,
}

fn map_reqwest(error: reqwest::Error) -> GitHubError {
    if error.is_timeout() {
        GitHubError::Timeout
    } else {
        GitHubError::Transport(error.to_string())
    }
}

fn generated_error<E: Debug>(error: generated::Error<E>) -> GitHubError {
    match error {
        generated::Error::RequestConstruction(error) => {
            GitHubError::Request(format!("generated request construction failed: {error:?}"))
        },
        generated::Error::Transport(error) => GitHubError::Transport(error.to_string()),
        generated::Error::Timeout(kind) => {
            GitHubError::Transport(format!("GitHub request timed out ({kind:?})"))
        },
        generated::Error::Protocol(error) => {
            GitHubError::Transport(format!("GitHub HTTP protocol error: {error:?}"))
        },
        generated::Error::Redirect(error) => {
            GitHubError::Transport(format!("GitHub redirect policy failed: {error:?}"))
        },
        generated::Error::Api(response) => {
            let status = response.status().as_u16();
            let request_id = github_request_id(response.headers());
            let mut message = format!("{:?}", response.inner());
            if let Some(id) = request_id.as_deref() {
                message.push_str(&format!(" (GitHub request ID: {id})"));
            }
            GitHubError::Status {
                status,
                message,
                request_id,
            }
        },
        generated::Error::UnexpectedStatus {
            status,
            headers,
            body,
        } => {
            let request_id = github_request_id(&headers);
            let mut message = api_error_message(&body);
            if let Some(id) = request_id.as_deref() {
                message.push_str(&format!(" (GitHub request ID: {id})"));
            }
            GitHubError::Status {
                status: status.as_u16(),
                message,
                request_id,
            }
        },
        generated::Error::Decode {
            path,
            body,
            truncated,
        } => {
            let suffix = if truncated {
                " (response was truncated)"
            } else {
                ""
            };
            GitHubError::Decode(format!(
                "field `{path}` could not be decoded{suffix}; response: {}",
                compact_response_excerpt(&body)
            ))
        },
        generated::Error::InterruptedBody(error) => {
            GitHubError::Transport(format!("GitHub response was interrupted: {error}"))
        },
    }
}

fn github_request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-github-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn api_error_message(bytes: &[u8]) -> String {
    serde_json::from_slice::<ApiErrorBody>(bytes)
        .map(|body| {
            let mut message = body.message;
            if let Some(errors) = body.errors {
                message.push_str(&format!(" Details: {errors}"));
            }
            if let Some(documentation) = body.documentation_url {
                message.push_str(&format!(" Documentation: {documentation}"));
            }
            message
        })
        .unwrap_or_else(|_| compact_response_excerpt(bytes))
}

fn compact_response_excerpt(bytes: &[u8]) -> String {
    const LIMIT: usize = 1_024;
    let excerpt = &bytes[..bytes.len().min(LIMIT)];
    let text = String::from_utf8_lossy(excerpt);
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if bytes.len() > LIMIT {
        format!("{compact}… ({} bytes total)", bytes.len())
    } else {
        compact
    }
}

fn next_page(current: u32, total: u64) -> Option<u32> {
    (u64::from(current) * u64::from(PER_PAGE) < total).then_some(current + 1)
}

#[cfg(test)]
fn parse_next_page(headers: &HeaderMap) -> Option<u32> {
    let link = headers.get("link")?.to_str().ok()?;
    for part in link.split(',') {
        if !part.contains("rel=\"next\"") {
            continue;
        }
        let url = part.trim().split(';').next()?.trim();
        let url = url.strip_prefix('<')?.strip_suffix('>')?;
        let url = reqwest::Url::parse(url).ok()?;
        return url
            .query_pairs()
            .find(|(key, _)| key == "page")
            .and_then(|(_, value)| value.parse().ok());
    }
    None
}

#[cfg(test)]
fn rate_limit(headers: &HeaderMap) -> RateLimit {
    RateLimit {
        limit: header_u64(headers, "x-ratelimit-limit"),
        remaining: header_u64(headers, "x-ratelimit-remaining"),
        reset: headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok()),
    }
}

#[cfg(test)]
fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}
