//! Remotes, repository operation state, synchronization, and recovery.

use crate::Repository;
use crate::VcsError;

/// A configured Git remote.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Remote {
    /// Remote name.
    pub name: String,
    /// Fetch URL, when configured.
    pub url: Option<String>,
}

/// A remote-tracking branch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoteBranch {
    /// Remote name.
    pub remote: String,
    /// Branch name without the remote prefix.
    pub name: String,
    /// Whether the remote advertises this as its default branch locally.
    pub is_default: bool,
}

/// An in-progress operation recorded by Git metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RepositoryOperation {
    /// A merge with unresolved or uncommitted results.
    Merge,
    /// A rebase in progress.
    Rebase,
    /// A cherry-pick in progress.
    CherryPick,
}

/// Current branch/upstream state for the Source Control header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RepositoryState {
    /// Current local branch, or `None` for detached `HEAD`.
    pub branch: Option<String>,
    /// Live configured upstream short name.
    pub upstream: Option<String>,
    /// Commits in local `HEAD` but not upstream.
    pub ahead: usize,
    /// Commits upstream but not in local `HEAD`.
    pub behind: usize,
    /// In-progress Git operation, if any.
    pub operation: Option<RepositoryOperation>,
    /// GitHub pull request number for a managed `pr/<number>` branch.
    pub pull_request: Option<u64>,
}

/// Result of synchronizing the current branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SyncOutcome {
    /// Pull and push completed.
    Synced,
    /// The current branch has no upstream and must be published first.
    NeedsPublish,
    /// A managed pull-request branch was fetched and fast-forwarded without pushing.
    PullRequestUpdated,
}

/// A git forge kind, detected from a remote's host name by [`detect_forge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ForgeKind {
    /// The public `github.com` service.
    GitHub,
    /// A GitLab host (`gitlab.com` or a self-managed host named `*gitlab*`).
    GitLab,
    /// A Gitea host (named `*gitea*`).
    Gitea,
    /// A Forgejo host (`codeberg.org` or a host named `*forgejo*`).
    Forgejo,
    /// A host none of the rules above recognize.
    Unknown,
}

impl ForgeKind {
    /// The human-readable forge name, for notes and status messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Forgejo => "Forgejo",
            Self::Unknown => "an unknown host",
        }
    }
}

/// A parsed git remote URL: the host, the repository path on it, and the forge
/// that host runs.
///
/// Distinct from [`Remote`], which is a configured remote's name and raw URL;
/// this is what [`parse_remote`] makes of such a URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RemoteUrl {
    /// The lowercased host name, without user or port.
    pub host: String,
    /// The repository path on the host, `/`-separated with no empty segments and
    /// one trailing `.git` stripped: `owner/repo`, or a deeper path where the
    /// forge has nested groups (GitLab subgroups). A [`ForgeKind::GitHub`]
    /// remote always has exactly `owner/repo`.
    pub repo_path: String,
    /// The forge detected from the host name.
    pub kind: ForgeKind,
}

/// Parse a git remote URL — scp-like `user@host:owner/repo.git`, `ssh://`,
/// `git://`, `http://`, or `https://` — into a [`RemoteUrl`].
///
/// The user and port never take part in the result, and the host is matched
/// case-insensitively. Returns `None` for anything that does not name a
/// repository on a host: a local path, another scheme, a query string or
/// fragment, a path with fewer than two segments, or a `github.com` path other
/// than exactly `owner/repo` (GitHub has no nested groups).
#[must_use]
pub fn parse_remote(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (authority, path) = if let Some(rest) = ["ssh://", "git://", "http://", "https://"]
        .iter()
        .find_map(|scheme| url.strip_prefix(scheme))
    {
        rest.split_once('/')?
    } else if !url.contains("://") && url.contains('@') && url.contains(':') {
        // scp-like: the first colon separates `user@host` from the path.
        url.split_once(':')?
    } else {
        return None;
    };
    if path.contains(['?', '#']) {
        return None;
    }
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host
        .split_once(':')
        .map_or(host, |(host, _)| host)
        .to_ascii_lowercase();
    let mut segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if let Some(last) = segments.last_mut() {
        *last = last.strip_suffix(".git").unwrap_or(last);
    }
    if host.is_empty() || segments.len() < 2 || segments.iter().any(|part| part.is_empty()) {
        return None;
    }
    let kind = detect_forge(&host);
    if kind == ForgeKind::GitHub && segments.len() != 2 {
        return None;
    }
    Some(RemoteUrl {
        repo_path: segments.join("/"),
        host,
        kind,
    })
}

/// Detect the forge a host runs from its name (matched case-insensitively).
#[must_use]
pub fn detect_forge(host: &str) -> ForgeKind {
    let host = host.to_ascii_lowercase();
    if host == "github.com" {
        ForgeKind::GitHub
    } else if host.contains("gitlab") {
        ForgeKind::GitLab
    } else if host == "codeberg.org" || host.contains("forgejo") {
        ForgeKind::Forgejo
    } else if host.contains("gitea") {
        ForgeKind::Gitea
    } else {
        ForgeKind::Unknown
    }
}

impl Repository {
    /// Read current branch, upstream divergence, and in-progress operation state.
    ///
    /// # Errors
    /// Returns [`VcsError`] when Git cannot resolve the repository state.
    pub fn repository_state(&self) -> Result<RepositoryState, VcsError> {
        let branch = self.current_branch()?;
        let upstream = self.upstream_of_head()?;
        let (ahead, behind) = if upstream.is_some() {
            parse_counts(&self.git_text([
                "rev-list",
                "--left-right",
                "--count",
                "HEAD...@{upstream}",
            ])?)?
        } else {
            (0, 0)
        };
        let pull_request = branch
            .as_deref()
            .and_then(|name| self.managed_pull_request(name));
        Ok(RepositoryState {
            branch,
            upstream,
            ahead,
            behind,
            operation: self.repository_operation(),
            pull_request,
        })
    }

    /// List configured remotes, sorted by name.
    ///
    /// # Errors
    /// Returns [`VcsError`] when Git cannot read remotes.
    pub fn remotes(&self) -> Result<Vec<Remote>, VcsError> {
        let mut remotes = Vec::new();
        for name in self
            .git_text(["remote"])?
            .lines()
            .filter(|row| !row.is_empty())
        {
            let url = self
                .git_output(["remote", "get-url", name])
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
            remotes.push(Remote {
                name: name.to_string(),
                url,
            });
        }
        remotes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(remotes)
    }

    /// List remote-tracking branches, excluding symbolic `HEAD` rows.
    ///
    /// # Errors
    /// Returns [`VcsError`] when references cannot be read.
    pub fn remote_branches(&self) -> Result<Vec<RemoteBranch>, VcsError> {
        let text = self.git_text(["for-each-ref", "--format=%(refname:short)", "refs/remotes"])?;
        let mut branches = Vec::new();
        for row in text.lines() {
            let Some((remote, name)) = row.split_once('/') else {
                continue;
            };
            if name == "HEAD" {
                continue;
            }
            let default = self.remote_default_branch(remote)?;
            branches.push(RemoteBranch {
                remote: remote.to_string(),
                name: name.to_string(),
                is_default: default.as_deref() == Some(name),
            });
        }
        branches.sort_by(|a, b| (&a.remote, &a.name).cmp(&(&b.remote, &b.name)));
        Ok(branches)
    }

    /// Fetch and prune one remote.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an unknown remote or failed fetch.
    pub fn fetch(&self, remote: &str) -> Result<(), VcsError> {
        self.validate_remote(remote)?;
        self.git_checked(["fetch", "--prune", remote])?;
        Ok(())
    }

    /// Pull using repository configuration and then push the current branch.
    ///
    /// # Errors
    /// Returns [`VcsError`] when pull or push fails.
    pub fn sync(&self) -> Result<SyncOutcome, VcsError> {
        let state = self.repository_state()?;
        if let Some(number) = state.pull_request {
            let branch = state.branch.as_deref().ok_or_else(|| {
                VcsError::Git("managed pull-request branch is detached".to_string())
            })?;
            let remote = self.managed_pull_remote(branch).ok_or_else(|| {
                VcsError::Git("managed pull-request branch has no remote".to_string())
            })?;
            self.checkout_github_pull_request(&remote, number)?;
            return Ok(SyncOutcome::PullRequestUpdated);
        }
        if state.upstream.is_none() {
            return Ok(SyncOutcome::NeedsPublish);
        }
        self.git_checked(["pull", "--no-edit"])?;
        self.git_checked(["push"])?;
        Ok(SyncOutcome::Synced)
    }

    /// Fetch a GitHub pull-request head and switch to the reusable `pr/<number>`
    /// local branch. Existing branches are updated by fast-forward only.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an unknown remote, missing PR ref, dirty switch, or
    /// divergent local PR branch. Existing local commits are never reset.
    pub fn checkout_github_pull_request(
        &self,
        remote: &str,
        number: u64,
    ) -> Result<String, VcsError> {
        self.validate_remote(remote)?;
        if number == 0 {
            return Err(VcsError::Git(
                "pull request number must be positive".to_string(),
            ));
        }
        let local = format!("pr/{number}");
        let tracking = format!("{remote}/pull/{number}");
        let refspec = format!("+refs/pull/{number}/head:refs/remotes/{remote}/pull/{number}");
        self.git_checked(["fetch", remote, refspec.as_str()])?;
        let exists = self
            .git_output([
                "show-ref",
                "--verify",
                "--quiet",
                format!("refs/heads/{local}").as_str(),
            ])?
            .status
            .success();
        if exists {
            self.git_checked(["switch", local.as_str()])?;
            self.git_checked(["merge", "--ff-only", tracking.as_str()])?;
        } else {
            self.git_checked([
                "switch",
                "--create",
                local.as_str(),
                "--track",
                tracking.as_str(),
            ])?;
        }
        let number_text = number.to_string();
        let number_key = format!("branch.{local}.karetPullRequest");
        self.git_checked(["config", number_key.as_str(), number_text.as_str()])?;
        let remote_key = format!("branch.{local}.karetPullRemote");
        self.git_checked(["config", remote_key.as_str(), remote])?;
        Ok(local)
    }

    /// Continue the detected merge, rebase, or cherry-pick.
    ///
    /// # Errors
    /// Returns [`VcsError`] when no operation exists or continuation fails.
    pub fn continue_operation(&self) -> Result<(), VcsError> {
        match self.repository_operation() {
            Some(RepositoryOperation::Merge) => self.git_checked(["merge", "--continue"]),
            Some(RepositoryOperation::Rebase) => self.git_checked(["rebase", "--continue"]),
            Some(RepositoryOperation::CherryPick) => {
                self.git_checked(["cherry-pick", "--continue"])
            },
            None => return Err(VcsError::Git("no Git operation is in progress".to_string())),
        }?;
        Ok(())
    }

    /// Abort the detected merge, rebase, or cherry-pick.
    ///
    /// # Errors
    /// Returns [`VcsError`] when no operation exists or aborting fails.
    pub fn abort_operation(&self) -> Result<(), VcsError> {
        match self.repository_operation() {
            Some(RepositoryOperation::Merge) => self.git_checked(["merge", "--abort"]),
            Some(RepositoryOperation::Rebase) => self.git_checked(["rebase", "--abort"]),
            Some(RepositoryOperation::CherryPick) => self.git_checked(["cherry-pick", "--abort"]),
            None => return Err(VcsError::Git("no Git operation is in progress".to_string())),
        }?;
        Ok(())
    }

    /// Skip the current rebase or cherry-pick step.
    ///
    /// # Errors
    /// Returns [`VcsError`] for a merge/no operation or a failed skip.
    pub fn skip_operation(&self) -> Result<(), VcsError> {
        match self.repository_operation() {
            Some(RepositoryOperation::Rebase) => self.git_checked(["rebase", "--skip"]),
            Some(RepositoryOperation::CherryPick) => self.git_checked(["cherry-pick", "--skip"]),
            Some(RepositoryOperation::Merge) => {
                return Err(VcsError::Git("a merge cannot skip a step".to_string()));
            },
            None => return Err(VcsError::Git("no Git operation is in progress".to_string())),
        }?;
        Ok(())
    }

    pub(crate) fn validate_remote(&self, remote: &str) -> Result<(), VcsError> {
        if remote.is_empty() || !self.remotes()?.iter().any(|item| item.name == remote) {
            return Err(VcsError::Git(format!("unknown remote: {remote}")));
        }
        Ok(())
    }

    pub(crate) fn remote_default_branch(&self, remote: &str) -> Result<Option<String>, VcsError> {
        let reference = format!("refs/remotes/{remote}/HEAD");
        let output = self.git_output(["symbolic-ref", "--quiet", "--short", reference.as_str()])?;
        if !output.status.success() {
            return Ok(None);
        }
        let value = String::from_utf8_lossy(&output.stdout);
        Ok(value
            .trim()
            .strip_prefix(&format!("{remote}/"))
            .map(str::to_string))
    }

    fn repository_operation(&self) -> Option<RepositoryOperation> {
        let git = self.inner.git_dir();
        let common = self.inner.common_dir();
        if git.join("rebase-merge").exists() || git.join("rebase-apply").exists() {
            Some(RepositoryOperation::Rebase)
        } else if git.join("CHERRY_PICK_HEAD").exists() {
            Some(RepositoryOperation::CherryPick)
        } else if git.join("MERGE_HEAD").exists() || common.join("MERGE_HEAD").exists() {
            Some(RepositoryOperation::Merge)
        } else {
            None
        }
    }

    fn managed_pull_request(&self, branch: &str) -> Option<u64> {
        let key = format!("branch.{branch}.karetPullRequest");
        let output = self.git_output(["config", "--get", key.as_str()]).ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }

    fn managed_pull_remote(&self, branch: &str) -> Option<String> {
        let key = format!("branch.{branch}.karetPullRemote");
        let output = self.git_output(["config", "--get", key.as_str()]).ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn parse_counts(text: &str) -> Result<(usize, usize), VcsError> {
    let mut fields = text.split_whitespace();
    let ahead = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| VcsError::Git("invalid ahead/behind response".to_string()))?;
    let behind = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| VcsError::Git("invalid ahead/behind response".to_string()))?;
    Ok((ahead, behind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    fn parsed(url: &str) -> Option<(String, String, ForgeKind)> {
        parse_remote(url).map(|remote| (remote.host, remote.repo_path, remote.kind))
    }

    fn owned(host: &str, path: &str, kind: ForgeKind) -> Option<(String, String, ForgeKind)> {
        Some((host.to_string(), path.to_string(), kind))
    }

    #[test]
    fn parses_every_clone_form_to_the_same_remote() {
        let want = owned("github.com", "getkono/karet", ForgeKind::GitHub);
        for url in [
            "git@github.com:getkono/karet.git",
            "https://github.com/getkono/karet.git",
            "https://github.com/getkono/karet",
            "http://github.com/getkono/karet/",
            "git://github.com/getkono/karet.git",
            "ssh://git@github.com:22/getkono/karet.git",
            "https://token@github.com:443/getkono/karet.git",
            "https://github.com//getkono//karet.git/",
            "  git@GitHub.COM:getkono/karet  ",
        ] {
            assert_eq!(parsed(url), want, "{url}");
        }
    }

    #[test]
    fn the_ssh_user_is_not_part_of_the_remote() {
        // A non-`git` user names the same repository; the user only matters to
        // ssh, never to which forge or repository the URL points at.
        assert_eq!(
            parsed("alice@github.com:getkono/karet.git"),
            parsed("ssh://bob@github.com/getkono/karet.git"),
        );
        assert_eq!(
            parsed("alice@github.com:getkono/karet.git"),
            owned("github.com", "getkono/karet", ForgeKind::GitHub),
        );
    }

    #[test]
    fn strips_one_trailing_dot_git_and_keeps_inner_dots() {
        assert_eq!(
            parsed("git@github.com:owner/repo.name.git"),
            owned("github.com", "owner/repo.name", ForgeKind::GitHub),
        );
        assert_eq!(
            parsed("https://gitlab.com/g/r.git.git"),
            owned("gitlab.com", "g/r.git", ForgeKind::GitLab),
        );
    }

    #[test]
    fn keeps_gitlab_subgroups_but_rejects_nested_github_paths() {
        assert_eq!(
            parsed("https://gitlab.com/group/subgroup/repo.git"),
            owned("gitlab.com", "group/subgroup/repo", ForgeKind::GitLab),
        );
        assert_eq!(parsed("https://github.com/owner/repo/extra"), None);
    }

    #[test]
    fn detects_each_forge_from_the_host() {
        assert_eq!(detect_forge("github.com"), ForgeKind::GitHub);
        assert_eq!(detect_forge("GitHub.com"), ForgeKind::GitHub);
        assert_eq!(detect_forge("gitlab.example.com"), ForgeKind::GitLab);
        assert_eq!(detect_forge("codeberg.org"), ForgeKind::Forgejo);
        assert_eq!(detect_forge("forgejo.example.com"), ForgeKind::Forgejo);
        assert_eq!(detect_forge("gitea.example.com"), ForgeKind::Gitea);
        // Lookalikes and enterprise hosts are not github.com.
        for host in [
            "www.github.com",
            "github.com.evil.test",
            "github.example.com",
        ] {
            assert_eq!(detect_forge(host), ForgeKind::Unknown, "{host}");
        }
        assert_eq!(ForgeKind::Forgejo.name(), "Forgejo");
    }

    #[test]
    fn rejects_urls_that_name_no_repository() {
        for url in [
            "",
            "/srv/git/repo.git",
            "file:///srv/git/repo.git",
            "ftp://github.com/o/r.git",
            "https://github.com/",
            "https://github.com/onlyowner",
            "git@github.com:repo.git",
            "https://github.com/o/.git",
            "https://github.com/o/r?tab=readme",
            "https://gitlab.com/g/r#readme",
            "https:///o/r",
        ] {
            assert_eq!(parsed(url), None, "accepted {url:?}");
        }
    }

    #[test]
    fn state_and_remote_inventory_track_upstream() -> Result<(), VcsError> {
        let repo = test_support::init("state")?;
        let remote = test_support::bare_remote("state")?;
        test_support::commit(&repo, "one\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        test_support::git(&repo.0, &["push", "-q", "-u", "origin", "main"])?;
        test_support::git(
            &repo.0,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )?;
        let vcs = Repository::discover(&repo.0)?;
        assert_eq!(vcs.remotes()?.len(), 1);
        let branches = vcs.remote_branches()?;
        assert_eq!(branches.len(), 1);
        assert!(branches[0].is_default);
        let state = vcs.repository_state()?;
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.upstream.as_deref(), Some("origin/main"));
        assert_eq!((state.ahead, state.behind), (0, 0));
        assert_eq!(state.pull_request, None);
        assert_eq!(vcs.sync()?, SyncOutcome::Synced);
        Ok(())
    }

    #[test]
    fn sync_without_upstream_requests_publication() -> Result<(), VcsError> {
        let repo = test_support::init("needs-publish")?;
        test_support::commit(&repo, "one\n", "initial")?;
        let vcs = Repository::discover(&repo.0)?;
        assert_eq!(vcs.sync()?, SyncOutcome::NeedsPublish);
        Ok(())
    }

    #[test]
    fn merge_conflict_is_detected_and_abortable() -> Result<(), VcsError> {
        let repo = test_support::init("operation")?;
        test_support::commit(&repo, "base\n", "initial")?;
        test_support::git(&repo.0, &["switch", "-q", "-c", "side"])?;
        test_support::commit(&repo, "side\n", "side")?;
        test_support::git(&repo.0, &["switch", "-q", "main"])?;
        test_support::commit(&repo, "main\n", "main")?;
        let merge = std::process::Command::new("git")
            .args(["merge", "side"])
            .current_dir(&repo.0)
            .output()
            .map_err(|error| VcsError::Git(error.to_string()))?;
        assert!(!merge.status.success());
        let vcs = Repository::discover(&repo.0)?;
        assert_eq!(
            vcs.repository_state()?.operation,
            Some(RepositoryOperation::Merge)
        );
        assert!(vcs.skip_operation().is_err());
        vcs.abort_operation()?;
        assert_eq!(vcs.repository_state()?.operation, None);
        Ok(())
    }

    #[test]
    fn count_parser_rejects_bad_output() {
        assert_eq!(parse_counts("2 3").unwrap_or_default(), (2, 3));
        assert!(parse_counts("bad").is_err());
    }

    #[test]
    fn github_pull_ref_checks_out_and_updates_without_push() -> Result<(), VcsError> {
        let repo = test_support::init("pull-request")?;
        let remote = test_support::bare_remote("pull-request")?;
        let first = test_support::commit(&repo, "one\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        test_support::git(&repo.0, &["push", "-q", "origin", "HEAD:refs/pull/7/head"])?;
        assert!(!first.is_empty());
        let vcs = Repository::discover(&repo.0)?;
        assert_eq!(vcs.checkout_github_pull_request("origin", 7)?, "pr/7");
        let state = vcs.repository_state()?;
        assert_eq!(state.pull_request, Some(7));
        assert_eq!(vcs.sync()?, SyncOutcome::PullRequestUpdated);
        assert!(vcs.checkout_github_pull_request("origin", 0).is_err());
        Ok(())
    }
}
