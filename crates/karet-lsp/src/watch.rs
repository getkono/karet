//! Watched files: which file changes a server asked to hear about, and the
//! notification that tells it.
//!
//! A server never advertises file watchers at the handshake -- the protocol
//! has no static form for them. It registers `workspace/didChangeWatchedFiles`
//! dynamically with a list of `FileSystemWatcher`s, each a glob (or a glob
//! relative to a base folder) and the kinds of change it wants. This client
//! keeps those per registration, so unregistering one drops exactly its
//! watchers, and [`LspClient::did_change_watched_files`] sends a consumer's
//! file changes filtered through them. Watching the disk is the consumer's
//! (karet's session uses `karet-watch`); deciding what the server hears is
//! this crate's, because the globs are protocol.

use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;

use crate::LspClient;
use crate::LspError;
use crate::selector::Pattern;
use crate::uri;

/// What happened to a watched file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileChangeKind {
    /// The file was created.
    Created,
    /// The file's contents changed.
    Changed,
    /// The file was deleted.
    Deleted,
}

impl FileChangeKind {
    /// The `WatchKind` bit a watcher sets to hear about this change.
    fn watch_bit(self) -> u64 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 4,
        }
    }

    /// The `FileChangeType` on the wire.
    fn wire(self) -> u8 {
        match self {
            Self::Created => 1,
            Self::Changed => 2,
            Self::Deleted => 3,
        }
    }
}

/// One file change the consumer observed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileChange {
    /// The file, absolute.
    pub path: PathBuf,
    /// What happened to it.
    pub kind: FileChangeKind,
}

impl FileChange {
    /// A change of `kind` to `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, kind: FileChangeKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

/// One `FileSystemWatcher` from a registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Watcher {
    pattern: Pattern,
    /// The `WatchKind` bits; every kind when the server leaves it out.
    kinds: u64,
}

impl Watcher {
    /// Read `registerOptions.watchers`, dropping any watcher whose glob karet
    /// cannot use (it would otherwise match nothing, silently).
    pub(crate) fn from_register_options(options: Option<&Value>) -> Vec<Self> {
        options
            .and_then(|options| options.get("watchers"))
            .and_then(Value::as_array)
            .map(|watchers| {
                watchers
                    .iter()
                    .filter_map(|watcher| {
                        Some(Self {
                            pattern: Pattern::parse(watcher.get("globPattern")?)?,
                            kinds: watcher.get("kind").and_then(Value::as_u64).unwrap_or(7),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this watcher wants to hear about `kind` of change to `path`.
    pub(crate) fn wants(&self, path: &Path, kind: FileChangeKind) -> bool {
        self.kinds & kind.watch_bit() != 0 && self.pattern.matches(path)
    }
}

impl LspClient {
    /// Send the changes in `changes` that a registered watcher asked for, as
    /// one `workspace/didChangeWatchedFiles`, returning how many were sent.
    ///
    /// Nothing is sent when no watcher wants any of them, so a consumer can
    /// hand every change it sees to every server.
    ///
    /// # Errors
    ///
    /// [`LspError::Closed`] when the connection is gone, or
    /// [`LspError::Protocol`] when the outbound queue is full or a path cannot
    /// form a `file://` URI.
    pub fn did_change_watched_files(&self, changes: &[FileChange]) -> Result<usize, LspError> {
        let wanted: Vec<&FileChange> = {
            let Ok(gate) = self.gate.read() else {
                return Ok(0);
            };
            changes
                .iter()
                .filter(|change| gate.watches(&change.path, change.kind))
                .collect()
        };
        if wanted.is_empty() {
            return Ok(0);
        }
        let events = wanted
            .iter()
            .map(|change| {
                Ok(serde_json::json!({
                    "uri": uri::path_to_uri(&change.path)?.as_str(),
                    "type": change.kind.wire(),
                }))
            })
            .collect::<Result<Vec<Value>, LspError>>()?;
        let sent = events.len();
        self.conn.notify(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({ "changes": events }),
        )?;
        Ok(sent)
    }
}
