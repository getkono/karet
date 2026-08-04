//! Stash creation, inspection, restoration, and lifecycle.

use crate::Repository;
use crate::VcsError;

/// Options for creating a stash.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StashOptions {
    /// Optional user-visible stash message.
    pub message: Option<String>,
    /// Include untracked files.
    pub include_untracked: bool,
    /// Leave staged changes in the index/worktree.
    pub keep_index: bool,
}

/// One stash entry, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashEntry {
    /// Stable selector such as `stash@{0}`.
    pub reference: String,
    /// Commit object id backing the stash.
    pub commit: String,
    /// Stash subject/message.
    pub message: String,
    /// Git's ISO-formatted creation date.
    pub date: String,
}

impl Repository {
    /// Create a stash. Returns `false` when there were no local changes to save.
    ///
    /// # Errors
    /// Returns [`VcsError`] when stash creation fails.
    pub fn stash_push(&self, options: &StashOptions) -> Result<bool, VcsError> {
        let before = self.stashes()?.first().map(|entry| entry.commit.clone());
        let mut args = vec!["stash", "push"];
        if options.include_untracked {
            args.push("--include-untracked");
        }
        if options.keep_index {
            args.push("--keep-index");
        }
        if let Some(message) = &options.message {
            args.push("--message");
            args.push(message);
        }
        self.git_checked(args)?;
        let after = self.stashes()?.first().map(|entry| entry.commit.clone());
        Ok(after.is_some() && after != before)
    }

    /// List stash entries from newest to oldest.
    ///
    /// # Errors
    /// Returns [`VcsError`] when the stash ref cannot be read.
    pub fn stashes(&self) -> Result<Vec<StashEntry>, VcsError> {
        let text = self.git_text(["stash", "list", "--format=%gd%x1f%H%x1f%gs%x1f%ci%x1e"])?;
        Ok(text
            .split('\u{1e}')
            .filter_map(|record| {
                let mut fields = record.trim().split('\u{1f}');
                Some(StashEntry {
                    reference: fields.next()?.to_string(),
                    commit: fields.next()?.to_string(),
                    message: fields.next()?.to_string(),
                    date: fields.next()?.to_string(),
                })
            })
            .collect())
    }

    /// Return a patch preview for one stash.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid selector or failed diff.
    pub fn stash_preview(&self, reference: &str) -> Result<String, VcsError> {
        self.validate_stash(reference)?;
        self.git_text(["stash", "show", "--patch", "--stat", reference])
    }

    /// Apply one stash without dropping it.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid selector or conflicts/failure.
    pub fn stash_apply(&self, reference: &str) -> Result<(), VcsError> {
        self.validate_stash(reference)?;
        self.git_checked(["stash", "apply", reference])?;
        Ok(())
    }

    /// Apply and drop one stash. A conflicted application keeps the stash.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid selector or conflicts/failure.
    pub fn stash_pop(&self, reference: &str) -> Result<(), VcsError> {
        self.validate_stash(reference)?;
        self.git_checked(["stash", "pop", reference])?;
        Ok(())
    }

    /// Drop one stash permanently.
    ///
    /// # Errors
    /// Returns [`VcsError`] for an invalid selector or failed deletion.
    pub fn stash_drop(&self, reference: &str) -> Result<(), VcsError> {
        self.validate_stash(reference)?;
        self.git_checked(["stash", "drop", reference])?;
        Ok(())
    }

    /// Create and switch to a branch from one stash, dropping it after a clean apply.
    ///
    /// # Errors
    /// Returns [`VcsError`] for invalid names/selectors or checkout conflicts.
    pub fn stash_branch(&self, name: &str, reference: &str) -> Result<(), VcsError> {
        self.validate_stash(reference)?;
        self.git_output(["check-ref-format", "--branch", name])?
            .status
            .success()
            .then_some(())
            .ok_or_else(|| VcsError::Git(format!("invalid branch name: {name}")))?;
        self.git_checked(["stash", "branch", name, reference])?;
        Ok(())
    }

    fn validate_stash(&self, reference: &str) -> Result<(), VcsError> {
        if self
            .stashes()?
            .iter()
            .any(|entry| entry.reference == reference)
        {
            Ok(())
        } else {
            Err(VcsError::Git(format!("unknown stash: {reference}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;

    #[test]
    fn stash_create_preview_apply_pop_and_drop() -> Result<(), VcsError> {
        let repo = test_support::init("stash")?;
        test_support::commit(&repo, "base\n", "initial")?;
        test_support::write(&repo.0, "file.txt", b"changed\n")?;
        test_support::write(&repo.0, "new.txt", b"new\n")?;
        let vcs = Repository::discover(&repo.0)?;
        assert!(vcs.stash_push(&StashOptions {
            message: Some("work".to_string()),
            include_untracked: true,
            keep_index: false,
        })?);
        let entries = vcs.stashes()?;
        assert_eq!(entries.len(), 1);
        assert!(entries[0].message.contains("work"));
        let reference = entries[0].reference.clone();
        assert!(vcs.stash_preview(&reference)?.contains("file.txt"));
        vcs.stash_apply(&reference)?;
        test_support::git(&repo.0, &["reset", "--hard", "-q"])?;
        let _ = std::fs::remove_file(repo.0.join("new.txt"));
        vcs.stash_pop(&reference)?;
        assert!(vcs.stashes()?.is_empty());

        test_support::git(&repo.0, &["reset", "--hard", "-q"])?;
        let _ = std::fs::remove_file(repo.0.join("new.txt"));
        test_support::write(&repo.0, "file.txt", b"again\n")?;
        assert!(vcs.stash_push(&StashOptions::default())?);
        let second = vcs.stashes()?[0].reference.clone();
        vcs.stash_drop(&second)?;
        assert!(vcs.stashes()?.is_empty());
        Ok(())
    }

    #[test]
    fn stash_branch_switches_and_consumes_entry() -> Result<(), VcsError> {
        let repo = test_support::init("stash-branch")?;
        test_support::commit(&repo, "base\n", "initial")?;
        test_support::write(&repo.0, "file.txt", b"topic\n")?;
        let vcs = Repository::discover(&repo.0)?;
        assert!(vcs.stash_push(&StashOptions::default())?);
        let reference = vcs.stashes()?[0].reference.clone();
        vcs.stash_branch("from-stash", &reference)?;
        assert_eq!(vcs.current_branch()?.as_deref(), Some("from-stash"));
        assert!(vcs.stashes()?.is_empty());
        Ok(())
    }

    #[test]
    fn empty_stash_is_reported_and_unknown_refs_fail() -> Result<(), VcsError> {
        let repo = test_support::init("empty-stash")?;
        test_support::commit(&repo, "base\n", "initial")?;
        let vcs = Repository::discover(&repo.0)?;
        assert!(!vcs.stash_push(&StashOptions::default())?);
        assert!(vcs.stash_apply("stash@{99}").is_err());
        Ok(())
    }
}
