//! The `libgit2`-backed write actions (stage / unstage / discard / commit).
//!
//! Status and diffs are read with `gix`; only the mutating index/worktree
//! operations use `git2`, which has a complete, battle-tested staging API. The
//! whole module is gated behind the `git2` feature, and the public entry points in
//! [`crate::Repository`] return [`VcsError::FeatureDisabled`] when it is off.

use std::path::PathBuf;

use crate::Repository;
use crate::VcsError;
use crate::repo::to_git;

impl Repository {
    /// The working directory, or a [`VcsError::Git`] for a bare repository (which
    /// has no worktree to stage from).
    fn git2_workdir(&self) -> Result<PathBuf, VcsError> {
        self.git2
            .workdir()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| VcsError::Git("repository has no working directory".into()))
    }

    /// The `HEAD` commit as an [`git2::Object`], or `None` on an unborn branch (a
    /// repository with no commits yet).
    fn git2_head_commit(&self) -> Result<Option<git2::Object<'_>>, VcsError> {
        match self.git2.head() {
            Ok(head) => Ok(Some(head.peel(git2::ObjectType::Commit).map_err(to_git)?)),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(None),
            Err(e) => Err(to_git(e)),
        }
    }

    pub(crate) fn git2_stage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        let workdir = self.git2_workdir()?;
        let mut index = self.git2.index().map_err(to_git)?;
        for rel in paths {
            // A path still on disk is added from the worktree (an explicit add
            // bypasses gitignore); a vanished path is staged as a deletion.
            if workdir.join(rel).symlink_metadata().is_ok() {
                index.add_path(rel).map_err(to_git)?;
            } else {
                index.remove_path(rel).map_err(to_git)?;
            }
        }
        index.write().map_err(to_git)
    }

    pub(crate) fn git2_unstage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        let head = self.git2_head_commit()?;
        // With a target, entries reset to their HEAD state; with `None` (unborn
        // branch) libgit2 removes the matching entries from the index.
        self.git2
            .reset_default(head.as_ref(), paths.iter())
            .map_err(to_git)
    }

    pub(crate) fn git2_stage_all(&self) -> Result<(), VcsError> {
        let mut index = self.git2.index().map_err(to_git)?;
        // `add_all` stages new and modified files; `update_all` additionally stages
        // deletions of tracked files.
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .map_err(to_git)?;
        index.update_all(["*"].iter(), None).map_err(to_git)?;
        index.write().map_err(to_git)
    }

    pub(crate) fn git2_unstage_all(&self) -> Result<(), VcsError> {
        let head = self.git2_head_commit()?;
        self.git2
            .reset_default(head.as_ref(), ["*"].iter())
            .map_err(to_git)
    }

    pub(crate) fn git2_discard(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        let workdir = self.git2_workdir()?;
        let head_tree = self.git2.head().ok().and_then(|h| h.peel_to_tree().ok());
        let mut index = self.git2.index().map_err(to_git)?;
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.force();
        let mut any_checkout = false;
        let mut index_dirty = false;
        for rel in paths {
            let in_head = head_tree.as_ref().is_some_and(|t| t.get_path(rel).is_ok());
            if in_head {
                // Restore the worktree and index entry to the committed version.
                checkout.path(rel);
                any_checkout = true;
            } else if index.get_path(rel, 0).is_some() {
                // Staged but never committed: un-add it, then delete the file.
                index.remove_path(rel).map_err(to_git)?;
                index_dirty = true;
                let _ = std::fs::remove_file(workdir.join(rel));
            } else {
                // Untracked: just remove it. `checkout_head` never deletes files
                // that are absent from HEAD, so this is the only way to drop them.
                let _ = std::fs::remove_file(workdir.join(rel));
            }
        }
        if index_dirty {
            index.write().map_err(to_git)?;
        }
        if any_checkout {
            self.git2
                .checkout_head(Some(&mut checkout))
                .map_err(to_git)?;
        }
        Ok(())
    }

    pub(crate) fn git2_commit(&self, message: &str) -> Result<String, VcsError> {
        let mut index = self.git2.index().map_err(to_git)?;
        // Errors with an "unmerged" git error when conflicts are still present.
        let tree_oid = index.write_tree().map_err(to_git)?;
        let tree = self.git2.find_tree(tree_oid).map_err(to_git)?;
        // Uses the repository's configured `user.name`/`user.email`; errors if unset.
        let sig = self.git2.signature().map_err(to_git)?;
        let parents = match self.git2.head() {
            Ok(head) => vec![head.peel_to_commit().map_err(to_git)?],
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Vec::new(),
            Err(e) => return Err(to_git(e)),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        let oid = self
            .git2
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .map_err(to_git)?;
        Ok(oid.to_string())
    }

    pub(crate) fn git2_staged_diff(&self) -> Result<crate::StagedDiff, VcsError> {
        let index = self.git2.index().map_err(to_git)?;
        // The `HEAD` tree, or `None` on an unborn branch — libgit2 then diffs the
        // index against the empty tree, so the first commit's staged files show as
        // additions rather than erroring.
        let head_tree = match self.git2_head_commit()? {
            Some(obj) => Some(obj.peel_to_tree().map_err(to_git)?),
            None => None,
        };
        let mut opts = git2::DiffOptions::new();
        let diff = self
            .git2
            .diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut opts))
            .map_err(to_git)?;

        let mut patch = String::new();
        diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
            // Context / added / removed lines carry their origin in a separate byte;
            // re-emit it so the text reads as a real `+`/`-`/` ` unified diff. Header
            // and hunk lines already embed their full text, so they pass through as-is.
            if matches!(line.origin(), '+' | '-' | ' ') {
                patch.push(line.origin());
            }
            patch.push_str(&String::from_utf8_lossy(line.content()));
            true
        })
        .map_err(to_git)?;

        let stats = diff.stats().map_err(to_git)?;
        let file_count = stats.files_changed();
        let stat = stats
            .to_buf(git2::DiffStatsFormat::FULL, 80)
            .map_err(to_git)?
            .as_str()
            .unwrap_or_default()
            .to_string();

        Ok(crate::StagedDiff {
            patch,
            stat,
            file_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use crate::Repository;
    use crate::Selection;
    use crate::StatusKind;
    use crate::VcsError;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A temp repository removed on drop.
    struct TempRepo(PathBuf);

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> Result<(), VcsError> {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map_err(|e| VcsError::Git(e.to_string()))?
            .success();
        ok.then_some(())
            .ok_or_else(|| VcsError::Git(format!("git {args:?} failed")))
    }

    fn init_repo() -> Result<TempRepo, VcsError> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("karet-vcs-write-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).map_err(|e| VcsError::Git(e.to_string()))?;
        let repo = TempRepo(path);
        git(&repo.0, &["init", "-q"])?;
        git(&repo.0, &["config", "user.email", "test@example.com"])?;
        git(&repo.0, &["config", "user.name", "karet test"])?;
        git(&repo.0, &["config", "commit.gpgsign", "false"])?;
        Ok(repo)
    }

    fn write(dir: &Path, rel: &str, contents: &[u8]) -> Result<(), VcsError> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| VcsError::Git(e.to_string()))?;
        }
        std::fs::write(&p, contents).map_err(|e| VcsError::Git(e.to_string()))
    }

    #[test]
    fn stage_then_commit_roundtrip() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"hello\n")?;
        let r = Repository::discover(&repo.0)?;

        r.stage(&[PathBuf::from("a.txt")])?;
        let staged = r.changes(Selection::Staged, None)?;
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].status, StatusKind::Added);

        let oid = r.commit("initial")?;
        assert_eq!(oid.len(), 40);
        assert!(r.changes(Selection::Staged, None)?.is_empty());
        assert!(r.changes(Selection::Unstaged, None)?.is_empty());
        Ok(())
    }

    #[test]
    fn unstage_with_unborn_head_removes_from_index() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"hello\n")?;
        let r = Repository::discover(&repo.0)?;
        r.stage(&[PathBuf::from("a.txt")])?;
        assert_eq!(r.changes(Selection::Staged, None)?.len(), 1);

        // No commit yet (unborn HEAD): unstage must remove the entry, not error.
        r.unstage(&[PathBuf::from("a.txt")])?;
        assert!(r.changes(Selection::Staged, None)?.is_empty());
        let working = r.changes(Selection::Unstaged, None)?;
        assert_eq!(working.len(), 1);
        assert_eq!(working[0].status, StatusKind::Untracked);
        Ok(())
    }

    #[test]
    fn staged_diff_on_unborn_head_shows_additions() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"hello\nworld\n")?;
        let r = Repository::discover(&repo.0)?;
        r.stage(&[PathBuf::from("a.txt")])?;

        let diff = r.staged_diff()?;
        assert_eq!(diff.file_count, 1);
        assert!(diff.patch.contains("+hello"), "patch: {}", diff.patch);
        assert!(diff.patch.contains("+world"), "patch: {}", diff.patch);
        assert!(diff.stat.contains("a.txt"), "stat: {}", diff.stat);
        Ok(())
    }

    #[test]
    fn staged_diff_reflects_only_staged_changes() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"one\n")?;
        git(&repo.0, &["add", "a.txt"])?;
        git(&repo.0, &["commit", "-q", "-m", "init"])?;
        // Stage a modification, then make a further unstaged edit on top of it.
        write(&repo.0, "a.txt", b"two\n")?;
        let r = Repository::discover(&repo.0)?;
        r.stage(&[PathBuf::from("a.txt")])?;
        write(&repo.0, "a.txt", b"three\n")?;

        let diff = r.staged_diff()?;
        assert_eq!(diff.file_count, 1);
        // The staged content is `two`; the unstaged `three` must not leak in.
        assert!(diff.patch.contains("+two"), "patch: {}", diff.patch);
        assert!(diff.patch.contains("-one"), "patch: {}", diff.patch);
        assert!(!diff.patch.contains("three"), "patch: {}", diff.patch);
        Ok(())
    }

    #[test]
    fn staged_diff_is_empty_with_nothing_staged() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"one\n")?;
        git(&repo.0, &["add", "a.txt"])?;
        git(&repo.0, &["commit", "-q", "-m", "init"])?;
        let r = Repository::discover(&repo.0)?;

        let diff = r.staged_diff()?;
        assert_eq!(diff.file_count, 0);
        assert!(diff.patch.is_empty(), "patch: {}", diff.patch);
        Ok(())
    }

    #[test]
    fn discard_deletes_untracked_file() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "junk.txt", b"x\n")?;
        let r = Repository::discover(&repo.0)?;
        assert_eq!(r.changes(Selection::Unstaged, None)?.len(), 1);

        r.discard(&[PathBuf::from("junk.txt")])?;
        assert!(!repo.0.join("junk.txt").exists());
        assert!(r.changes(Selection::Unstaged, None)?.is_empty());
        Ok(())
    }

    #[test]
    fn discard_restores_modified_tracked_file() -> Result<(), VcsError> {
        let repo = init_repo()?;
        write(&repo.0, "a.txt", b"one\n")?;
        git(&repo.0, &["add", "a.txt"])?;
        git(&repo.0, &["commit", "-q", "-m", "init"])?;
        write(&repo.0, "a.txt", b"two\n")?;
        let r = Repository::discover(&repo.0)?;
        assert_eq!(r.changes(Selection::Unstaged, None)?.len(), 1);

        r.discard(&[PathBuf::from("a.txt")])?;
        let restored =
            std::fs::read(repo.0.join("a.txt")).map_err(|e| VcsError::Git(e.to_string()))?;
        assert_eq!(restored, b"one\n");
        assert!(r.changes(Selection::Unstaged, None)?.is_empty());
        Ok(())
    }

    #[test]
    fn staging_in_a_linked_worktree_uses_the_worktree_index() -> Result<(), VcsError> {
        let main = init_repo()?;
        write(&main.0, "a.txt", b"base\n")?;
        git(&main.0, &["add", "a.txt"])?;
        git(&main.0, &["commit", "-q", "-m", "init"])?;
        // Create a linked worktree (its index/HEAD live in .git/worktrees/wt).
        git(&main.0, &["worktree", "add", "-q", "-b", "feature", "wt"])?;
        let wt = main.0.join("wt");

        // Modify and stage the file *in the worktree* via karet-vcs.
        write(&wt, "a.txt", b"changed\n")?;
        let r = Repository::discover(&wt)?;
        r.stage(&[PathBuf::from("a.txt")])?;

        // The worktree's index shows the staged modification...
        let staged = r.changes(Selection::Staged, None)?;
        assert!(
            staged
                .iter()
                .any(|c| c.path.as_path() == Path::new("a.txt") && c.status == StatusKind::Modified)
        );
        // ...while the main worktree's index is untouched.
        let main_repo = Repository::discover(&main.0)?;
        assert!(main_repo.changes(Selection::Staged, None)?.is_empty());
        Ok(())
    }
}
