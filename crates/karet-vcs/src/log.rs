//! Commit-history log: a paginated `git log`-style walk from `HEAD`.

use std::path::Path;

use crate::Repository;
use crate::VcsError;
use crate::repo::to_git;

/// One commit in the history log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    /// The full commit hash (hex).
    pub hash: String,
    /// The abbreviated hash (first 7 hex characters).
    pub short_hash: String,
    /// The first line of the commit message.
    pub summary: String,
    /// The author's name.
    pub author: String,
    /// The commit time, in seconds since the Unix epoch.
    pub time: i64,
    /// The full hex hashes of this commit's parents, first-parent first. Empty for a
    /// root commit; two or more for a merge. Drives the DAG lane layout.
    pub parents: Vec<String>,
}

impl Repository {
    /// Walk the commit history from `HEAD`, skipping the first `skip` commits and
    /// returning up to `limit` more, newest first. This is the backing read for a
    /// lazily-paged source-control log.
    ///
    /// Returns an empty vector when the branch is unborn (no `HEAD` commit yet).
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn log(&self, skip: usize, limit: usize) -> Result<Vec<Commit>, VcsError> {
        // An unborn branch has no HEAD commit; that is an empty log, not an error.
        let Ok(head) = self.inner.head_id() else {
            return Ok(Vec::new());
        };
        let walk = self
            .inner
            .rev_walk(Some(head.detach()))
            .all()
            .map_err(to_git)?;

        let mut out = Vec::with_capacity(limit.min(64));
        for info in walk.skip(skip).take(limit) {
            let info = info.map_err(to_git)?;
            let hash = info.id.to_hex().to_string();
            out.push(self.build_commit(info.id, hash)?);
        }
        Ok(out)
    }

    /// The full hex hash of the current `HEAD` commit, or `None` on an unborn branch.
    ///
    /// A cheap single-ref read used to detect when the branch tip has moved (a new
    /// commit, amend, rebase, or checkout) so the log can be reconciled incrementally
    /// rather than re-walked in full.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn head_hash(&self) -> Result<Option<String>, VcsError> {
        // An unborn branch has no HEAD commit; that is `None`, not an error.
        Ok(self.inner.head_id().ok().map(|id| id.to_hex().to_string()))
    }

    /// Walk commits from `HEAD` until reaching `stop` (exclusive), collecting at most
    /// `cap`, newest first. Used to reconcile the log after the branch tip moves: pass
    /// the previously-known top commit hash as `stop` to fetch only what is new.
    ///
    /// If the returned length equals `cap` the walk may not have reached `stop` (the
    /// history was rewritten, or more than `cap` commits arrived at once), so the
    /// caller should fall back to a full reload rather than prepend a partial slice.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn commits_since(&self, stop: Option<&str>, cap: usize) -> Result<Vec<Commit>, VcsError> {
        let Ok(head) = self.inner.head_id() else {
            return Ok(Vec::new());
        };
        let walk = self
            .inner
            .rev_walk(Some(head.detach()))
            .all()
            .map_err(to_git)?;

        let mut out = Vec::new();
        for info in walk.take(cap) {
            let info = info.map_err(to_git)?;
            let hash = info.id.to_hex().to_string();
            if stop == Some(hash.as_str()) {
                break;
            }
            out.push(self.build_commit(info.id, hash)?);
        }
        Ok(out)
    }

    /// Walk the history of a single file, newest first: the commits that changed the
    /// blob at `path` (added, modified, or removed it relative to their first parent),
    /// like `git log -- <path>`. Skips the first `skip` matches and returns up to
    /// `limit` more.
    ///
    /// `path` is resolved the same way as [`changes`](Self::changes): a relative path is
    /// relative to the process's current directory. A path outside the worktree, or an
    /// unborn branch, yields an empty vector.
    ///
    /// # Errors
    /// Returns [`VcsError::Git`] on failure.
    pub fn file_history(
        &self,
        path: &Path,
        skip: usize,
        limit: usize,
    ) -> Result<Vec<Commit>, VcsError> {
        let Some(rel) = crate::changes::repo_relative(&self.inner, path) else {
            return Ok(Vec::new());
        };
        let Ok(head) = self.inner.head_id() else {
            return Ok(Vec::new());
        };
        let walk = self
            .inner
            .rev_walk(Some(head.detach()))
            .all()
            .map_err(to_git)?;

        let mut out = Vec::with_capacity(limit.min(64));
        let mut skipped = 0usize;
        for info in walk {
            let info = info.map_err(to_git)?;
            let commit = self.inner.find_commit(info.id).map_err(to_git)?;
            // The blob id at `rel` in this commit's tree.
            let here = self.blob_at(&commit, &rel)?;
            // The blob id at `rel` in the first parent (None => root, treat as absent).
            let parent = match commit.parent_ids().next() {
                Some(pid) => {
                    let pc = self.inner.find_commit(pid).map_err(to_git)?;
                    self.blob_at(&pc, &rel)?
                },
                None => None,
            };
            // The commit touched the file when the blob id differs from the parent's
            // (an add, a modification, or a deletion). Unchanged => skip.
            if here == parent {
                continue;
            }
            if skipped < skip {
                skipped += 1;
                continue;
            }
            let hash = info.id.to_hex().to_string();
            out.push(self.build_commit(info.id, hash)?);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// The object id of the blob at repo-relative `rel` in `commit`'s tree, or `None`
    /// when the path is absent (or names a tree/submodule rather than a file).
    fn blob_at(
        &self,
        commit: &gix::Commit<'_>,
        rel: &Path,
    ) -> Result<Option<gix::ObjectId>, VcsError> {
        let tree = commit.tree().map_err(to_git)?;
        let entry = tree.lookup_entry_by_path(rel).map_err(to_git)?;
        Ok(entry
            .filter(|e| e.mode().is_blob_or_symlink())
            .map(|e| e.id().detach()))
    }

    /// Build a [`Commit`] from a resolved object id and its precomputed hex hash.
    fn build_commit(&self, id: gix::ObjectId, hash: String) -> Result<Commit, VcsError> {
        let commit = self.inner.find_commit(id).map_err(to_git)?;
        let short_hash = hash.chars().take(7).collect();
        let summary = commit
            .message()
            .map(|m| m.summary().to_string())
            .unwrap_or_default();
        let author = commit
            .author()
            .map(|a| a.name.to_string())
            .unwrap_or_default();
        let time = commit.time().map(|t| t.seconds).unwrap_or_default();
        // Parent ids are already decoded in the commit object; collect them (first
        // parent first) so the renderer can lay out branch/merge lanes.
        let parents = commit
            .parent_ids()
            .map(|id| id.to_hex().to_string())
            .collect();
        Ok(Commit {
            hash,
            short_hash,
            summary,
            author,
            time,
            parents,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use crate::Repository;
    use crate::VcsError;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "git {args:?} failed");
    }

    fn io(e: std::io::Error) -> VcsError {
        VcsError::Git(e.to_string())
    }

    fn repo_with_commits(n: usize) -> Result<(TempDir, Repository), VcsError> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("karet-vcs-log-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let guard = TempDir(dir.clone());
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Tester"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        for i in 0..n {
            std::fs::write(dir.join("f.txt"), format!("v{i}\n")).map_err(io)?;
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", &format!("commit {i}")]);
        }
        let repo = Repository::discover(&dir)?;
        Ok((guard, repo))
    }

    #[test]
    fn log_returns_newest_first() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(3)?;
        let log = repo.log(0, 10)?;
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].summary, "commit 2");
        assert_eq!(log[2].summary, "commit 0");
        assert_eq!(log[0].author, "Tester");
        assert_eq!(log[0].short_hash.len(), 7);
        assert!(log[0].hash.starts_with(&log[0].short_hash));
        Ok(())
    }

    #[test]
    fn log_paginates_with_skip_and_limit() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(5)?;
        let page = repo.log(1, 2)?;
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].summary, "commit 3");
        assert_eq!(page[1].summary, "commit 2");
        Ok(())
    }

    #[test]
    fn log_is_empty_on_unborn_branch() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(0)?;
        assert!(repo.log(0, 10)?.is_empty());
        Ok(())
    }

    #[test]
    fn head_hash_tracks_the_tip() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(2)?;
        let head = repo.head_hash()?;
        assert_eq!(head.as_deref(), Some(repo.log(0, 1)?[0].hash.as_str()));
        Ok(())
    }

    #[test]
    fn head_hash_is_none_on_unborn_branch() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(0)?;
        assert!(repo.head_hash()?.is_none());
        Ok(())
    }

    #[test]
    fn commits_since_returns_only_the_new_ones() -> Result<(), VcsError> {
        let (g, repo) = repo_with_commits(3)?;
        // The tip as previously known: fetch nothing new against the current HEAD.
        let tip = repo.head_hash()?;
        assert!(repo.commits_since(tip.as_deref(), 25)?.is_empty());
        // Add two commits; commits_since(old tip) returns exactly those two, newest first.
        std::fs::write(g.0.join("f.txt"), "new1\n").map_err(io)?;
        git(&g.0, &["commit", "-qam", "commit 3"]);
        std::fs::write(g.0.join("f.txt"), "new2\n").map_err(io)?;
        git(&g.0, &["commit", "-qam", "commit 4"]);
        let fresh = repo.commits_since(tip.as_deref(), 25)?;
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0].summary, "commit 4");
        assert_eq!(fresh[1].summary, "commit 3");
        Ok(())
    }

    #[test]
    fn commits_since_caps_and_signals_fallback() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(5)?;
        // With no known tip and a small cap, the walk fills to `cap`, signalling the
        // caller to reload in full rather than trust a partial slice.
        let capped = repo.commits_since(None, 2)?;
        assert_eq!(capped.len(), 2);
        Ok(())
    }

    #[test]
    fn linear_history_records_single_parents() -> Result<(), VcsError> {
        let (_g, repo) = repo_with_commits(2)?;
        let log = repo.log(0, 10)?;
        assert_eq!(log.len(), 2);
        // The newest commit's single parent is the older commit; the root has none.
        assert_eq!(log[0].parents, vec![log[1].hash.clone()]);
        assert!(log[1].parents.is_empty(), "root commit has no parents");
        Ok(())
    }

    #[test]
    fn file_history_lists_only_commits_touching_the_path() -> Result<(), VcsError> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("karet-vcs-hist-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let _guard = TempDir(dir.clone());
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Tester"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        // c0 creates a.txt; c1 touches only b.txt; c2 modifies a.txt again.
        std::fs::write(dir.join("a.txt"), "a0\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "c0 add a"]);
        std::fs::write(dir.join("b.txt"), "b0\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "c1 add b"]);
        std::fs::write(dir.join("a.txt"), "a1\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "c2 modify a"]);

        let repo = Repository::discover(&dir)?;
        let hist = repo.file_history(&dir.join("a.txt"), 0, 10)?;
        let summaries: Vec<&str> = hist.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, vec!["c2 modify a", "c0 add a"]);
        // Paging: skip the newest, take one.
        let page = repo.file_history(&dir.join("a.txt"), 1, 1)?;
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].summary, "c0 add a");
        Ok(())
    }

    #[test]
    fn merge_commit_records_two_parents() -> Result<(), VcsError> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("karet-vcs-merge-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let _guard = TempDir(dir.clone());
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.name", "Tester"]);
        git(&dir, &["config", "user.email", "t@example.com"]);
        std::fs::write(dir.join("f.txt"), "base\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "base"]);
        // A side branch with its own commit.
        git(&dir, &["checkout", "-q", "-b", "side"]);
        std::fs::write(dir.join("s.txt"), "side\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "side commit"]);
        // Back to the original branch, diverge, then merge (no fast-forward).
        git(&dir, &["checkout", "-q", "-"]);
        std::fs::write(dir.join("m.txt"), "main\n").map_err(io)?;
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "main commit"]);
        git(
            &dir,
            &["merge", "--no-ff", "-q", "-m", "merge side", "side"],
        );

        let repo = Repository::discover(&dir)?;
        let log = repo.log(0, 10)?;
        assert_eq!(log[0].summary, "merge side");
        assert_eq!(log[0].parents.len(), 2, "a merge records both parents");
        Ok(())
    }
}
