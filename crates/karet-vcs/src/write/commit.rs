//! Committing with the hooks' console output streamed back as it is produced.
//!
//! A commit is the one write whose *output* matters. Hooks run inside it — a
//! formatter, a linter, a test suite — and what they print is the only account of
//! why a commit took thirty seconds or why it was refused. Buffering that until
//! the process exits throws away the part a person is waiting on.
//!
//! # Why two threads and a channel
//!
//! `git` writes to two pipes. Reading them one after the other deadlocks as soon
//! as the unread one fills its 64 KiB buffer, which a chatty hook does easily. So
//! each pipe gets a reader thread, both feed one channel, and the caller drains
//! that channel until both readers are done and only then waits on the child.
//!
//! The sink is called on the caller's thread, never a reader's: it belongs to
//! whoever asked for the commit, and hauling it across a thread boundary would
//! force `Send + Sync` on every caller for no benefit.
//!
//! # Why stdin is closed
//!
//! `Command::output` connects stdin to null, but `spawn` *inherits* it — and the
//! editor's terminal is in raw mode. A hook that reads stdin would sit there
//! forever, eating the user's keystrokes as it went. Closing it turns that hang
//! into an immediate EOF, which is what a hook run non-interactively should see.

use std::ffi::OsStr;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;

use crate::Repository;
use crate::VcsError;

/// Which of `git`'s two streams a line arrived on.
///
/// **Not a severity.** `git` connects a hook's own stdout to *its* stderr, so
/// everything a hook prints — progress, results, complaints — arrives on
/// [`Stderr`](Self::Stderr), and [`Stdout`](Self::Stdout) carries little but
/// git's own commit summary. A console that paints this stream as an error
/// paints every successful hook red.
///
/// It is reported rather than merged because two pipes are not interleaved in
/// any order the operating system promises, and a reader that knows which pipe a
/// line came from can make sense of an order that is only approximate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OutputStream {
    /// `git`'s standard output, where hooks usually report progress.
    Stdout,
    /// `git`'s standard error, where hooks usually report problems.
    Stderr,
}

/// One line of a running commit's console output.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CommitOutputLine {
    /// The stream it arrived on.
    pub stream: OutputStream,
    /// The line, without its trailing newline. Escape sequences are preserved:
    /// hooks colour their output, and a console that strips it reads worse.
    pub text: String,
}

impl Repository {
    /// Commit the staged changes, calling `on_line` for every line `git` and its
    /// hooks print, and returning the new commit's hash.
    ///
    /// Every line reaches `on_line` before success or failure is decided, so a
    /// hook that fails after printing its diagnosis still delivers it.
    pub fn commit_streaming(
        &self,
        message: &str,
        on_line: &mut dyn FnMut(CommitOutputLine),
    ) -> Result<String, VcsError> {
        let mut child = self.spawn_commit(message)?;
        let (sender, receiver) = mpsc::channel();
        let readers = [
            reader(child.stdout.take(), OutputStream::Stdout, sender.clone()),
            reader(child.stderr.take(), OutputStream::Stderr, sender),
        ];
        // Drain until both readers have dropped their sender, then reap the
        // child: waiting first is what deadlocks on a full pipe.
        let mut stderr_tail = String::new();
        for line in receiver {
            if line.stream == OutputStream::Stderr && !line.text.trim().is_empty() {
                stderr_tail = line.text.clone();
            }
            on_line(line);
        }
        for reader in readers.into_iter().flatten() {
            let _ = reader.join();
        }
        let status = child
            .wait()
            .map_err(|error| VcsError::Git(error.to_string()))?;
        if !status.success() {
            return Err(VcsError::Git(if stderr_tail.is_empty() {
                format!("git exited with {status}")
            } else {
                stderr_tail
            }));
        }
        let head = self.git_checked(["rev-parse", "HEAD"])?;
        Ok(String::from_utf8_lossy(&head.stdout).trim().to_string())
    }

    /// Start `git commit`, with both pipes captured and stdin closed.
    fn spawn_commit(&self, message: &str) -> Result<Child, VcsError> {
        Command::new("git")
            .args([OsStr::new("commit"), OsStr::new("-m"), OsStr::new(message)])
            .current_dir(self.commit_workdir()?)
            // The same non-interactive environment every other write runs under.
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| VcsError::GitUnavailable(error.to_string()))
    }
}

/// Spawn a thread forwarding every line of `pipe` to `sender`, tagged `stream`.
fn reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    stream: OutputStream,
    sender: mpsc::Sender<CommitOutputLine>,
) -> Option<std::thread::JoinHandle<()>> {
    let pipe = pipe?;
    Some(std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            // A closed receiver means nobody is listening any more; draining the
            // rest of the pipe still matters, so keep reading rather than break.
            let _ = sender.send(CommitOutputLine {
                stream,
                text: line.trim_end_matches('\r').to_string(),
            });
        }
    }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::Repository;
    use crate::VcsError;
    use crate::test_support::commit;
    use crate::test_support::git;
    use crate::test_support::init;
    use crate::test_support::write;

    /// Install an executable `pre-commit` hook running `body`.
    fn hook(repo: &std::path::Path, body: &str) -> Result<(), VcsError> {
        use std::os::unix::fs::PermissionsExt as _;
        let path = repo.join(".git/hooks/pre-commit");
        write(
            repo,
            ".git/hooks/pre-commit",
            format!("#!/bin/sh\n{body}\n").as_bytes(),
        )?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| VcsError::Git(error.to_string()))
    }

    /// Stage a change so there is something to commit.
    fn stage(repo: &std::path::Path, body: &str) -> Result<(), VcsError> {
        write(repo, "file.txt", body.as_bytes())?;
        git(repo, &["add", "file.txt"]).map(|_| ())
    }

    #[test]
    fn a_hooks_output_arrives_on_both_streams_and_the_commit_lands()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("stream")?;
        commit(&fixture, "one", "first")?;
        hook(&fixture.0, "echo running checks\necho a warning >&2")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let mut lines = Vec::new();
        let oid = repository.commit_with_output("second", &mut |line| lines.push(line))?;
        assert_eq!(oid.len(), 40, "the commit landed: {oid}");
        // Both of the hook's streams arrive on git's stderr — see `OutputStream`.
        assert!(
            lines
                .iter()
                .any(|line| line.stream == OutputStream::Stderr
                    && line.text.contains("running checks")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.stream == OutputStream::Stderr && line.text.contains("a warning")),
            "{lines:?}"
        );
        // And git's own summary arrives on stdout, so both pipes are captured.
        assert!(
            lines
                .iter()
                .any(|line| line.stream == OutputStream::Stdout && line.text.contains("second")),
            "{lines:?}"
        );
        Ok(())
    }

    #[test]
    fn a_failing_hook_still_delivers_everything_it_printed()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("stream-fail")?;
        commit(&fixture, "one", "first")?;
        // The diagnosis goes to stdout, which the buffered path used to discard
        // entirely — leaving a refusal with no reason attached.
        hook(&fixture.0, "echo the diagnosis\necho refused >&2\nexit 1")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let mut lines = Vec::new();
        let Err(error) = repository.commit_with_output("second", &mut |line| lines.push(line))
        else {
            unreachable!("a failing hook must refuse the commit");
        };
        assert!(
            lines.iter().any(|line| line.text.contains("the diagnosis")),
            "stdout survives a failure: {lines:?}"
        );
        assert!(error.to_string().contains("refused"), "{error}");
        Ok(())
    }

    #[test]
    fn a_flood_on_both_pipes_never_deadlocks() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("stream-flood")?;
        commit(&fixture, "one", "first")?;
        // Far past a pipe's 64 KiB buffer on both streams at once: reading them
        // in sequence would wedge here and never return.
        hook(
            &fixture.0,
            "i=0; while [ $i -lt 5000 ]; do echo \"out $i\"; echo \"err $i\" >&2; i=$((i+1)); done",
        )?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let mut lines = 0usize;
        let mut stdout = 0usize;
        repository.commit_with_output("second", &mut |line| {
            lines += 1;
            if line.stream == OutputStream::Stdout {
                stdout += 1;
            }
        })?;
        assert!(lines >= 10_000, "every line arrived: {lines}");
        assert!(stdout > 0, "and both pipes were drained: {stdout}");
        Ok(())
    }

    #[test]
    fn a_hook_that_reads_stdin_sees_eof_rather_than_the_terminal()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("stream-stdin")?;
        commit(&fixture, "one", "first")?;
        // Inheriting the editor's raw-mode terminal here would hang forever and
        // swallow the user's keystrokes while it did.
        hook(&fixture.0, "read line; echo \"read: $line\"")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let oid = repository.commit_with_output("second", &mut |_| {})?;
        assert_eq!(oid.len(), 40);
        Ok(())
    }
}
