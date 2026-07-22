//! Local and remote branch lifecycle operations.

use std::ffi::OsStr;

use crate::Repository;
use crate::VcsError;

/// A branch that can be switched to.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BranchTarget {
    /// An existing local branch.
    Local(String),
    /// A remote-tracking branch, materialized as `local_name` before switching.
    Remote {
        /// The remote name.
        remote: String,
        /// The remote branch name without the remote prefix.
        branch: String,
        /// The local branch name to create.
        local_name: String,
    },
}

/// Options for creating a branch.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CreateBranchOptions {
    /// New local branch name.
    pub name: String,
    /// Revision from which the branch starts (`HEAD` by default).
    pub start_point: String,
    /// Switch to the new branch immediately.
    pub switch: bool,
    /// Publish to this remote after creation, if present.
    pub publish_remote: Option<String>,
    /// Configure the selected remote branch as upstream when publishing.
    pub set_upstream: bool,
}

impl Default for CreateBranchOptions {
    fn default() -> Self {
        Self {
            name: String::new(),
            start_point: "HEAD".to_string(),
            switch: true,
            publish_remote: None,
            set_upstream: true,
        }
    }
}

/// Outcome of undoing the last commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UndoCommitOutcome {
    /// Commit moved out of `HEAD`.
    pub commit: String,
    /// Whether that commit was already reachable from the configured upstream.
    pub was_upstream: bool,
}

impl Repository {
    /// Create a local branch, optionally switch to and publish it.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid/existing name, an unresolved start point,
    /// a failed switch, or a failed publication.
    pub fn create_branch(&self, options: &CreateBranchOptions) -> Result<(), VcsError> {
        self.validate_branch_name(&options.name)?;
        let verb = if options.switch { "switch" } else { "branch" };
        let create = if options.switch {
            "--create"
        } else {
            "--no-track"
        };
        self.git_checked([
            OsStr::new(verb),
            OsStr::new(create),
            OsStr::new(&options.name),
            OsStr::new(&options.start_point),
        ])?;
        if let Some(remote) = &options.publish_remote {
            self.publish_branch(remote, &options.name, options.set_upstream)?;
        }
        Ok(())
    }

    /// Switch to a local branch or create a tracking branch from a remote branch.
    ///
    /// # Errors
    /// Returns [`VcsError`] when the target is invalid or Git refuses the switch.
    pub fn switch_branch(&self, target: &BranchTarget) -> Result<(), VcsError> {
        match target {
            BranchTarget::Local(name) => {
                self.validate_branch_name(name)?;
                self.git_checked(["switch", name])?;
            },
            BranchTarget::Remote {
                remote,
                branch,
                local_name,
            } => {
                self.validate_remote(remote)?;
                self.validate_branch_name(branch)?;
                self.validate_branch_name(local_name)?;
                let upstream = format!("{remote}/{branch}");
                self.git_checked([
                    "switch",
                    "--create",
                    local_name,
                    "--track",
                    upstream.as_str(),
                ])?;
            },
        }
        Ok(())
    }

    /// Rename a local branch without changing its upstream.
    ///
    /// # Errors
    /// Returns [`VcsError`] when either name is invalid or the rename is refused.
    pub fn rename_branch(&self, old: &str, new: &str) -> Result<(), VcsError> {
        self.validate_branch_name(old)?;
        self.validate_branch_name(new)?;
        self.git_checked(["branch", "--move", old, new])?;
        Ok(())
    }

    /// Safely delete a fully-merged local branch. The current branch cannot be deleted.
    ///
    /// # Errors
    /// Returns [`VcsError`] when the name is invalid or Git refuses safe deletion.
    pub fn delete_branch(&self, name: &str) -> Result<(), VcsError> {
        self.validate_branch_name(name)?;
        if self.current_branch()?.as_deref() == Some(name) {
            return Err(VcsError::Git(
                "cannot delete the current branch".to_string(),
            ));
        }
        self.git_checked(["branch", "--delete", name])?;
        Ok(())
    }

    /// Publish a local branch to a remote, optionally setting its upstream.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid name or failed push.
    pub fn publish_branch(
        &self,
        remote: &str,
        branch: &str,
        set_upstream: bool,
    ) -> Result<(), VcsError> {
        self.validate_remote(remote)?;
        self.validate_branch_name(branch)?;
        let refspec = format!("{branch}:{branch}");
        if set_upstream {
            self.git_checked(["push", "--set-upstream", remote, refspec.as_str()])?;
        } else {
            self.git_checked(["push", remote, refspec.as_str()])?;
        }
        Ok(())
    }

    /// Delete a remote branch after refusing the remote's known default branch.
    ///
    /// # Errors
    /// Returns [`VcsError`] for invalid names, a default branch, or a failed push.
    pub fn delete_remote_branch(&self, remote: &str, branch: &str) -> Result<(), VcsError> {
        self.validate_remote(remote)?;
        self.validate_branch_name(branch)?;
        if self.remote_default_branch(remote)?.as_deref() == Some(branch) {
            return Err(VcsError::Git(
                "cannot delete the remote default branch".to_string(),
            ));
        }
        self.git_checked(["push", remote, "--delete", branch])?;
        Ok(())
    }

    /// Undo the last non-root, non-merge commit with a soft reset.
    ///
    /// When the commit is already reachable from its upstream, `allow_upstream` must
    /// be true. The worktree and index are otherwise left untouched.
    ///
    /// # Errors
    /// Returns [`VcsError`] for root/merge commits, a published commit without
    /// confirmation, or a failed reset.
    pub fn undo_commit(&self, allow_upstream: bool) -> Result<UndoCommitOutcome, VcsError> {
        let line = self.git_text(["rev-list", "--parents", "--max-count=1", "HEAD"])?;
        let ids: Vec<&str> = line.split_whitespace().collect();
        if ids.len() == 1 {
            return Err(VcsError::Git("cannot undo the root commit".to_string()));
        }
        if ids.len() > 2 {
            return Err(VcsError::Git("cannot undo a merge commit".to_string()));
        }
        let published = self.head_is_upstream()?;
        if published && !allow_upstream {
            return Err(VcsError::ConfirmationRequired(
                "the last commit is already present upstream".to_string(),
            ));
        }
        let commit = ids[0].to_string();
        self.git_checked(["reset", "--soft", "HEAD^"])?;
        Ok(UndoCommitOutcome {
            commit,
            was_upstream: published,
        })
    }

    fn validate_branch_name(&self, name: &str) -> Result<(), VcsError> {
        if name.is_empty()
            || !self
                .git_output(["check-ref-format", "--branch", name])?
                .status
                .success()
        {
            return Err(VcsError::Git(format!("invalid branch name: {name}")));
        }
        Ok(())
    }

    pub(crate) fn head_is_upstream(&self) -> Result<bool, VcsError> {
        if self.upstream_of_head()?.is_none() {
            return Ok(false);
        }
        let output = self.git_output(["merge-base", "--is-ancestor", "HEAD", "@{upstream}"])?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(VcsError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn create_switch_rename_and_safe_delete() -> Result<(), VcsError> {
        let repo = test_support::init("branches")?;
        test_support::commit(&repo, "one\n", "initial")?;
        let vcs = Repository::discover(&repo.0)?;
        vcs.create_branch(&CreateBranchOptions {
            name: "feature".to_string(),
            ..CreateBranchOptions::default()
        })?;
        assert_eq!(vcs.current_branch()?.as_deref(), Some("feature"));
        vcs.rename_branch("feature", "renamed")?;
        assert_eq!(vcs.current_branch()?.as_deref(), Some("renamed"));
        assert!(vcs.delete_branch("renamed").is_err());
        vcs.switch_branch(&BranchTarget::Local("main".to_string()))?;
        vcs.delete_branch("renamed")?;
        assert!(
            !vcs.branches()?
                .iter()
                .any(|branch| branch.name == "renamed")
        );
        Ok(())
    }

    #[test]
    fn publish_and_switch_remote_branch() -> Result<(), VcsError> {
        let repo = test_support::init("publish")?;
        let remote = test_support::bare_remote("publish")?;
        test_support::commit(&repo, "one\n", "initial")?;
        test_support::git(
            &repo.0,
            &["remote", "add", "origin", &remote.0.to_string_lossy()],
        )?;
        let vcs = Repository::discover(&repo.0)?;
        vcs.publish_branch("origin", "main", true)?;
        vcs.create_branch(&CreateBranchOptions {
            name: "topic".to_string(),
            switch: false,
            ..CreateBranchOptions::default()
        })?;
        vcs.publish_branch("origin", "topic", false)?;
        vcs.delete_branch("topic")?;
        test_support::git(&repo.0, &["fetch", "origin"])?;
        vcs.switch_branch(&BranchTarget::Remote {
            remote: "origin".to_string(),
            branch: "topic".to_string(),
            local_name: "topic".to_string(),
        })?;
        assert_eq!(vcs.current_branch()?.as_deref(), Some("topic"));
        Ok(())
    }

    #[test]
    fn undo_keeps_changes_staged_and_guards_root() -> Result<(), VcsError> {
        let repo = test_support::init("undo")?;
        test_support::commit(&repo, "one\n", "initial")?;
        let second = test_support::commit(&repo, "two\n", "second")?;
        let vcs = Repository::discover(&repo.0)?;
        let outcome = vcs.undo_commit(false)?;
        assert_eq!(outcome.commit, second);
        assert!(!outcome.was_upstream);
        assert_eq!(vcs.changes(crate::Selection::Staged, None)?.len(), 1);
        assert!(vcs.undo_commit(false).is_err());
        Ok(())
    }

    #[test]
    fn defaults_are_safe_and_names_are_validated() -> Result<(), VcsError> {
        let options = CreateBranchOptions::default();
        assert_eq!(options.start_point, "HEAD");
        assert!(options.switch);
        assert!(options.set_upstream);
        let repo = test_support::init("invalid-branch")?;
        test_support::commit(&repo, "one\n", "initial")?;
        let vcs = Repository::discover(&repo.0)?;
        assert!(
            vcs.create_branch(&CreateBranchOptions {
                name: "bad name".to_string(),
                ..options
            })
            .is_err()
        );
        Ok(())
    }
}
