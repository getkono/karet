//! The hardened `git` subprocess runners every CLI-backed read and write uses.
//!
//! Repository reads stay in-process through `gix`; the operations that need the
//! user's Git (writes, network, and the few reads `gix` does not cover) run it as
//! a child process through this module, so the non-interactive environment and
//! the stderr-to-[`VcsError`] mapping are defined once. No runner invokes a shell.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;
use std::process::Output;

use crate::Repository;
use crate::VcsError;

impl Repository {
    /// The worktree root, or an error for a bare repository.
    pub(crate) fn workdir(&self) -> Result<&Path, VcsError> {
        self.inner
            .workdir()
            .ok_or_else(|| VcsError::Git("repository has no working directory".to_string()))
    }

    /// A `git` command rooted at the worktree under the non-interactive
    /// environment every write runs with: no credential prompt, and an editor
    /// that accepts whatever message or todo list it is handed.
    pub(crate) fn git_command(&self) -> Result<Command, VcsError> {
        let mut command = Command::new("git");
        command
            .current_dir(self.workdir()?)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true");
        Ok(command)
    }

    /// Run `git` and return its output whatever its exit status; only a `git`
    /// that cannot be launched is an error.
    pub(crate) fn git_output<I, S>(&self, args: I) -> Result<Output, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git_command()?
            .args(args)
            .output()
            .map_err(|error| VcsError::GitUnavailable(error.to_string()))
    }

    /// [`git_checked`](Self::git_checked) with extra environment overrides
    /// (e.g. a prepared `GIT_SEQUENCE_EDITOR` for interactive rebase).
    pub(crate) fn git_checked_with_env<I, S>(
        &self,
        args: I,
        env: &[(&str, &str)],
    ) -> Result<Output, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.git_command()?;
        command.args(args).envs(env.iter().copied());
        let output = command
            .output()
            .map_err(|error| VcsError::GitUnavailable(error.to_string()))?;
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

    /// Run `git`, mapping a non-zero exit to [`VcsError::Git`] carrying its
    /// stderr (or the exit status when stderr is empty).
    pub(crate) fn git_checked<I, S>(&self, args: I) -> Result<Output, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.git_checked_with_env(args, &[])
    }

    /// [`git_checked`](Self::git_checked)'s stdout as trimmed (lossy UTF-8) text.
    pub(crate) fn git_text<I, S>(&self, args: I) -> Result<String, VcsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git_checked(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::Repository;
    use crate::VcsError;
    use crate::test_support::TestRepo;

    fn init_repo() -> Result<TestRepo, VcsError> {
        crate::test_support::init("subprocess")
    }

    /// `git_checked` is `git_checked_with_env` with no overrides, so both carry
    /// the hardened editor, an override still wins, and a refusal surfaces
    /// git's own stderr.
    #[test]
    fn checked_git_runs_hardened_and_reports_stderr() -> Result<(), VcsError> {
        let repo = init_repo()?;
        let r = Repository::discover(&repo.0)?;
        let editor = |output: std::process::Output| {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        assert_eq!(editor(r.git_checked(["var", "GIT_EDITOR"])?), "true");
        assert_eq!(
            editor(r.git_checked_with_env(["var", "GIT_EDITOR"], &[("GIT_EDITOR", "vi")])?),
            "vi"
        );
        let refused = r.git_checked(["rev-parse", "--verify", "nope"]);
        assert!(
            matches!(&refused, Err(VcsError::Git(message)) if message.starts_with("fatal:")),
            "{refused:?}"
        );
        Ok(())
    }

    #[test]
    fn git_errors_carry_stderr_or_a_status_fallback() -> Result<(), VcsError> {
        let repo = init_repo()?;
        let r = Repository::discover(&repo.0)?;
        // A failing command with stderr surfaces that stderr verbatim (trimmed).
        match r.git_checked(["rev-parse", "--verify", "no-such-rev"]) {
            Err(VcsError::Git(message)) => {
                assert!(message.starts_with("fatal:"), "stderr was {message:?}");
                assert_eq!(message, message.trim());
            },
            other => {
                return Err(VcsError::Git(format!(
                    "expected a git error, got {other:?}"
                )));
            },
        }
        // A failure that writes nothing to stderr falls back to the exit status.
        match r.git_checked(["rev-parse", "--verify", "--quiet", "no-such-rev"]) {
            Err(VcsError::Git(message)) => {
                assert!(message.starts_with("git exited with"), "{message:?}");
            },
            other => {
                return Err(VcsError::Git(format!(
                    "expected a git error, got {other:?}"
                )));
            },
        }
        // The env-overriding variant maps failures the same way.
        assert!(matches!(
            r.git_checked_with_env(["rev-parse", "--verify", "--quiet", "no-such-rev"], &[]),
            Err(VcsError::Git(message)) if message.starts_with("git exited with")
        ));
        Ok(())
    }

    /// `git_output` reports a failing exit as output, not an error, and
    /// `git_text` trims what a successful command printed.
    #[test]
    fn unchecked_output_and_trimmed_text() -> Result<(), VcsError> {
        let repo = init_repo()?;
        let r = Repository::discover(&repo.0)?;
        let refused = r.git_output(["rev-parse", "--verify", "--quiet", "no-such-rev"])?;
        assert!(!refused.status.success());
        assert_eq!(r.git_text(["var", "GIT_EDITOR"])?, "true");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_git_binary_is_git_unavailable() -> Result<(), VcsError> {
        let repo = init_repo()?;
        let r = Repository::discover(&repo.0)?;
        // An overridden child `PATH` is what the program lookup searches, so an
        // empty directory makes `git` unspawnable without touching this process.
        let empty = crate::test_support::scratch_dir("no-git")?;
        let path = empty.0.to_string_lossy().into_owned();
        assert!(matches!(
            r.git_checked_with_env(["status"], &[("PATH", path.as_str())]),
            Err(VcsError::GitUnavailable(_))
        ));
        Ok(())
    }
}
