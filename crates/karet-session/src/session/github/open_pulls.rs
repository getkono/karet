//! The Source Control branch picker's open-pull-request listing.
//!
//! Unlike the rest of the GitHub view it names its own remote, so it is served
//! by the GitHub worker's client when one is running and by a client discovered
//! for the request otherwise; it never depends on the worker's eligibility.

use std::path::PathBuf;

use karet_github::GitHubClient;
use karet_github::GitHubRemote;
use karet_github::RepositoryIdentity;
use karet_vcs::Repository;

use super::Session;
use super::map_pull_request;
use crate::api::Event;
use crate::api::GithubOperation;
use crate::api::GithubPage;
use crate::api::RequestId;

impl Session {
    /// Serve the branch picker's open-pull-request listing without the GitHub
    /// worker. The listing names its own remote, so it needs neither the
    /// worker's `origin` scope nor its eligibility rule, and it works in every
    /// workspace whose chosen remote is on github.com.
    pub(super) fn open_pull_requests_without_worker(
        &self,
        id: RequestId,
        root: Option<PathBuf>,
        remote: String,
        page: u32,
    ) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.emit(
                Some(id),
                pull_request_listing_failed(
                    "this session has no async runtime to reach GitHub on".to_string(),
                ),
            );
            return;
        }
        let events = self.events.clone();
        tokio::spawn(async move {
            let event = open_pull_requests_without_worker(root, remote, page).await;
            events.emit(Some(id), event);
        });
    }
}

/// One page of the open pull requests of the repository `remote` names, read
/// with the worker's authenticated client. A failure is answered as a
/// [`GithubOperation::PullRequests`] error carrying the bare reason.
pub(super) async fn open_pull_requests(
    client: &GitHubClient,
    root: Option<PathBuf>,
    remote: String,
    page: u32,
) -> Event {
    match resolve_remote(root, remote).await {
        Ok(repository) => list_open_pull_requests(client, &repository, page).await,
        Err(message) => pull_request_listing_failed(message),
    }
}

/// [`open_pull_requests`] for a session with no GitHub worker (the workspace
/// root is not the worktree root, or `origin` is not on github.com): the remote
/// is resolved first, so an unusable one costs no credential lookup, then a
/// client is discovered for this one page.
async fn open_pull_requests_without_worker(
    root: Option<PathBuf>,
    remote: String,
    page: u32,
) -> Event {
    let repository = match resolve_remote(root, remote).await {
        Ok(repository) => repository,
        Err(message) => return pull_request_listing_failed(message),
    };
    match GitHubClient::discover().await {
        Ok(client) => list_open_pull_requests(&client, &repository, page).await,
        Err(error) => pull_request_listing_failed(error.to_string()),
    }
}

/// [`remote_repository`] off the async thread: `git remote get-url` is a
/// subprocess.
async fn resolve_remote(
    root: Option<PathBuf>,
    remote: String,
) -> Result<RepositoryIdentity, String> {
    tokio::task::spawn_blocking(move || remote_repository(root, &remote))
        .await
        .map_err(|error| error.to_string())?
}

async fn list_open_pull_requests(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
    page: u32,
) -> Event {
    match client.open_pull_requests(repository, page).await {
        Ok(page) => Event::GithubPullRequests {
            page: GithubPage {
                items: page.items.into_iter().map(map_pull_request).collect(),
                page: page.page,
                next_page: page.next_page,
                total_count: page.total_count,
            },
        },
        Err(error) => pull_request_listing_failed(error.to_string()),
    }
}

fn pull_request_listing_failed(message: String) -> Event {
    Event::GithubError {
        operation: GithubOperation::PullRequests,
        message,
    }
}

/// The github.com repository the configured remote `name` points at, in the
/// repository whose worktree is `root`; the `Err` side is a user-facing reason.
fn remote_repository(root: Option<PathBuf>, name: &str) -> Result<RepositoryIdentity, String> {
    let root = root.ok_or_else(|| "no repository is open".to_string())?;
    let repository = Repository::discover(&root).map_err(|error| error.to_string())?;
    let url = repository
        .remotes()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|remote| remote.name == name)
        .ok_or_else(|| format!("unknown remote: {name}"))?
        .url
        .ok_or_else(|| format!("remote {name} has no fetch URL"))?;
    GitHubRemote::parse(&url)
        .map(GitHubRemote::into_identity)
        .ok_or_else(|| format!("remote {name} is not hosted on GitHub"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;
    use crate::api::GithubCommand;

    /// The open-pull-request listing reads whichever remote it is given, not only
    /// `origin`, and refuses one it cannot place on github.com.
    #[test]
    fn remote_repository_resolves_any_named_github_remote() -> Result<(), String> {
        let dir = TempDir::new().map_err(|error| error.to_string())?;
        let git = |args: &[&str]| -> Result<(), String> {
            let status = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .map_err(|error| error.to_string())?;
            status
                .success()
                .then_some(())
                .ok_or_else(|| format!("git {args:?} failed"))
        };
        git(&["init", "-q"])?;
        git(&["remote", "add", "origin", "https://gitlab.com/me/karet.git"])?;
        git(&[
            "remote",
            "add",
            "upstream",
            "git@github.com:getkono/karet.git",
        ])?;
        let root = Some(dir.path().to_path_buf());

        let upstream = remote_repository(root.clone(), "upstream")?;
        assert_eq!(upstream.full_name(), "getkono/karet");
        let refusal = |name| remote_repository(root.clone(), name).map(|r| r.full_name());
        assert_eq!(
            refusal("origin"),
            Err("remote origin is not hosted on GitHub".to_string())
        );
        assert_eq!(
            refusal("missing"),
            Err("unknown remote: missing".to_string())
        );
        assert_eq!(
            remote_repository(None, "origin").map(|r| r.full_name()),
            Err("no repository is open".to_string())
        );
        Ok(())
    }

    /// Without a GitHub worker the listing still answers on its own id, and an
    /// unusable remote is refused before any credential lookup or request.
    #[tokio::test]
    async fn the_listing_answers_without_a_worker() {
        assert!(matches!(
            open_pull_requests_without_worker(None, "origin".to_string(), 1).await,
            Event::GithubError { operation: GithubOperation::PullRequests, message }
                if message == "no repository is open"
        ));

        let (mut session, mut events, _snapshots) =
            Session::new(crate::session::SessionConfig::default());
        session.handle_github(
            RequestId(7),
            GithubCommand::ListOpenPullRequests {
                remote: "origin".to_string(),
                page: 1,
            },
        );
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv()).await;
        assert!(
            matches!(
                &answer,
                Ok(Some((
                    Some(RequestId(7)),
                    Event::GithubError {
                        operation: GithubOperation::PullRequests,
                        ..
                    }
                )))
            ),
            "{answer:?}"
        );
    }
}
