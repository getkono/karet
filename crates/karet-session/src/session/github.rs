//! GitHub eligibility and asynchronous request worker.

use std::path::Path;
use std::path::PathBuf;

use karet_github::GitHubClient;
use karet_github::GitHubRemote;
use karet_github::RepositoryIdentity;
use karet_vcs::Repository;
use tokio::sync::mpsc;

use super::Session;
use crate::api::Event;
use crate::api::GithubAuth;
use crate::api::GithubAuthSource;
use crate::api::GithubCheckRun;
use crate::api::GithubComment;
use crate::api::GithubIssue;
use crate::api::GithubLabel;
use crate::api::GithubNewIssue;
use crate::api::GithubNewPullRequest;
use crate::api::GithubPage;
use crate::api::GithubPullRequest;
use crate::api::GithubPullRequestActivity;
use crate::api::GithubPullRequestCommit;
use crate::api::GithubRepository;
use crate::api::GithubVerification;
use crate::api::GithubWorkflow;
use crate::api::GithubWorkflowRun;
use crate::api::RequestId;

pub(super) enum GithubJob {
    Refresh,
    Login {
        token: String,
    },
    Issues {
        query: String,
        page: u32,
    },
    PullRequests {
        query: String,
        page: u32,
    },
    Actions {
        page: u32,
    },
    Issue {
        number: u64,
    },
    PullRequest {
        number: u64,
    },
    UpdatePullRequestBody {
        number: u64,
        body: String,
    },
    CommentPullRequest {
        number: u64,
        body: String,
    },
    MergePullRequest {
        number: u64,
        head_sha: String,
    },
    SetPullRequestDraft {
        node_id: String,
        number: u64,
        draft: bool,
    },
    IssueMetadata,
    CreateIssue {
        issue: GithubNewIssue,
    },
    CreatePullRequest {
        pull_request: GithubNewPullRequest,
    },
    Verification {
        hash: String,
    },
}

/// Return the eligible public-GitHub identity only when the session root is the
/// exact canonical worktree root.
pub(super) fn eligible_repository(
    roots: &[PathBuf],
    repository: Option<&Repository>,
) -> Option<RepositoryIdentity> {
    let root = roots.first()?;
    let root = std::fs::canonicalize(root).ok()?;
    if root.parent().is_none() || root == Path::new("/") {
        return None;
    }
    let repository = repository?;
    if repository.worktree_root().as_deref() != Some(root.as_path()) {
        return None;
    }
    let remote = repository.origin_url()?;
    GitHubRemote::parse(&remote).map(GitHubRemote::into_identity)
}

impl Session {
    pub(super) fn refresh_github(&mut self, id: RequestId) {
        let next = eligible_repository(&self.config.roots, self.vcs.as_ref());
        if next != self.github_repository {
            self.github_repository = next;
            // Dropping the old sender lets its worker finish any in-flight request and
            // then exit. New commands can only reach the freshly scoped worker.
            self.github_tx = None;
            self.start_github();
        } else if next.is_none() {
            // Ineligibility is a normal availability result, not an operation
            // failure. In particular, `.git/config` watches can refresh a
            // non-GitHub repository repeatedly without producing notifications.
            self.emit(
                Some(id),
                Event::GithubAvailability {
                    repository: None,
                    auth: GithubAuth {
                        source: GithubAuthSource::Anonymous,
                        can_write: false,
                        viewer_id: None,
                        viewer_login: None,
                    },
                },
            );
        } else {
            self.send_github(id, GithubJob::Refresh);
        }
    }

    pub(super) fn start_github(&mut self) {
        let Some(repository) = self.github_repository.clone() else {
            self.emit(
                None,
                Event::GithubAvailability {
                    repository: None,
                    auth: GithubAuth {
                        source: GithubAuthSource::Anonymous,
                        can_write: false,
                        viewer_id: None,
                        viewer_login: None,
                    },
                },
            );
            return;
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        self.github_tx = Some(tx);
        let events = self.events.clone();
        tokio::spawn(async move {
            let mut client = match GitHubClient::discover().await {
                Ok(client) => client,
                Err(error) => {
                    let _ = events.send((
                        None,
                        Event::GithubError {
                            operation: "authentication".to_string(),
                            message: error.to_string(),
                        },
                    ));
                    return;
                },
            };
            let _ = events.send((
                None,
                Event::GithubAvailability {
                    repository: Some(map_repository(&repository)),
                    auth: map_auth(client.auth_state()),
                },
            ));
            while let Some((id, job)) = rx.recv().await {
                if let GithubJob::Login { token } = job {
                    match GitHubClient::authenticate(token).await {
                        Ok(authenticated) => {
                            client = authenticated;
                            let _ = events.send((
                                Some(id),
                                Event::GithubAvailability {
                                    repository: Some(map_repository(&repository)),
                                    auth: map_auth(client.auth_state()),
                                },
                            ));
                        },
                        Err(error) => {
                            let _ = events.send((
                                Some(id),
                                Event::GithubError {
                                    operation: "sign in".to_string(),
                                    message: error.to_string(),
                                },
                            ));
                        },
                    }
                    continue;
                }
                run_job(&client, &repository, id, job, &events).await;
            }
        });
    }

    pub(super) fn send_github(&self, id: RequestId, job: GithubJob) {
        if let Some(tx) = self.github_tx.as_ref() {
            let _ = tx.send((id, job));
        } else {
            self.emit(
                Some(id),
                Event::GithubError {
                    operation: "eligibility".to_string(),
                    message: "the workspace root is not an eligible github.com worktree"
                        .to_string(),
                },
            );
        }
    }
}

async fn run_job(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
    id: RequestId,
    job: GithubJob,
    events: &mpsc::UnboundedSender<(Option<RequestId>, Event)>,
) {
    let operation = operation_name(&job);
    let result = match job {
        GithubJob::Refresh => {
            let event = Event::GithubAvailability {
                repository: Some(map_repository(repository)),
                auth: map_auth(client.auth_state()),
            };
            Ok(event)
        },
        GithubJob::Login { .. } => Err(karet_github::GitHubError::Request(
            "sign-in request reached the ordinary GitHub job runner".to_string(),
        )),
        GithubJob::Issues { query, page } => client
            .search_issues(repository, &query, page)
            .await
            .map(|page| Event::GithubIssues {
                page: GithubPage {
                    items: page.items.into_iter().map(map_issue).collect(),
                    page: page.page,
                    next_page: page.next_page,
                    total_count: page.total_count,
                },
            }),
        GithubJob::PullRequests { query, page } => client
            .search_pull_requests(repository, &query, page)
            .await
            .map(|page| Event::GithubPullRequests {
                page: GithubPage {
                    items: page.items.into_iter().map(map_pull_request).collect(),
                    page: page.page,
                    next_page: page.next_page,
                    total_count: page.total_count,
                },
            }),
        GithubJob::Actions { page } => match client.workflows(repository, page).await {
            Ok(workflows) => {
                client
                    .workflow_runs(repository, page)
                    .await
                    .map(|runs| Event::GithubActions {
                        workflows: GithubPage {
                            items: workflows.items.into_iter().map(map_workflow).collect(),
                            page: workflows.page,
                            next_page: workflows.next_page,
                            total_count: workflows.total_count,
                        },
                        runs: GithubPage {
                            items: runs.items.into_iter().map(map_workflow_run).collect(),
                            page: runs.page,
                            next_page: runs.next_page,
                            total_count: runs.total_count,
                        },
                    })
            },
            Err(error) => Err(error),
        },
        GithubJob::Issue { number } => match client.issue(repository, number).await {
            Ok(detail) => load_all_comments(client, repository, number)
                .await
                .map(|comments| Event::GithubIssueReady {
                    issue: map_issue(detail.summary),
                    comments: complete_comment_page(comments),
                }),
            Err(error) => Err(error),
        },
        GithubJob::PullRequest { number } => load_pull_request(client, repository, number).await,
        GithubJob::UpdatePullRequestBody { number, body } => match client
            .update_pull_request_body(repository, number, body)
            .await
        {
            Ok(_) => load_pull_request(client, repository, number).await,
            Err(error) => Err(error),
        },
        GithubJob::CommentPullRequest { number, body } => match client
            .create_pull_request_comment(repository, number, body)
            .await
        {
            Ok(_) => load_pull_request(client, repository, number).await,
            Err(error) => Err(error),
        },
        GithubJob::MergePullRequest { number, head_sha } => match client
            .merge_pull_request(repository, number, head_sha)
            .await
        {
            Ok(()) => load_pull_request(client, repository, number).await,
            Err(error) => Err(error),
        },
        GithubJob::SetPullRequestDraft {
            node_id,
            number,
            draft,
        } => match client.set_pull_request_draft(node_id, draft).await {
            Ok(()) => load_pull_request(client, repository, number).await,
            Err(error) => Err(error),
        },
        GithubJob::IssueMetadata => client.issue_assignees(repository).await.map(|assignees| {
            Event::GithubIssueMetadataReady {
                assignees: assignees.into_iter().map(|user| user.login).collect(),
            }
        }),
        GithubJob::CreateIssue { issue } => {
            let request = karet_github::NewIssue {
                title: issue.title,
                body: issue.body,
                assignees: issue.assignees,
                labels: issue.labels,
                milestone: issue.milestone,
                r#type: issue.issue_type,
                issue_field_values: Vec::new(),
            };
            client
                .create_issue(repository, &request)
                .await
                .map(|detail| Event::GithubIssueReady {
                    issue: map_issue(detail.summary),
                    comments: GithubPage {
                        items: Vec::new(),
                        page: 1,
                        next_page: None,
                        total_count: Some(0),
                    },
                })
        },
        GithubJob::CreatePullRequest { pull_request } => {
            let request = karet_github::NewPullRequest {
                title: pull_request.title,
                head: pull_request.head,
                base: pull_request.base,
                body: pull_request.body,
                draft: pull_request.draft,
                maintainer_can_modify: pull_request.maintainer_can_modify,
            };
            match client.create_pull_request(repository, &request).await {
                Ok(pull_request) => {
                    load_pull_request(client, repository, pull_request.number).await
                },
                Err(error) => Err(error),
            }
        },
        GithubJob::Verification { hash } => client
            .commit_verification(repository, &hash)
            .await
            .map(|verification| Event::CommitVerification {
                hash,
                status: GithubVerification {
                    verified: verification.verified,
                    reason: verification.reason,
                    signer: verification.signer,
                },
            }),
    };
    let event = match result {
        Ok(event) => event,
        Err(error) => Event::GithubError {
            operation: operation.to_string(),
            message: error.to_string(),
        },
    };
    let _ = events.send((Some(id), event));
}

fn operation_name(job: &GithubJob) -> &'static str {
    match job {
        GithubJob::Refresh => "refresh",
        GithubJob::Login { .. } => "sign in",
        GithubJob::Issues { .. } => "issues",
        GithubJob::PullRequests { .. } => "pull requests",
        GithubJob::Actions { .. } => "actions",
        GithubJob::Issue { .. } => "issue detail",
        GithubJob::PullRequest { .. } => "pull request detail",
        GithubJob::UpdatePullRequestBody { .. } => "update pull request body",
        GithubJob::CommentPullRequest { .. } => "comment on pull request",
        GithubJob::MergePullRequest { .. } => "merge pull request",
        GithubJob::SetPullRequestDraft { .. } => "update pull request readiness",
        GithubJob::IssueMetadata => "issue metadata",
        GithubJob::CreateIssue { .. } => "create issue",
        GithubJob::CreatePullRequest { .. } => "create pull request",
        GithubJob::Verification { .. } => "commit verification",
    }
}

fn map_repository(repository: &RepositoryIdentity) -> GithubRepository {
    GithubRepository {
        owner: repository.owner.clone(),
        repo: repository.repo.clone(),
    }
}

fn map_auth(auth: karet_github::AuthState) -> GithubAuth {
    let source = match auth.source {
        karet_github::AuthSource::Anonymous => GithubAuthSource::Anonymous,
        karet_github::AuthSource::GitHubToken => GithubAuthSource::GithubToken,
        karet_github::AuthSource::GhToken => GithubAuthSource::GhToken,
        karet_github::AuthSource::GitHubCli => GithubAuthSource::GithubCli,
        karet_github::AuthSource::Explicit => GithubAuthSource::Explicit,
    };
    GithubAuth {
        source,
        can_write: auth.can_write,
        viewer_id: auth.viewer_id,
        viewer_login: auth.viewer_login,
    }
}

async fn load_pull_request(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
    number: u64,
) -> Result<Event, karet_github::GitHubError> {
    let pull_request = client.pull_request(repository, number).await?;
    let head_sha = pull_request.head_sha.clone();
    let (comments, commits, checks, activity) = tokio::join!(
        load_all_comments(client, repository, number),
        client.pull_request_commits(repository, number),
        client.check_runs(repository, &head_sha),
        client.pull_request_timeline(repository, number),
    );
    let (activity, activity_error) = match activity {
        Ok(activity) => (activity.into_iter().map(map_activity).collect(), None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    Ok(Event::GithubPullRequestReady {
        pull_request: map_pull_request(pull_request),
        comments: complete_comment_page(comments?),
        commits: commits?.into_iter().map(map_pull_request_commit).collect(),
        checks: checks?.into_iter().map(map_check_run).collect(),
        activity,
        activity_error,
    })
}

async fn load_all_comments(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
    number: u64,
) -> Result<Vec<karet_github::Comment>, karet_github::GitHubError> {
    let mut comments = Vec::new();
    let mut page = 1;
    loop {
        let response = client.issue_comments(repository, number, page).await?;
        comments.extend(response.items);
        let Some(next) = response.next_page else {
            return Ok(comments);
        };
        page = next;
    }
}

fn complete_comment_page(comments: Vec<karet_github::Comment>) -> GithubPage<GithubComment> {
    let total_count = u64::try_from(comments.len()).ok();
    GithubPage {
        items: comments.into_iter().map(map_comment).collect(),
        page: 1,
        next_page: None,
        total_count,
    }
}

fn map_issue(issue: karet_github::IssueSummary) -> GithubIssue {
    let blocked = issue.is_blocked();
    GithubIssue {
        number: issue.number,
        title: issue.title,
        body: issue.body,
        state: issue.state,
        creator: issue.user.as_ref().map(|user| user.login.clone()),
        creator_id: issue.user.as_ref().map(|user| user.id),
        created_unix: issue.created_at.unix_timestamp(),
        updated_unix: issue.updated_at.unix_timestamp(),
        labels: issue
            .labels
            .into_iter()
            .map(|label| GithubLabel {
                name: label.name,
                color: label.color,
            })
            .collect(),
        blocked,
        html_url: issue.html_url,
    }
}

fn map_pull_request(pull: karet_github::PullRequestSummary) -> GithubPullRequest {
    GithubPullRequest {
        number: pull.number,
        title: pull.title,
        body: pull.body,
        state: pull.state,
        creator: pull.creator,
        creator_id: pull.creator_id,
        created_unix: pull.created_at.unix_timestamp(),
        updated_unix: pull.updated_at.unix_timestamp(),
        labels: pull
            .labels
            .into_iter()
            .map(|label| GithubLabel {
                name: label.name,
                color: label.color,
            })
            .collect(),
        draft: pull.draft,
        node_id: pull.node_id,
        head_sha: pull.head_sha,
        base_sha: pull.base_sha,
        mergeable: pull.mergeable,
        merged: pull.merged,
        html_url: pull.html_url,
    }
}

fn map_comment(comment: karet_github::Comment) -> GithubComment {
    GithubComment {
        id: comment.id,
        creator: comment.user.map(|user| user.login),
        body: comment.body,
        created_unix: comment.created_at.unix_timestamp(),
        updated_unix: comment.updated_at.unix_timestamp(),
        html_url: comment.html_url,
    }
}

fn map_pull_request_commit(commit: karet_github::PullRequestCommit) -> GithubPullRequestCommit {
    GithubPullRequestCommit {
        sha: commit.sha,
        summary: commit.summary,
        author: commit.author,
        committed_unix: commit.committed_unix,
        parents: commit.parents,
        html_url: commit.html_url,
    }
}

fn map_check_run(check: karet_github::CheckRun) -> GithubCheckRun {
    GithubCheckRun {
        id: check.id,
        name: check.name,
        status: check.status,
        conclusion: check.conclusion,
        html_url: check.html_url,
    }
}

fn map_activity(activity: karet_github::TimelineEvent) -> GithubPullRequestActivity {
    GithubPullRequestActivity {
        id: activity.id,
        kind: activity.kind,
        actor: activity.actor,
        commit_id: activity.commit_id,
        before: activity.before,
        after: activity.after,
        created_unix: activity.created_unix,
    }
}

fn map_workflow(workflow: karet_github::Workflow) -> GithubWorkflow {
    GithubWorkflow {
        id: workflow.id,
        name: workflow.name,
        path: workflow.path,
        state: workflow.state,
        updated_unix: workflow.updated_at.unix_timestamp(),
    }
}

fn map_workflow_run(run: karet_github::WorkflowRun) -> GithubWorkflowRun {
    GithubWorkflowRun {
        id: run.id,
        workflow_id: run.workflow_id,
        title: run.title,
        branch: run.branch,
        head_sha: run.head_sha,
        event: run.event,
        status: run.status,
        conclusion: run.conclusion,
        actor: run.actor,
        run_number: run.run_number,
        created_unix: run.created_at.unix_timestamp(),
        html_url: run.html_url,
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn eligibility_requires_exact_worktree_root_and_public_github_origin() -> Result<(), String> {
        let dir = TempDir::new().map_err(|error| error.to_string())?;
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .map_err(|error| error.to_string())?;
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/getkono/karet.git",
            ])
            .current_dir(dir.path())
            .status()
            .map_err(|error| error.to_string())?;
        assert!(status.success());
        std::fs::create_dir(dir.path().join("nested")).map_err(|error| error.to_string())?;
        let repository = Repository::discover(dir.path()).map_err(|error| error.to_string())?;

        let identity = eligible_repository(&[dir.path().to_path_buf()], Some(&repository));
        assert_eq!(
            identity.map(|identity| identity.full_name()).as_deref(),
            Some("getkono/karet")
        );
        assert!(eligible_repository(&[dir.path().join("nested")], Some(&repository)).is_none());
        Ok(())
    }

    #[test]
    fn refreshing_an_ineligible_workspace_reports_normal_availability() -> Result<(), String> {
        let dir = TempDir::new().map_err(|error| error.to_string())?;
        let config = crate::session::SessionConfig {
            roots: vec![dir.path().to_path_buf()],
            ..crate::session::SessionConfig::default()
        };
        let (mut session, mut events, _snapshots) = Session::new(config);
        let id = RequestId(42);

        session.refresh_github(id);

        let Some((event_id, event)) = events.try_recv() else {
            return Err("refresh did not emit availability".to_string());
        };
        assert_eq!(event_id, Some(id));
        assert!(matches!(
            event,
            Event::GithubAvailability {
                repository: None,
                auth: GithubAuth {
                    source: GithubAuthSource::Anonymous,
                    can_write: false,
                    viewer_id: None,
                    viewer_login: None,
                },
            }
        ));
        Ok(())
    }
}
