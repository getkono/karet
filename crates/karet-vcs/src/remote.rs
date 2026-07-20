//! Remotes, repository operation state, synchronization, and recovery.

use std::ffi::OsStr;

use crate::Repository;
use crate::VcsError;

/// A configured Git remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    /// Remote name.
    pub name: String,
    /// Fetch URL, when configured.
    pub url: Option<String>,
}

/// A remote-tracking branch.
#[derive(Clone, Debug, PartialEq, Eq)]
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
pub struct RepositoryState {
    /// Current local branch, or `None` for detached `HEAD`.
    pub branch: Option<String>,
    /// Configured upstream short name.
    pub upstream: Option<String>,
    /// Commits in local `HEAD` but not upstream.
    pub ahead: usize,
    /// Commits upstream but not in local `HEAD`.
    pub behind: usize,
    /// In-progress Git operation, if any.
    pub operation: Option<RepositoryOperation>,
}

/// Result of synchronizing the current branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SyncOutcome {
    /// Pull and push completed.
    Synced,
    /// The current branch has no upstream and must be published first.
    NeedsPublish,
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
        Ok(RepositoryState {
            branch,
            upstream,
            ahead,
            behind,
            operation: self.repository_operation(),
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
        if self.upstream_of_head()?.is_none() {
            return Ok(SyncOutcome::NeedsPublish);
        }
        self.git_checked(["pull", "--no-edit"])?;
        self.git_checked(["push"])?;
        Ok(SyncOutcome::Synced)
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

    pub(crate) fn git_text<I, S>(&self, args: I) -> Result<String, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git_checked(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
}
