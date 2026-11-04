//! Fetching folding ranges from a language's primary server.
//!
//! Shaped like [`semantic_tokens`](super::semantic_tokens): each open
//! document is asked at every version the server has been sent, off the
//! server task's serial path and at most once at a time, a newer version
//! aborting the fetch still in flight for an older one. The answer travels as
//! an [`LspUpdate::FoldingRanges`] stamped with the version asked, so the
//! session drops one the text has since moved past.
//!
//! Only a primary slot fetches -- a companion's folds would compete with the
//! primary's for one document -- and only for a document the server offers
//! folding ranges for. A request the server cancels (`ServerCancelled`) is
//! asked again a bounded number of times; any other failure leaves the
//! document with what it last had until its next version.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use karet_core::FoldRegions;
use karet_core::ServerFeature;
use karet_lsp::LspClient;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::task::JoinSet;

use super::commands::OpenDocument;
use super::message::LspUpdate;
use super::semantic_tokens::is_server_cancelled;
use super::slot::SlotKey;
use super::slot::SlotToken;

/// One connection's folding-range fetches.
pub(super) struct FoldFetches {
    /// Whether this slot fetches at all: it is its language's primary.
    enabled: bool,
    /// The version last asked for each open document.
    asked: HashMap<PathBuf, i32>,
    /// The fetch in flight for each document, to abort when superseded.
    in_flight: HashMap<PathBuf, AbortHandle>,
    /// Every launched fetch. Each holds the client, so they are stopped
    /// before a polite shutdown needs to own it outright.
    tasks: JoinSet<()>,
}

impl FoldFetches {
    /// Fetches for a slot that is (`primary`) or is not its language's
    /// primary server.
    pub(super) fn new(primary: bool) -> Self {
        Self {
            enabled: primary,
            asked: HashMap::new(),
            in_flight: HashMap::new(),
            tasks: JoinSet::new(),
        }
    }

    /// Fetch every open document whose sent version has not been asked yet,
    /// skipping the one with an edit still buffered (`pending`) and any
    /// awaiting a resync: the server has not seen the version an answer
    /// would be stamped with.
    pub(super) fn launch_ready(
        &mut self,
        client: &Arc<LspClient>,
        documents: &HashMap<PathBuf, OpenDocument>,
        pending: Option<&Path>,
        (updates, key, token): (&mpsc::UnboundedSender<LspUpdate>, &SlotKey, SlotToken),
    ) {
        if !self.enabled {
            return;
        }
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
                || !client.supports_for(ServerFeature::FoldingRange, path)
            {
                continue;
            }
            self.asked.insert(path.clone(), document.version);
            if let Some(superseded) = self.in_flight.remove(path) {
                superseded.abort();
            }
            let handle = self.tasks.spawn(fetch(
                Arc::clone(client),
                path.clone(),
                document.version,
                (updates.clone(), key.clone(), token),
            ));
            self.in_flight.insert(path.clone(), handle);
        }
    }

    /// Stop every fetch and forget what was asked: the connection they were
    /// asked on is gone, and its successor is asked afresh.
    pub(super) fn abandon(&mut self) {
        self.tasks.abort_all();
        self.asked.clear();
        self.in_flight.clear();
    }

    /// Stop every fetch and wait until each has let go of the client.
    pub(super) async fn shutdown(&mut self) {
        self.tasks.shutdown().await;
        self.asked.clear();
        self.in_flight.clear();
    }
}

/// How many times a request the server cancels is asked again before the
/// document waits for its next version.
const CANCELLED_RETRIES: usize = 3;

/// Fetch `path`'s folding ranges at `version` and relay them.
async fn fetch(
    client: Arc<LspClient>,
    path: PathBuf,
    version: i32,
    (updates, key, token): (mpsc::UnboundedSender<LspUpdate>, SlotKey, SlotToken),
) {
    let mut answer = client.folding_ranges(&path).await;
    for _ in 0..CANCELLED_RETRIES {
        match &answer {
            Err(error) if is_server_cancelled(error) => {
                answer = client.folding_ranges(&path).await;
            },
            _ => break,
        }
    }
    let folds: FoldRegions = match answer {
        Ok(folds) => folds,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "folding ranges failed");
            return;
        },
    };
    let _ = updates.send(LspUpdate::FoldingRanges {
        token,
        server: key,
        path,
        version,
        folds,
    });
}
