//! Repository discovery and the error-mapping helpers shared by the other modules.

use std::path::Path;
use std::path::PathBuf;

use crate::Repository;
use crate::VcsError;

/// Map any error that implements [`std::fmt::Display`] into [`VcsError::Git`].
pub(crate) fn to_git<E: std::fmt::Display>(e: E) -> VcsError {
    VcsError::Git(e.to_string())
}

/// Map a discovery error: "no repository found" becomes [`VcsError::NotARepository`];
/// anything else (e.g. an inaccessible directory) becomes [`VcsError::Git`].
fn map_discover(e: gix::discover::Error) -> VcsError {
    use gix::discover::upwards::Error as U;
    match e {
        gix::discover::Error::Discover(
            U::NoGitRepository { .. }
            | U::NoGitRepositoryWithinCeiling { .. }
            | U::NoGitRepositoryWithinFs { .. }
            | U::NoMatchingCeilingDir
            | U::NoTrustedGitRepository { .. },
        ) => VcsError::NotARepository,
        other => VcsError::Git(other.to_string()),
    }
}

impl Repository {
    /// Discover the repository containing `path`, searching upwards through parents.
    ///
    /// # Errors
    /// Returns [`VcsError::NotARepository`] if no repository is found, or
    /// [`VcsError::Git`] for any other discovery failure.
    pub fn discover(path: &Path) -> Result<Self, VcsError> {
        let inner = gix::discover(path).map_err(map_discover)?;
        Ok(Self { inner })
    }

    /// The URL of the `origin` remote, if one is configured. Used to derive the
    /// `owner/repo` for forge lookups (e.g. GitHub commit verification).
    #[must_use]
    pub fn origin_url(&self) -> Option<String> {
        use gix::bstr::ByteSlice;
        self.inner
            .config_snapshot()
            .string("remote.origin.url")
            .map(|v| v.to_str_lossy().into_owned())
    }

    /// The path of `path` relative to the repository's worktree root, or `None` when
    /// `path` lies outside the worktree (or the repository is bare). Both sides are
    /// canonicalized first (resolving `.`, `..`, and symlinks), so a relative `path`
    /// resolves against the process's current directory — the same resolution
    /// [`file_history`](Self::file_history) and [`file_at_rev`](Self::file_at_rev)
    /// apply to their `path` argument.
    #[must_use]
    pub fn path_in_worktree(&self, path: &Path) -> Option<PathBuf> {
        crate::changes::repo_relative(&self.inner, path)
    }

    /// The git-metadata directories whose contents a file watcher should observe to
    /// keep status fresh: the per-worktree git directory (holding `index`, `HEAD`,
    /// `MERGE_HEAD`) and, for a linked worktree, the common directory (holding
    /// `refs`, `packed-refs`). The two coincide for an ordinary repository and are
    /// deduplicated. Paths are canonicalized so they match the absolute paths a
    /// platform watcher reports.
    #[must_use]
    pub fn metadata_dirs(&self) -> Vec<PathBuf> {
        let git_dir = self.inner.git_dir();
        let common_dir = self.inner.common_dir();
        let mut dirs = vec![git_dir.to_path_buf()];
        if common_dir != git_dir {
            dirs.push(common_dir.to_path_buf());
        }
        dirs.iter()
            .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
            .collect()
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

    /// A temp directory removed on drop.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn metadata_dirs_is_single_git_dir_for_normal_repo() -> Result<(), VcsError> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("karet-vcs-meta-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| VcsError::Git(e.to_string()))?;
        let _guard = TempDir(dir.clone());
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .map_err(|e| VcsError::Git(e.to_string()))?;
        assert!(status.success());

        let repo = Repository::discover(&dir)?;
        let dirs = repo.metadata_dirs();
        // A non-worktree repo has git_dir == common_dir, so exactly one entry.
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".git"));
        assert!(dirs[0].is_dir());
        Ok(())
    }

    #[test]
    fn path_in_worktree_maps_inside_and_rejects_outside() -> Result<(), VcsError> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("karet-vcs-rel-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).map_err(|e| VcsError::Git(e.to_string()))?;
        let _guard = TempDir(dir.clone());
        let status = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .map_err(|e| VcsError::Git(e.to_string()))?;
        assert!(status.success());
        std::fs::write(dir.join("sub/a.txt"), "x\n").map_err(|e| VcsError::Git(e.to_string()))?;

        let repo = Repository::discover(&dir)?;
        assert_eq!(
            repo.path_in_worktree(&dir.join("sub/a.txt")),
            Some(std::path::PathBuf::from("sub/a.txt"))
        );
        assert!(repo.path_in_worktree(&std::env::temp_dir()).is_none());
        Ok(())
    }
}
