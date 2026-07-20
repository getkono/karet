//! Argument-safe `git` write operations.
//!
//! Repository reads stay in-process through `gix`. Mutations use the user's Git so
//! worktree/index semantics, hooks, identities, and configuration match the command
//! line without adding a C-backed library dependency. No operation invokes a shell.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;

use crate::Repository;
use crate::StagedDiff;
use crate::VcsError;

impl Repository {
    fn workdir(&self) -> Result<&Path, VcsError> {
        self.inner
            .workdir()
            .ok_or_else(|| VcsError::Git("repository has no working directory".to_string()))
    }

    pub(crate) fn git_output<I, S>(&self, args: I) -> Result<Output, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .args(args)
            .current_dir(self.workdir()?)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .output()
            .map_err(|error| VcsError::GitUnavailable(error.to_string()))
    }

    pub(crate) fn git_checked<I, S>(&self, args: I) -> Result<Output, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git_output(args)?;
        if output.status.success() {
            Ok(output)
        } else {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(VcsError::Git(if message.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                message
            }))
        }
    }

    fn has_head(&self) -> Result<bool, VcsError> {
        Ok(self
            .git_output(["rev-parse", "--verify", "--quiet", "HEAD"])?
            .status
            .success())
    }

    fn path_args(prefix: &[&str], paths: &[PathBuf]) -> Result<Vec<OsString>, VcsError> {
        let mut args: Vec<OsString> = prefix.iter().map(OsString::from).collect();
        args.push(OsString::from("--"));
        for path in paths {
            validate_relative(path)?;
            args.push(path.as_os_str().to_owned());
        }
        Ok(args)
    }

    pub(crate) fn git_stage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        if paths.is_empty() {
            return Ok(());
        }
        self.git_checked(Self::path_args(&["add"], paths)?)?;
        Ok(())
    }

    pub(crate) fn git_unstage(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        if paths.is_empty() {
            return Ok(());
        }
        let prefix = if self.has_head()? {
            vec!["reset", "--quiet", "HEAD"]
        } else {
            vec!["rm", "--cached", "--quiet", "--ignore-unmatch", "-r"]
        };
        self.git_checked(Self::path_args(&prefix, paths)?)?;
        Ok(())
    }

    pub(crate) fn git_stage_all(&self) -> Result<(), VcsError> {
        self.git_checked(["add", "--all", "--", "."])?;
        Ok(())
    }

    pub(crate) fn git_unstage_all(&self) -> Result<(), VcsError> {
        if self.has_head()? {
            self.git_checked(["reset", "--quiet", "HEAD", "--", "."])?;
        } else {
            self.git_checked([
                "rm",
                "--cached",
                "--quiet",
                "--ignore-unmatch",
                "-r",
                "--",
                ".",
            ])?;
        }
        Ok(())
    }

    pub(crate) fn git_discard(&self, paths: &[PathBuf]) -> Result<(), VcsError> {
        let workdir = self.workdir()?.to_path_buf();
        for path in paths {
            validate_relative(path)?;
            let in_head = self
                .git_output([OsStr::new("cat-file"), OsStr::new("-e"), &head_path(path)])?
                .status
                .success();
            if in_head {
                self.git_checked(Self::path_args(
                    &["restore", "--source=HEAD", "--staged", "--worktree"],
                    std::slice::from_ref(path),
                )?)?;
            } else {
                self.git_checked(Self::path_args(
                    &["rm", "--cached", "--quiet", "--ignore-unmatch", "-r", "-f"],
                    std::slice::from_ref(path),
                )?)?;
                remove_untracked(&workdir.join(path))?;
            }
        }
        Ok(())
    }

    pub(crate) fn git_commit(&self, message: &str) -> Result<String, VcsError> {
        self.git_checked([
            OsStr::new("commit"),
            OsStr::new("--quiet"),
            OsStr::new("-m"),
            OsStr::new(message),
        ])?;
        let output = self.git_checked(["rev-parse", "HEAD"])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub(crate) fn git_staged_diff(&self) -> Result<StagedDiff, VcsError> {
        let patch = self.git_checked(["diff", "--cached", "--binary", "--no-ext-diff"])?;
        let stat = self.git_checked(["diff", "--cached", "--stat", "--no-ext-diff"])?;
        let names = self.git_checked(["diff", "--cached", "--name-only", "--no-ext-diff"])?;
        Ok(StagedDiff {
            patch: String::from_utf8_lossy(&patch.stdout).into_owned(),
            stat: String::from_utf8_lossy(&stat.stdout).into_owned(),
            file_count: names
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|row| !row.is_empty())
                .count(),
        })
    }
}

fn validate_relative(path: &Path) -> Result<(), VcsError> {
    let valid = !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    valid.then_some(()).ok_or_else(|| {
        VcsError::Git(format!(
            "unsafe repository-relative path: {}",
            path.display()
        ))
    })
}

fn head_path(path: &Path) -> OsString {
    let mut value = OsString::from("HEAD:");
    value.push(path);
    value
}

fn remove_untracked(path: &Path) -> Result<(), VcsError> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|error| VcsError::Git(error.to_string()))
        },
        Ok(_) => std::fs::remove_file(path).map_err(|error| VcsError::Git(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(VcsError::Git(error.to_string())),
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
