//! What a broker reports when its process never came up.
//!
//! The broker owns the server, so only it can see the child exit and what the
//! child said on the way out. Its own stderr goes to `/dev/null` — it is a
//! detached hidden process — so the report travels through a file beside the
//! endpoint, written with the same atomic-rename discipline.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;

/// Why a brokered process never served.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BrokeredLaunchFailure {
    /// The executable the broker was asked to run.
    pub command: String,
    /// Its arguments.
    pub args: Vec<String>,
    /// The broker's own description of what went wrong.
    pub message: String,
    /// Whether the process ran at all before failing.
    ///
    /// The distinction the connector needs: a server that started and then
    /// ended its stdout is not coming back, whatever the retry policy hopes.
    pub ran: bool,
}

impl std::fmt::Display for BrokeredLaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Record `failure` at `path`, replacing anything already there.
///
/// Best effort throughout: a broker that cannot write its report still has to
/// exit, and the caller falls back to its startup timeout.
pub(crate) fn write(path: &Path, failure: &BrokeredLaunchFailure) {
    let Ok(encoded) = serde_json::to_vec(failure) else {
        return;
    };
    let temporary = path.with_extension("error.tmp");
    if std::fs::write(&temporary, &encoded).is_ok() && std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// Read a report, if one is there and intact.
pub(crate) fn read(path: &Path) -> Option<BrokeredLaunchFailure> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
