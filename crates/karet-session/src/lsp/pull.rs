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

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use karet_core::ServerFeature;
use karet_lsp::LspClient;
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
    /// The pull in flight for each document, to abort when it is superseded.
    in_flight: HashMap<PathBuf, AbortHandle>,
    /// Every launched pull. Each holds the client, so they are stopped before
    /// a polite shutdown needs to own it outright.
    tasks: JoinSet<()>,
    /// The connection's registration changes, subscribed on its first launch:
    /// a registration arriving after a document was skipped for want of one
    /// wakes the server task to ask again.
    registrations: Option<watch::Receiver<()>>,
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
        while self.tasks.try_join_next().is_some() {}
        self.asked.retain(|path, _| documents.contains_key(path));
        self.in_flight.retain(|path, handle| {
            let open = documents.contains_key(path);
            if !open {
                handle.abort();
            }
            open && !handle.is_finished()
        });
        for (path, document) in documents {
            if pending == Some(path.as_path())
                || document.resync
                || self.asked.get(path) == Some(&document.version)
                // Asked again once a registration covers it.
                || !client.supports_for(ServerFeature::PullDiagnostics, path)
            {
                continue;
            }
            self.asked.insert(path.clone(), document.version);
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
        self.asked.clear();
        self.in_flight.clear();
        self.registrations = None;
    }

    /// Stop every pull and wait until each has let go of the client.
    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.asked.clear();
        self.in_flight.clear();
        self.registrations = None;
    }

    /// Resolve when the connection's registrations change; never, before the
    /// first launch or once the connection has gone.
    pub(super) async fn registered(&mut self) {
        let changed = match self.registrations.as_mut() {
            Some(registrations) => registrations.changed().await.is_ok(),
            None => false,
        };
        if !changed {
            std::future::pending::<()>().await;
        }
    }
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
