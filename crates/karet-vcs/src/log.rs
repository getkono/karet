//! Commit-history log: a paginated `git log`-style walk from `HEAD`.

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
            let commit = self.inner.find_commit(info.id).map_err(to_git)?;
            let hash = info.id.to_hex().to_string();
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
            out.push(Commit {
                hash,
                short_hash,
                summary,
                author,
                time,
            });
        }
        Ok(out)
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
}
