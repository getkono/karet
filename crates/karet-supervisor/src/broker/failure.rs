//! What a broker reports when its process never came up.
//!
//! The broker owns the server, so only it can see the child exit and what the
//! child said on the way out. Its own stderr goes to `/dev/null` — it is a
//! detached hidden process — so the report travels through a file beside the
//! endpoint, written with the same atomic-rename discipline.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;

use serde::Deserialize;
use serde::Serialize;

/// How long an unclaimed report is kept before a sweep may remove it.
///
/// Orders of magnitude above the connector's five-second deadline, so a sweep
/// can never race a report a client is still waiting for.
const MAX_REPORT_AGE: Duration = Duration::from_secs(60 * 60);

/// How many trailing stderr lines a report carries.
///
/// The same bound `karet_lsp::launch` keeps for the direct fork, so the two
/// forks describe one failure the same way. It is restated rather than shared
/// because the broker skeleton may not name `karet-lsp` (see [`super`]).
const STDERR_TAIL: usize = 20;

/// Why a brokered process never served.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrokeredLaunchFailure {
    /// The executable the broker was asked to run.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The broker's own description of what went wrong.
    pub message: String,
    /// The last lines the brokered process wrote to stderr, oldest first.
    ///
    /// The whole reason a failed launch is diagnosable: "connection closed"
    /// names no problem, "Cannot find module 'vscode-languageserver'" does.
    /// The broker's child is a grandchild of the editor and the broker's own
    /// stderr goes to `/dev/null`, so unless the tail travels in the report it
    /// is gone -- which is what left the brokered fork, the one the app always
    /// takes, reporting less than the direct one.
    ///
    /// `serde(default)` so a report written by a broker from another karet
    /// build still parses; a missing tail costs the diagnosis, not the report.
    #[serde(default)]
    pub stderr: Vec<String>,
    /// Whether the process ran at all before failing.
    ///
    /// The distinction the connector needs: a server that started and then
    /// ended its stdout is not coming back, whatever the retry policy hopes.
    pub ran: bool,
    /// Process id of the broker that wrote the report.
    ///
    /// Every broker for a key writes to the same path, so a report found there
    /// is not necessarily *this* launch's. Identity is what makes it
    /// attributable: `ran` is a permanent verdict, and crediting one attempt's
    /// death to another retires a server that never actually failed. A
    /// connector accepts a report only when this matches the broker it elected
    /// or observed, which it learns from the lease (see [`super::lease`]).
    pub pid: u32,
}

impl std::fmt::Display for BrokeredLaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.diagnosis() {
            Some(line) => write!(f, "{}: {line}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl BrokeredLaunchFailure {
    /// The server's own last words, when it said anything.
    ///
    /// The single most useful line about a failed launch, and the one the
    /// direct fork already leads with; a broker reporting only its own
    /// "connection closed" tells the user nothing they can act on.
    #[must_use]
    pub fn diagnosis(&self) -> Option<&str> {
        self.stderr
            .iter()
            .rev()
            .map(|line| line.trim())
            .find(|line| !line.is_empty())
    }
}

/// A bounded, shared view of the brokered child's most recent stderr.
///
/// Deliberately the same shape as `karet_lsp::launch::StderrTail`, restated
/// here because the broker skeleton may not name `karet-lsp`: only the lines
/// still in this window are kept, so a server that logs continuously cannot
/// grow the report without limit.
#[derive(Clone, Default)]
pub(crate) struct StderrTail(Arc<Mutex<VecDeque<String>>>);

impl StderrTail {
    /// Record one line, dropping the oldest once the window is full.
    pub(crate) fn push(&self, line: String) {
        // A poisoned lock costs the diagnosis, never the report.
        if let Ok(mut lines) = self.0.lock() {
            if lines.len() == STDERR_TAIL {
                lines.pop_front();
            }
            lines.push_back(line);
        }
    }

    /// The retained lines, oldest first.
    pub(crate) fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Where `write` stages the report before publishing it.
///
/// Process-qualified for the same reason [`super::endpoint::write_endpoint`]
/// does it: the name is shared by every broker for the key, and `std::fs::write`
/// truncates before it writes. Two brokers failing at once interleaved into one
/// fixed `{key}.error.tmp`, and the winning rename published mixed or truncated
/// bytes that no longer parsed — losing the report entirely and putting the
/// connector back on the full startup timeout the report exists to avoid.
fn staging_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or(OsStr::new("report"))
        .to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Whether `path` is a report, or a report's staging file.
fn is_report(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name.ends_with(".error") || (name.contains(".error.") && name.ends_with(".tmp"))
}

/// Record `failure` at `path`, replacing anything already there.
///
/// Best effort throughout: a broker that cannot write its report still has to
/// exit, and the caller falls back to its startup timeout. Every failing path
/// removes the staging file; one left by a broker that was killed mid-write is
/// the [`sweep`]'s to collect.
pub(crate) fn write(path: &Path, failure: &BrokeredLaunchFailure) {
    let Ok(encoded) = serde_json::to_vec(failure) else {
        return;
    };
    let staging = staging_path(path);
    if std::fs::write(&staging, &encoded).is_err() || std::fs::rename(&staging, path).is_err() {
        let _ = std::fs::remove_file(&staging);
    }
}

/// Read a report, if one is there and intact.
pub(crate) fn read(path: &Path) -> Option<BrokeredLaunchFailure> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Remove reports in `directory` that no connector will ever claim.
///
/// A report is otherwise unlinked only by the next connector for the *same*
/// key, and a user who edits their server's argv changes the key — so the old
/// key's report would sit in the registry directory forever. Bounded to one
/// directory listing and entirely best effort: a launch must never fail
/// because a leftover file could not be removed.
pub(crate) fn sweep(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_report(&path) {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_none_or(|age| age >= MAX_REPORT_AGE);
        if abandoned {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::FileTimes;

    use super::*;

    type BoxError = Box<dyn std::error::Error>;

    fn failure() -> BrokeredLaunchFailure {
        BrokeredLaunchFailure {
            command: "rust-analyzer".to_owned(),
            args: vec!["--stdio".to_owned()],
            message: "server stdout ended".to_owned(),
            stderr: vec!["Error: Cannot find module 'vscode-languageserver'".to_owned()],
            ran: true,
            pid: std::process::id(),
        }
    }

    /// Ages `path` so a sweep sees it as abandoned.
    fn age(path: &Path, by: Duration) -> Result<(), BoxError> {
        let file = std::fs::File::options().write(true).open(path)?;
        let when = SystemTime::now() - by;
        file.set_times(FileTimes::new().set_accessed(when).set_modified(when))?;
        Ok(())
    }

    /// The report is the only way the child's last words leave the broker, so
    /// what it carries is what the user is told. Without the tail the brokered
    /// fork could say no more than "connection closed" about a server that had
    /// explained itself perfectly well on stderr.
    #[test]
    fn a_report_leads_with_the_servers_own_last_words() {
        let failure = failure();
        assert_eq!(
            failure.diagnosis(),
            Some("Error: Cannot find module 'vscode-languageserver'")
        );
        assert_eq!(
            failure.to_string(),
            "server stdout ended: Error: Cannot find module 'vscode-languageserver'"
        );
    }

    /// A server that said nothing still reports what the broker saw.
    #[test]
    fn a_silent_server_leaves_the_brokers_own_description() {
        let mut failure = failure();
        failure.stderr = vec!["   ".to_owned(), String::new()];
        assert_eq!(failure.diagnosis(), None);
        assert_eq!(failure.to_string(), "server stdout ended");
    }

    /// A report written by another karet build has no `stderr` key at all.
    #[test]
    fn a_report_without_a_tail_still_parses() -> Result<(), BoxError> {
        let bare = br#"{"command":"gopls","args":[],"message":"gone","ran":true,"pid":7}"#;
        let parsed: BrokeredLaunchFailure = serde_json::from_slice(bare)?;
        assert!(parsed.stderr.is_empty());
        Ok(())
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

    #[test]
    fn a_report_round_trips() -> Result<(), BoxError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("key.error");
        write(&path, &failure());
        assert_eq!(read(&path), Some(failure()));
        Ok(())
    }

    /// The staging name is the broker's, not the key's: two brokers failing for
    /// one key must not write through the same file descriptor-shaped hole.
    #[test]
    fn staging_is_qualified_by_the_writing_process() {
        let staging = staging_path(Path::new("/state/key.error"));
        assert_eq!(
            staging.file_name().and_then(OsStr::to_str),
            Some(format!("key.error.{}.tmp", std::process::id()).as_str())
        );
    }

    /// Stands in for a sibling broker occupying the fixed staging name: with a
    /// shared `{key}.error.tmp` the write goes nowhere and the report is lost,
    /// which is the silent-loss half of the interleaving defect.
    #[test]
    fn a_sibling_holding_the_shared_staging_name_cannot_swallow_the_report() -> Result<(), BoxError>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("key.error");
        std::fs::create_dir(directory.path().join("key.error.tmp"))?;

        write(&path, &failure());
        assert_eq!(
            read(&path),
            Some(failure()),
            "a name another broker occupies must not cost this broker its report"
        );
        Ok(())
    }

    /// The rename cannot succeed onto a non-empty directory, so this exercises
    /// the failure path and proves it leaves no staging file behind.
    #[test]
    fn a_report_that_cannot_be_published_leaves_no_staging_file() -> Result<(), BoxError> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("key.error");
        std::fs::create_dir(&path)?;
        std::fs::write(path.join("occupant"), b"x")?;

        write(&path, &failure());
        assert!(!staging_path(&path).exists(), "the staging file leaked");
        Ok(())
    }

    #[test]
    fn a_sweep_removes_abandoned_reports_and_keeps_fresh_ones() -> Result<(), BoxError> {
        let directory = tempfile::tempdir()?;
        let fresh = directory.path().join("fresh.error");
        let stale = directory.path().join("stale.error");
        let staging = directory.path().join("stale.error.99.tmp");
        let endpoint = directory.path().join("stale.json");
        for path in [&fresh, &stale, &staging, &endpoint] {
            std::fs::write(path, b"{}")?;
        }
        for path in [&stale, &staging, &endpoint] {
            age(path, MAX_REPORT_AGE + Duration::from_secs(60))?;
        }

        sweep(directory.path());

        assert!(fresh.exists(), "a report a connector may still claim");
        assert!(!stale.exists(), "an abandoned report was kept");
        assert!(!staging.exists(), "an abandoned staging file was kept");
        assert!(endpoint.exists(), "only reports are this sweep's to remove");
        Ok(())
    }

    #[test]
    fn a_sweep_of_a_missing_directory_is_silent() {
        sweep(Path::new("/nonexistent/karet-broker-sweep"));
    }
}
