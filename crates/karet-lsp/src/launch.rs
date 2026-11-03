//! What is known about a language-server launch that never reached a working
//! connection.
//!
//! A launch can fail in ways that need completely different responses -- the
//! binary is absent, it is present but not executable, it ran and exited
//! immediately, or the host could not even prepare the process -- and the only
//! thing that distinguishes them for a user is usually the server's own stderr.
//! That output is otherwise logged at `debug` and discarded, so the failure is
//! carried here instead: the argv, the exit status, and a bounded tail of what
//! the process said on its way out.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

/// How many trailing stderr lines a failure carries.
///
/// Enough for a stack-ish preamble plus the line that matters, bounded so a
/// server that logs continuously cannot grow this without limit.
const STDERR_TAIL: usize = 20;

/// The kind of launch failure, chosen so a caller can decide whether retrying
/// could ever help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LaunchCause {
    /// The executable was not found.
    NotFound,
    /// The executable was found but could not be run.
    PermissionDenied,
    /// The process started and then exited before or during the handshake.
    Exited,
    /// The process started, stayed alive, and never answered the handshake
    /// within the request deadline.
    ///
    /// Distinct from [`Self::Exited`] because the server is still there: it
    /// may simply be a large workspace being indexed, and writing it off for
    /// the rest of the session is the wrong answer.
    Timeout,
    /// The process started but exposed no piped standard I/O.
    NoStdio,
    /// The process could not be started for some other I/O reason.
    Io,
    /// The failure happened in the host before the process ran at all -- no
    /// supervisor, or a broker that could not be reached.
    Host,
    /// This host cannot run language servers at all.
    ///
    /// Distinct from [`Self::Host`], which is a host problem that may be over
    /// by the next attempt. This one is a property of the build: a karet
    /// without its process supervisor has no way to start a server, and that
    /// is settled for the life of the manager, so retrying is pure noise --
    /// which is exactly what it was, a "retrying with backoff" toast repeated
    /// until the restart circuit opened and then every five minutes after.
    Unsupported,
}

impl LaunchCause {
    /// Whether another attempt could plausibly succeed.
    ///
    /// A binary that is absent or unusable, and a server that exits on sight,
    /// will do the same thing on every retry; the caller stops rather than
    /// respawning forever. A server that is merely slow to answer is the
    /// opposite case -- it is running, so the next attempt may well land.
    #[must_use]
    pub const fn is_permanent(self) -> bool {
        matches!(
            self,
            Self::NotFound
                | Self::PermissionDenied
                | Self::NoStdio
                | Self::Exited
                | Self::Unsupported
        )
    }

    const fn describe(self) -> &'static str {
        match self {
            Self::NotFound => "was not found",
            Self::PermissionDenied => "is not executable",
            Self::Exited => "exited immediately",
            Self::Timeout => "did not answer the handshake",
            Self::NoStdio => "exposed no usable standard I/O",
            Self::Io => "could not be started",
            Self::Host => "could not be launched",
            Self::Unsupported => "cannot be launched by this build",
        }
    }
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExitReport {
    /// It exited with this status code.
    Code(i32),
    /// It was terminated by this signal (unix only).
    Signal(i32),
}

impl std::fmt::Display for ExitReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code(code) => write!(f, "exit {code}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
        }
    }
}

impl ExitReport {
    pub(crate) fn from_status(status: std::process::ExitStatus) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Some(Self::Signal(signal));
            }
        }
        status.code().map(Self::Code)
    }
}

/// Everything known about a launch that did not produce a working connection.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LaunchFailure {
    /// The executable karet tried to run.
    pub command: String,
    /// The arguments it was given.
    pub args: Vec<String>,
    /// What went wrong.
    pub cause: LaunchCause,
    /// How the process ended, when it ran at all.
    pub exit: Option<ExitReport>,
    /// The last lines the process wrote to stderr, oldest first.
    pub stderr: Vec<String>,
    /// Host-level detail for a failure that happened before the process ran.
    pub detail: Option<String>,
}

impl LaunchFailure {
    /// A failure with nothing yet known beyond its kind.
    ///
    /// The struct is `#[non_exhaustive]`, so this is how a consumer (a test
    /// double, another host) builds one; [`Self::with_exit`] and
    /// [`Self::with_stderr`] add the evidence when there is any.
    #[must_use]
    pub fn new(command: impl Into<String>, args: Vec<String>, cause: LaunchCause) -> Self {
        Self {
            command: command.into(),
            args,
            cause,
            exit: None,
            stderr: Vec::new(),
            detail: None,
        }
    }

    /// Record how the process ended.
    #[must_use]
    pub fn with_exit(mut self, exit: Option<ExitReport>) -> Self {
        self.exit = exit;
        self
    }

    /// Record the tail of what the process wrote to stderr, oldest first.
    #[must_use]
    pub fn with_stderr(mut self, stderr: Vec<String>) -> Self {
        self.stderr = stderr;
        self
    }

    /// Record host-level detail: what the host itself could not do.
    ///
    /// The counterpart to [`Self::with_stderr`] for a failure with no process
    /// to have said anything -- [`Self::diagnosis`] falls back to it -- so a
    /// cause that is not [`LaunchCause::Host`] can still carry one line of
    /// explanation instead of only its classification.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A failure that happened in the host, before any process was started.
    #[must_use]
    pub fn host(command: impl Into<String>, args: Vec<String>, detail: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            args,
            cause: LaunchCause::Host,
            exit: None,
            stderr: Vec::new(),
            detail: Some(detail.into()),
        }
    }

    /// The most specific line available about why this failed.
    ///
    /// The server's own last words if it said anything, since those name the
    /// actual problem ("Cannot find module", "unrecognized subcommand") far
    /// better than any classification here can.
    #[must_use]
    pub fn diagnosis(&self) -> String {
        if let Some(line) = self
            .stderr
            .iter()
            .rev()
            .find(|line| !line.trim().is_empty())
        {
            return line.trim().to_owned();
        }
        if let Some(detail) = &self.detail {
            return detail.clone();
        }
        match self.exit {
            Some(exit) => exit.to_string(),
            None => self.cause.describe().to_owned(),
        }
    }

    /// The command and arguments as they would be typed.
    #[must_use]
    pub fn command_line(&self) -> String {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "'{}' {}", self.command_line(), self.cause.describe())?;
        if let Some(exit) = self.exit {
            write!(f, " ({exit})")?;
        }
        let diagnosis = self.diagnosis();
        if diagnosis != self.cause.describe() {
            write!(f, ": {diagnosis}")?;
        }
        Ok(())
    }
}

/// A bounded, shared view of a child's most recent stderr.
///
/// The drain task keeps logging every line; this retains only the tail, so the
/// lines are still available at the moment a failure has to be described.
#[derive(Clone, Default)]
pub(crate) struct StderrTail(Arc<Mutex<VecDeque<String>>>);

impl StderrTail {
    pub(crate) fn push(&self, line: String) {
        // A poisoned lock costs the diagnosis, never the launch.
        if let Ok(mut lines) = self.0.lock() {
            if lines.len() == STDERR_TAIL {
                lines.pop_front();
            }
            lines.push_back(line);
        }
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_leads_with_the_servers_own_last_words() {
        let failure = LaunchFailure::new(
            "node",
            vec!["cli.js".to_owned(), "start".to_owned()],
            LaunchCause::Exited,
        )
        .with_exit(Some(ExitReport::Code(1)))
        .with_stderr(vec![
            "some/path/cli.js:3".to_owned(),
            "Error: Cannot find module 'vscode-languageserver'".to_owned(),
        ]);
        assert_eq!(failure.command_line(), "node cli.js start");
        assert_eq!(
            failure.diagnosis(),
            "Error: Cannot find module 'vscode-languageserver'"
        );
        assert_eq!(
            failure.to_string(),
            "'node cli.js start' exited immediately (exit 1): Error: Cannot find module \
             'vscode-languageserver'"
        );
    }

    #[test]
    fn a_silent_failure_still_says_what_kind_it_was() {
        let failure = LaunchFailure::new("gopls", Vec::new(), LaunchCause::NotFound);
        assert_eq!(failure.to_string(), "'gopls' was not found");
    }

    #[test]
    fn a_host_failure_carries_its_own_detail() {
        let failure = LaunchFailure::host("taplo", vec!["lsp".to_owned()], "no supervisor");
        assert_eq!(failure.diagnosis(), "no supervisor");
        assert!(!failure.cause.is_permanent());
    }

    /// A retry cannot conjure a missing binary, but a broker that was briefly
    /// unreachable may well answer next time.
    #[test]
    fn only_failures_a_retry_cannot_fix_are_permanent() {
        for cause in [
            LaunchCause::NotFound,
            LaunchCause::PermissionDenied,
            LaunchCause::NoStdio,
            LaunchCause::Exited,
        ] {
            assert!(cause.is_permanent(), "{cause:?}");
        }
        for cause in [LaunchCause::Io, LaunchCause::Host, LaunchCause::Timeout] {
            assert!(!cause.is_permanent(), "{cause:?}");
        }
    }

    /// A host with no supervisor is not a host that is momentarily unwell: the
    /// supervisor path is fixed when the manager is built, so every retry runs
    /// the identical impossible launch. Reported as `Host` it produced
    /// "Starting, Retrying, Retrying, Retrying, Retrying" and then a five-minute
    /// circuit repeating for the session.
    #[test]
    fn a_host_that_can_never_run_a_server_is_permanent() {
        let failure = LaunchFailure::new("rust-analyzer", Vec::new(), LaunchCause::Unsupported)
            .with_detail("this karet build has no process supervisor");
        assert!(failure.cause.is_permanent());
        assert_eq!(
            failure.diagnosis(),
            "this karet build has no process supervisor"
        );
        assert_eq!(
            failure.to_string(),
            "'rust-analyzer' cannot be launched by this build: this karet build has no \
             process supervisor"
        );
    }

    /// A server that is alive but slow to answer `initialize` is described as
    /// silent, not as dead, and is left a retry.
    #[test]
    fn a_handshake_that_timed_out_is_not_a_dead_server() {
        let failure = LaunchFailure::new("gopls", vec!["serve".to_owned()], LaunchCause::Timeout)
            .with_stderr(vec!["loading packages".to_owned()]);
        assert!(!failure.cause.is_permanent());
        assert_eq!(failure.exit, None);
        assert_eq!(
            failure.to_string(),
            "'gopls serve' did not answer the handshake: loading packages"
        );
    }

    #[test]
    fn the_stderr_tail_keeps_the_most_recent_lines() {
        let tail = StderrTail::default();
        for line in 0..(STDERR_TAIL + 5) {
            tail.push(line.to_string());
        }
        let lines = tail.lines();
        assert_eq!(lines.len(), STDERR_TAIL);
        assert_eq!(lines.first().map(String::as_str), Some("5"));
        assert_eq!(
            lines.last().map(String::as_str),
            Some((STDERR_TAIL + 4).to_string().as_str())
        );
    }
}
