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
//! # How cancellation works
//!
//! A commit is spawned as its own **process group**, so one signal reaches
//! `git` and every hook `git` started — killing `git` alone would leave a
//! formatter or a test suite running with nobody waiting on it. The group is
//! asked to stop with `SIGTERM` first, because `git` installs a signal handler
//! that unlinks its own lock files; only a group that ignores that (a shell
//! hook can `trap '' TERM`) is killed outright, after a grace period.
//!
//! Signalling the group also closes the pipes, which ends the drain loop below
//! on its own — cancellation costs the reading path nothing.
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
use std::path::Path;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use command_group::CommandGroup as _;
use command_group::GroupChild;

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

/// How long a cancelled commit is given to exit on its own before it is killed
/// outright. Long enough for a hook to finish a write and unwind; short enough
/// that a wedged commit does not hold the console open.
const TERMINATE_GRACE: Duration = Duration::from_secs(3);
/// How often the drain loop wakes to reconsider a cancellation.
const DRAIN_TICK: Duration = Duration::from_millis(100);
/// How long the pipes are drained after a cancellation before they are
/// abandoned. A process that escaped the group by double-forking can hold the
/// write end open forever; the commit is over regardless.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// How long the killed child is given to be reaped before its status is
/// treated as unknown. The outcome comes from `HEAD`, not from the status, so
/// giving up here loses nothing that matters.
const REAP_GRACE: Duration = Duration::from_secs(10);

/// A handle for stopping a running commit, usable from any thread.
///
/// Cancellation is *not* cooperative here: it signals the commit's whole
/// process group, so it reaches `git` and every hook `git` started. What it
/// cannot do is decide the outcome — see [`CommitOutcome`].
///
/// A token belongs to **one** commit. Cancellation is permanent once asked for,
/// so reusing a cancelled token would stop the next commit before it started.
#[derive(Clone, Debug, Default)]
pub struct CommitCancel(Arc<CancelState>);

#[derive(Debug, Default)]
struct CancelState {
    /// Set by the first [`CommitCancel::cancel`]; never cleared.
    requested: AtomicBool,
    /// The running process group, between spawn and reap.
    ///
    /// Every transition goes through this lock, which is what closes the race
    /// between a cancellation and a spawn: either the canceller sees the child
    /// and signals it, or the spawn sees the request and signals it itself.
    child: Mutex<Option<GroupChild>>,
}

impl CommitCancel {
    /// A handle for a commit that has not been asked to stop.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the running commit to stop.
    ///
    /// Idempotent, and safe before the commit starts, while it runs, and after
    /// it has finished. Returns as soon as the signal is delivered rather than
    /// waiting for the commit to die.
    pub fn cancel(&self) {
        if self.0.requested.swap(true, Ordering::SeqCst) {
            return;
        }
        self.0.terminate();
        // A hook is free to ignore a polite signal — `trap '' TERM` in a shell
        // hook does exactly that, and leaves git dead with its children still
        // running. Escalate once the grace period is up.
        let state = Arc::clone(&self.0);
        std::thread::spawn(move || {
            std::thread::sleep(TERMINATE_GRACE);
            state.kill_group();
        });
    }

    /// Whether stopping has been asked for.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.requested.load(Ordering::SeqCst)
    }
}

impl CancelState {
    /// Signal the group politely, so `git` runs its own cleanup.
    ///
    /// `git` removes its lock files from a signal handler it installs for
    /// SIGTERM among others, so this is what keeps a cancelled commit from
    /// leaving `.git/index.lock` behind. Windows has no equivalent — a job
    /// object can only be terminated — which is what the lock repair covers.
    fn terminate(&self) {
        if let Ok(mut slot) = self.child.lock() {
            terminate(&mut slot);
        }
    }

    /// Kill the group outright, for anything that ignored [`Self::terminate`].
    ///
    /// Unconditional: killing a group that has already exited simply fails, and
    /// checking first would only reintroduce the race it is meant to close.
    fn kill_group(&self) {
        if let Ok(mut slot) = self.child.lock()
            && let Some(child) = slot.as_mut()
        {
            let _ = child.kill();
        }
    }

    /// Take ownership of a freshly spawned group.
    ///
    /// Signals it straight away when cancellation arrived while the child was
    /// being spawned: the canceller looked for a child, found none, and left
    /// without signalling anything, so doing it is this side's job.
    fn adopt(&self, child: GroupChild) {
        let Ok(mut slot) = self.child.lock() else {
            return;
        };
        *slot = Some(child);
        if self.requested.load(Ordering::SeqCst) {
            terminate(&mut slot);
        }
    }

    /// Wait for the group to finish, sweeping it first when the run was cancelled.
    ///
    /// `try_wait` here reports the whole *group*, not just `git`: it answers
    /// `Some` only once every process in it has been reaped. A hook that ignored
    /// `SIGTERM` therefore keeps it pending long after `git` itself is gone —
    /// which is why a cancelled run ends with a kill rather than a wait. The
    /// polite signal has already had its chance, and nothing still running was
    /// asked to keep running.
    ///
    /// Polls rather than blocking in `wait`, so that holding the lock never
    /// stops [`Self::kill_group`] from being the thing that ends the wait.
    fn finish(&self, cancelled: bool) -> Option<ExitStatus> {
        if cancelled {
            self.kill_group();
        }
        let deadline = Instant::now() + REAP_GRACE;
        loop {
            if let Ok(mut slot) = self.child.lock()
                && let Some(child) = slot.as_mut()
            {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *slot = None;
                        return Some(status);
                    },
                    Ok(None) if Instant::now() < deadline => {},
                    // Out of time, or the group can no longer be waited on. The
                    // outcome comes from HEAD, so an unknown status loses nothing.
                    _ => {
                        let _ = child.kill();
                        *slot = None;
                        return None;
                    },
                }
            } else {
                return None;
            }
            std::thread::sleep(DRAIN_TICK);
        }
    }
}

/// Ask the group to stop, as politely as the platform allows.
fn terminate(slot: &mut Option<GroupChild>) {
    let Some(child) = slot.as_mut() else {
        return;
    };
    #[cfg(unix)]
    {
        use command_group::Signal;
        use command_group::UnixChildExt as _;
        let _ = child.signal(Signal::SIGTERM);
    }
    // A job object cannot be asked; it can only be ended.
    #[cfg(not(unix))]
    let _ = child.kill();
}

/// How a streamed commit ended.
///
/// Cancellation asks a running `git commit` to stop, but asking does not undo
/// what already happened: a request that lands while a `post-commit` hook is
/// running arrives after the commit exists. So the outcome is read from `HEAD`
/// rather than from what the caller intended, and a cancelled run that
/// nonetheless produced a commit reports [`Created`](Self::Created).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CommitOutcome {
    /// A commit was created, with its hex id.
    Created(String),
    /// No commit was created; the run was stopped.
    Cancelled,
}

impl Repository {
    /// Commit the staged changes, calling `on_line` for every line `git` and its
    /// hooks print, and reporting whether a commit was created.
    ///
    /// Every line reaches `on_line` before success or failure is decided, so a
    /// hook that fails after printing its diagnosis still delivers it.
    ///
    /// `cancel` can stop the run from another thread at any point; see
    /// [`CommitCancel`] for what that does and [`CommitOutcome`] for why it does
    /// not by itself decide the answer.
    pub fn commit_streaming(
        &self,
        message: &str,
        cancel: &CommitCancel,
        on_line: &mut dyn FnMut(CommitOutputLine),
    ) -> Result<CommitOutcome, VcsError> {
        // Read before anything runs: the only trustworthy account of whether a
        // commit happened is whether one exists afterwards that did not before.
        let head_before = self.head_id();
        let lock = self.commit_git_dir().join("index.lock");
        let lock_existed = lock.exists();

        if cancel.is_cancelled() {
            return Ok(CommitOutcome::Cancelled);
        }
        let mut child = self.spawn_commit(message)?;
        let (sender, receiver) = mpsc::channel();
        let readers = [
            reader(
                child.inner().stdout.take(),
                OutputStream::Stdout,
                sender.clone(),
            ),
            reader(child.inner().stderr.take(), OutputStream::Stderr, sender),
        ];
        // From here the group belongs to `cancel`, which signals it if a
        // cancellation arrived while it was being spawned.
        cancel.0.adopt(child);

        let mut stderr_tail = String::new();
        let mut abandoned = false;
        let mut cancelled_at: Option<Instant> = None;
        loop {
            match receiver.recv_timeout(DRAIN_TICK) {
                Ok(line) => {
                    if line.stream == OutputStream::Stderr && !line.text.trim().is_empty() {
                        stderr_tail = line.text.clone();
                    }
                    on_line(line);
                },
                // Both readers are done: the child's pipes are closed.
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !cancel.is_cancelled() {
                        continue;
                    }
                    // Give a cancelled commit a moment to flush what it had, then
                    // stop waiting: a process that escaped the group by
                    // double-forking holds the write end open indefinitely, and
                    // the commit is over either way.
                    if cancelled_at.get_or_insert_with(Instant::now).elapsed() >= DRAIN_GRACE {
                        abandoned = true;
                        break;
                    }
                },
            }
        }
        if abandoned {
            // Joining would wait on exactly the pipe that is not closing.
            drop(readers);
        } else {
            for reader in readers.into_iter().flatten() {
                let _ = reader.join();
            }
        }
        let status = cancel.0.finish(cancel.is_cancelled());
        if cancel.is_cancelled() {
            repair_index_lock(&lock, lock_existed);
        }

        // A cancellation that lands during `post-commit` arrives after the commit
        // exists. What HEAD says outranks what was asked for.
        let head_after = self.head_id();
        if let Some(head) = head_after
            && Some(&head) != head_before.as_ref()
        {
            return Ok(CommitOutcome::Created(head));
        }
        if cancel.is_cancelled() {
            return Ok(CommitOutcome::Cancelled);
        }
        let status =
            status.ok_or_else(|| VcsError::Git("git commit never finished".to_string()))?;
        if !status.success() {
            return Err(VcsError::Git(if stderr_tail.is_empty() {
                format!("git exited with {status}")
            } else {
                stderr_tail
            }));
        }
        self.head_id()
            .map(CommitOutcome::Created)
            .ok_or_else(|| VcsError::Git("commit succeeded but HEAD is unset".to_string()))
    }

    /// The commit `HEAD` names, or `None` on an unborn branch.
    fn head_id(&self) -> Option<String> {
        let output = self.git_output(["rev-parse", "HEAD"]).ok()?;
        if !output.status.success() {
            return None;
        }
        let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!head.is_empty()).then_some(head)
    }

    /// Start `git commit` in its own process group, pipes captured, stdin closed.
    fn spawn_commit(&self, message: &str) -> Result<GroupChild, VcsError> {
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
            .group_spawn()
            .map_err(|error| VcsError::GitUnavailable(error.to_string()))
    }
}

/// Remove `.git/index.lock` when a killed commit left one behind.
///
/// Only ever removes a lock *this* commit created. `git` takes that lock with
/// `O_EXCL`, so a competing holder would have made our own `git commit` fail
/// rather than let it run — a lock that was absent before the child started and
/// present after it died is ours. On unix `git` clears it from its own signal
/// handler, so this is a backstop for a forced kill, and for Windows, where a
/// job object can only be terminated and never asked.
fn repair_index_lock(lock: &Path, existed_before: bool) {
    if existed_before {
        return;
    }
    let _ = std::fs::remove_file(lock);
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
        let outcome =
            repository.commit_with_output("second", &CommitCancel::new(), &mut |line| {
                lines.push(line);
            })?;
        let CommitOutcome::Created(oid) = outcome else {
            unreachable!("an uncancelled commit lands");
        };
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
        let Err(error) =
            repository
                .commit_with_output("second", &CommitCancel::new(), &mut |line| lines.push(line))
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
        repository.commit_with_output("second", &CommitCancel::new(), &mut |line| {
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
        let outcome = repository.commit_with_output("second", &CommitCancel::new(), &mut |_| {})?;
        assert!(matches!(outcome, CommitOutcome::Created(oid) if oid.len() == 40));
        Ok(())
    }

    /// Install an executable hook named `name` running `body`.
    fn named_hook(repo: &std::path::Path, name: &str, body: &str) -> Result<(), VcsError> {
        use std::os::unix::fs::PermissionsExt as _;
        let relative = format!(".git/hooks/{name}");
        write(repo, &relative, format!("#!/bin/sh\n{body}\n").as_bytes())?;
        std::fs::set_permissions(repo.join(&relative), std::fs::Permissions::from_mode(0o755))
            .map_err(|error| VcsError::Git(error.to_string()))
    }

    /// Note the group's leader while it runs, then cancel. The pid is what lets a
    /// test ask whether the whole group actually went away.
    fn observe_group(token: &CommitCancel) -> std::thread::JoinHandle<u32> {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            let leader = token
                .0
                .child
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(GroupChild::id))
                .unwrap_or_default();
            token.cancel();
            leader
        })
    }

    /// Cancel `token` once the hook has had time to start.
    fn cancel_shortly(token: &CommitCancel) -> std::thread::JoinHandle<()> {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(700));
            token.cancel();
        })
    }

    /// Processes still sharing the group led by `leader`.
    fn group_members(leader: u32) -> usize {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-eo", "pid=,pgid="])
            .output()
        else {
            return 0;
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|row| row.split_whitespace().nth(1) == Some(&leader.to_string()))
            .count()
    }

    #[test]
    fn cancelling_a_running_hook_stops_the_commit_and_leaves_no_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("cancel-running")?;
        commit(&fixture, "one", "first")?;
        hook(&fixture.0, "echo working\nsleep 30")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let head_before = repository.head_id();
        let token = CommitCancel::new();
        let canceller = cancel_shortly(&token);
        let outcome = repository.commit_with_output("second", &token, &mut |_| {})?;
        let _ = canceller.join();

        assert_eq!(outcome, CommitOutcome::Cancelled, "no commit was created");
        assert_eq!(repository.head_id(), head_before, "HEAD did not move");
        assert!(
            !fixture.0.join(".git/index.lock").exists(),
            "a cancelled commit leaves no lock for the user to clear by hand"
        );
        Ok(())
    }

    #[test]
    fn cancelling_before_the_spawn_never_runs_git() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("cancel-early")?;
        commit(&fixture, "one", "first")?;
        // The hook would leave a trace if it ever ran.
        hook(&fixture.0, "touch .git/hook-ran")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let token = CommitCancel::new();
        token.cancel();
        let outcome = repository.commit_with_output("second", &token, &mut |_| {})?;

        assert_eq!(outcome, CommitOutcome::Cancelled);
        assert!(
            !fixture.0.join(".git/hook-ran").exists(),
            "a commit cancelled before it started never spawned git"
        );
        Ok(())
    }

    #[test]
    fn a_hook_that_ignores_the_polite_signal_is_killed_outright()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("cancel-stubborn")?;
        commit(&fixture, "one", "first")?;
        // `git` dies on SIGTERM regardless, but a hook that traps it survives and
        // keeps the whole group alive. Only the escalation clears this, and an
        // earlier version of this code leaked the hook: it dropped the group
        // handle as soon as git was reaped, leaving nothing left to kill with.
        hook(&fixture.0, "trap '' TERM\necho stubborn\nsleep 47")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let token = CommitCancel::new();
        let leader = observe_group(&token);
        let started = std::time::Instant::now();
        let outcome = repository.commit_with_output("second", &token, &mut |_| {})?;
        let leader = leader.join().unwrap_or_default();

        assert_eq!(outcome, CommitOutcome::Cancelled);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(25),
            "the escalation ended it rather than waiting out the hook: {:?}",
            started.elapsed()
        );
        assert!(leader > 0, "the group leader was observed while it ran");
        assert_eq!(
            group_members(leader),
            0,
            "the hook that ignored SIGTERM was killed rather than left running"
        );
        assert!(!fixture.0.join(".git/index.lock").exists());
        Ok(())
    }

    #[test]
    fn a_cancellation_that_arrives_too_late_reports_the_commit_it_could_not_stop()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = init("cancel-late")?;
        commit(&fixture, "one", "first")?;
        // `post-commit` runs *after* the commit exists, so cancelling here
        // cannot undo it. Reporting `Cancelled` would be a lie.
        named_hook(&fixture.0, "post-commit", "sleep 30")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let head_before = repository.head_id();
        let token = CommitCancel::new();
        let canceller = cancel_shortly(&token);
        let outcome = repository.commit_with_output("second", &token, &mut |_| {})?;
        let _ = canceller.join();

        let CommitOutcome::Created(oid) = outcome else {
            unreachable!("the commit existed before the cancellation arrived");
        };
        assert_eq!(oid.len(), 40);
        assert_ne!(repository.head_id(), head_before, "HEAD moved");
        Ok(())
    }

    #[test]
    fn cancelling_reaps_the_whole_hook_tree_not_just_git() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = init("cancel-tree")?;
        commit(&fixture, "one", "first")?;
        // A hook with children of its own: killing `git` alone would leave
        // these running with nobody waiting on them.
        hook(&fixture.0, "sleep 30 &\nsleep 30 &\necho spawned\nsleep 30")?;
        stage(&fixture.0, "two")?;

        let repository = Repository::discover(&fixture.0)?;
        let token = CommitCancel::new();
        let leader = observe_group(&token);
        let outcome = repository.commit_with_output("second", &token, &mut |_| {})?;
        let leader = leader.join().unwrap_or_default();

        assert_eq!(outcome, CommitOutcome::Cancelled);
        assert!(leader > 0, "the group leader was observed while it ran");
        assert_eq!(
            group_members(leader),
            0,
            "every process in the commit's group is gone"
        );
        Ok(())
    }
}
