//! Pulling diagnostics from a server that answers `textDocument/diagnostic`
//! rather than (or as well as) publishing them.
//!
//! A pull is asked for every open document at each version the server has
//! been sent, and its answer joins the stream a publication takes: an
//! [`LspUpdate::Diagnostics`] stamped with the version that was pulled, so the
//! session drops one that arrives after the text moved on, exactly as it drops
//! a stale publication. It is marked [`DiagnosticFeed::Pulled`], so it
//! replaces only the pulled layer: a server that also publishes (rust-analyzer
//! keeps publishing `cargo check`'s results) keeps its pushed set beside it.
//!
//! Pulls run as their own tasks, off the server task's serial path, and at
//! most one per document: a newer version aborts the pull still in flight for
//! an older one, whose answer could only be dropped. Their failures never
//! count against the connection's health -- a server still computing may
//! cancel a pull, and the next edit asks again.
//!
//! Two things re-pull a document whose own version has not moved. A server
//! whose provider declares `interFileDependencies` says an edit in one file
//! can change another's diagnostics, so a new version of any document it
//! covers pulls every open document again. And `workspace/diagnostic/refresh`
//! marks every version asked as stale, so every open document is pulled again
//! -- the server saw a project-wide change (a dependency edit, a configuration
//! reload) that no document's version records. The versions themselves are
//! kept, so an edit buffered across the refresh still reads as an edit when it
//! is flushed, and still re-pulls the documents that depend on it.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use karet_core::ServerFeature;
use karet_lsp::LspClient;
use karet_lsp::ServerRefresh;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use super::commands::OpenDocument;
use super::message::DiagnosticFeed;
use super::message::LspUpdate;
use super::slot::SlotKey;
use super::slot::SlotToken;

/// One connection's pulls: what was asked, and what is still in flight.
#[derive(Default)]
pub(super) struct Pulls {
    /// The version last pulled for each open document on this connection.
    asked: HashMap<PathBuf, i32>,
    /// The documents whose last pull a diagnostic refresh made stale: pulled
    /// again at their current version, whether or not it moved.
    refreshed: HashSet<PathBuf>,
    /// The pull in flight for each document, to abort when it is superseded.
    in_flight: HashMap<PathBuf, AbortHandle>,
    /// Every launched pull. Each holds the client, so they are stopped before
    /// a polite shutdown needs to own it outright.
    tasks: JoinSet<()>,
    /// The connection's registration changes, subscribed on its first launch:
    /// a registration arriving after a document was skipped for want of one
    /// wakes the server task to ask again.
    registrations: Option<watch::Receiver<()>>,
    /// The connection's refresh requests, subscribed on its first launch.
    refreshes: Option<broadcast::Receiver<ServerRefresh>>,
}

impl Pulls {
    /// Pull every open document whose sent version has not been pulled yet.
    ///
    /// `pending` names the document with an edit still buffered: the server
    /// has not seen its latest version, so pulling it now would stamp an
    /// answer about older text with the newer version. A document whose last
    /// change failed to send is skipped for the same reason until a change
    /// resyncs it.
    pub(super) fn launch_ready(
        &mut self,
        client: &Arc<LspClient>,
        documents: &HashMap<PathBuf, OpenDocument>,
        pending: Option<&Path>,
        (updates, key, token): (&mpsc::UnboundedSender<LspUpdate>, &SlotKey, SlotToken),
    ) {
        // Subscribed before any gate is read below, so a registration either
        // is already in the gate or marks this receiver.
        self.registrations
            .get_or_insert_with(|| client.registrations());
        self.refreshes.get_or_insert_with(|| client.refreshes());
        while self.tasks.try_join_next().is_some() {}
        self.asked.retain(|path, _| documents.contains_key(path));
        self.refreshed.retain(|path| documents.contains_key(path));
        self.in_flight.retain(|path, handle| {
            let open = documents.contains_key(path);
            if !open {
                handle.abort();
            }
            open && !handle.is_finished()
        });
        let ready = |path: &PathBuf, document: &OpenDocument| {
            pending != Some(path.as_path())
                && !document.resync
                // Asked again once a registration covers it.
                && client.supports_for(ServerFeature::PullDiagnostics, path)
        };
        // An edit -- a new version of a document already pulled, not a first
        // open -- to a file whose diagnostics have inter-file dependencies
        // may have changed every other open document's too.
        let dependents_stale = documents.iter().any(|(path, document)| {
            ready(path, document)
                && self
                    .asked
                    .get(path)
                    .is_some_and(|asked| *asked != document.version)
                && client.supports_for(ServerFeature::InterFileDiagnostics, path)
        });
        for (path, document) in documents {
            if !ready(path, document)
                || (!dependents_stale
                    && !self.refreshed.contains(path)
                    && self.asked.get(path) == Some(&document.version))
            {
                continue;
            }
            self.asked.insert(path.clone(), document.version);
            self.refreshed.remove(path);
            if let Some(superseded) = self.in_flight.remove(path) {
                superseded.abort();
            }
            let handle = self.tasks.spawn(pull(
                Arc::clone(client),
                path.clone(),
                document.version,
                (updates.clone(), key.clone(), token),
            ));
            self.in_flight.insert(path.clone(), handle);
        }
    }

    /// Stop every pull and forget what was asked: the connection they were
    /// asked on is gone, and its successor is asked afresh.
    pub(super) fn abandon(&mut self) {
        self.tasks.abort_all();
        self.forget();
    }

    /// Stop every pull and wait until each has let go of the client.
    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.forget();
    }

    fn forget(&mut self) {
        self.asked.clear();
        self.refreshed.clear();
        self.in_flight.clear();
        self.registrations = None;
        self.refreshes = None;
    }

    /// Resolve when a document skipped or already pulled may need pulling
    /// now: the connection's registrations changed, or the server asked for
    /// a diagnostic refresh, which marks every version asked as stale.
    /// Never, before the first launch or once the connection has gone.
    ///
    /// Cancel-safe, as the server task's `select!` needs: each arm is, and
    /// nothing is forgotten until an arm has completed.
    pub(super) async fn woken(&mut self) {
        loop {
            let wake = {
                let Self {
                    registrations,
                    refreshes,
                    ..
                } = &mut *self;
                let registered = async {
                    match registrations.as_mut() {
                        Some(registrations) => registrations.changed().await.is_ok(),
                        None => std::future::pending().await,
                    }
                };
                let refresh = async {
                    match refreshes.as_mut() {
                        Some(refreshes) => refreshes.recv().await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    changed = registered => if changed {
                        PullWake::Registered
                    } else {
                        PullWake::RegistrationsClosed
                    },
                    received = refresh => match received {
                        // A lag lost only duplicates of signals that might
                        // each have been this one, so it is taken as one.
                        Ok(ServerRefresh::Diagnostics)
                        | Err(broadcast::error::RecvError::Lagged(_)) => PullWake::Refresh,
                        Ok(_) => PullWake::Ignore,
                        Err(broadcast::error::RecvError::Closed) => PullWake::RefreshesClosed,
                    },
                }
            };
            match wake {
                PullWake::Registered => return,
                PullWake::Refresh => {
                    // Marked rather than forgotten: a document whose edit is
                    // still buffered keeps the version it was last pulled at,
                    // so its flush is an edit to `launch_ready`, and re-pulls
                    // the documents whose diagnostics depend on it.
                    self.refreshed.extend(self.asked.keys().cloned());
                    return;
                },
                PullWake::Ignore => {},
                // The connection's handler is gone, and every wake with it.
                PullWake::RegistrationsClosed => self.registrations = None,
                PullWake::RefreshesClosed => self.refreshes = None,
            }
        }
    }
}

/// What woke [`Pulls::woken`].
enum PullWake {
    /// A capability was registered or withdrawn.
    Registered,
    /// The server asked to re-pull every document.
    Refresh,
    /// The server asked to refresh something else.
    Ignore,
    /// The registration stream ended with its connection.
    RegistrationsClosed,
    /// The refresh stream ended with its connection.
    RefreshesClosed,
}

/// Pull `path`'s diagnostics at `version` and relay what the server said.
async fn pull(
    client: Arc<LspClient>,
    path: PathBuf,
    version: i32,
    (updates, key, token): (mpsc::UnboundedSender<LspUpdate>, SlotKey, SlotToken),
) {
    // No previous result id is handed back: an "unchanged" answer would be
    // about a set the session may have dropped as stale, leaving nothing to
    // keep showing.
    let pulled = match client.pull_diagnostics(&path, None).await {
        Ok(pulled) => pulled,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "diagnostic pull failed");
            return;
        },
    };
    if let Some(diagnostics) = pulled.diagnostics {
        let _ = updates.send(LspUpdate::Diagnostics {
            token,
            server: key.clone(),
            path,
            version: Some(version),
            feed: DiagnosticFeed::Pulled,
            diagnostics,
        });
    }
    for related in pulled.related {
        let _ = updates.send(LspUpdate::Diagnostics {
            token,
            server: key.clone(),
            path: related.path,
            version: related.version,
            feed: DiagnosticFeed::Pulled,
            diagnostics: related.diagnostics,
        });
    }
}
