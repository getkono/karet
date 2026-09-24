//! Inlay-hint requests, kept off the server task's serial path.
//!
//! Every other request a server task handles is awaited in line: it flushes
//! the pending `didChange`, issues the request, and waits for the reply before
//! reading its next command. That is right for completion and hover, which the
//! user is waiting on and which must see the latest text. It was wrong for
//! inlay hints, on both counts. A hint request is background work the editor
//! issues on its own, and awaiting it in line put every completion and hover
//! behind a server's slowest inference pass. And flushing for it defeated the
//! `didChange` debounce -- the editor asks for hints after edits, so each
//! request forced out the edit the debounce was holding.
//!
//! So a hint request here is *deferred* until the server has the text it was
//! asked against, then *launched* as its own task. The first half is what
//! keeps it correct without flushing: the task holds at most one un-flushed
//! edit, for one path. A request for any other path is asked against text the
//! server already has and launches at once; one for the pending path waits
//! for the flush the debounce was going to do anyway. The second half is what
//! keeps it out of the way: its answer comes back through the task's wake
//! loop, so the connection's health still hears about it.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use karet_core::InlayHint;
use karet_core::Range;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::health::FailureTally;
use super::message::LspUpdate;
use super::slot::SlotKey;
use super::slot::SlotToken;
use crate::api::DocumentId;
use crate::api::RequestId;

/// Who a hint request answers: echoed on the reply whatever becomes of it.
#[derive(Clone, Copy, Debug)]
pub(super) struct HintTag {
    /// The originating request.
    pub(super) request: RequestId,
    /// The target document.
    pub(super) doc: DocumentId,
    /// The buffer version the request was made against.
    pub(super) version: u64,
}

/// A hint request waiting for the server to have its text.
pub(super) struct HintAsk {
    /// What the answer echoes.
    pub(super) tag: HintTag,
    /// The document path.
    pub(super) path: PathBuf,
    /// The range, already in UTF-16 columns.
    pub(super) range: Range,
}

/// A launched request's outcome.
pub(super) struct HintAnswer {
    /// What the answer echoes.
    pub(super) tag: HintTag,
    /// The server's answer, or why there is none.
    pub(super) result: Result<Vec<InlayHint>, LspError>,
}

/// The hint requests one server task has accepted and not yet answered.
#[derive(Default)]
pub(super) struct HintFlight {
    /// Asked against text the server has not been sent yet.
    deferred: Vec<HintAsk>,
    /// Issued, and awaiting the server.
    tasks: JoinSet<HintAnswer>,
    /// What each running task answers, so one that fails to finish is still
    /// answered.
    running: HashMap<tokio::task::Id, HintTag>,
}

impl HintFlight {
    /// Accept a request, superseding any older one for the same document that
    /// has not been launched yet.
    ///
    /// The superseded one is answered empty rather than dropped: every request
    /// is answered. Its asker has already moved on -- it asked again -- so the
    /// empty set is discarded there rather than painted.
    pub(super) fn ask(
        &mut self,
        ask: HintAsk,
        updates: &mpsc::UnboundedSender<LspUpdate>,
        generation: u64,
    ) {
        let (superseded, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deferred)
            .into_iter()
            .partition(|older| older.tag.doc == ask.tag.doc);
        self.deferred = kept;
        for older in superseded {
            answer_empty(updates, older.tag, generation);
        }
        self.deferred.push(ask);
    }

    /// Launch every deferred request the server can now answer correctly: all
    /// of them but those for `pending`, the one path with an edit not yet sent.
    pub(super) fn launch_ready(&mut self, client: &Arc<LspClient>, pending: Option<&Path>) {
        let (waiting, ready): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deferred)
            .into_iter()
            .partition(|ask| pending == Some(ask.path.as_path()));
        self.deferred = waiting;
        for ask in ready {
            let client = Arc::clone(client);
            let tag = ask.tag;
            let handle = self.tasks.spawn(async move {
                HintAnswer {
                    tag,
                    result: client.inlay_hints(&ask.path, ask.range).await,
                }
            });
            self.running.insert(handle.id(), tag);
        }
    }

    /// Whether any request is awaiting the server.
    pub(super) fn is_busy(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// The next launched request to finish.
    ///
    /// A task that ended without answering -- it panicked -- still yields an
    /// answer, an error, so its asker is not left waiting on it.
    pub(super) async fn next(&mut self) -> Option<HintAnswer> {
        match self.tasks.join_next_with_id().await? {
            Ok((id, answer)) => {
                self.running.remove(&id);
                Some(answer)
            },
            Err(error) => {
                let tag = self.running.remove(&error.id())?;
                Some(HintAnswer {
                    tag,
                    result: Err(LspError::Protocol(format!(
                        "inlay-hint request ended without an answer: {error}"
                    ))),
                })
            },
        }
    }

    /// Answer everything accepted with an empty set and stop what is running:
    /// the connection it was headed for is gone.
    pub(super) fn abandon(&mut self, updates: &mpsc::UnboundedSender<LspUpdate>, generation: u64) {
        // Dropping the set aborts its tasks, which releases the client they
        // hold -- and with it, for a spawned server, the process.
        self.tasks = JoinSet::new();
        let tags = self
            .deferred
            .drain(..)
            .map(|ask| ask.tag)
            .chain(self.running.drain().map(|(_, tag)| tag));
        for tag in tags {
            answer_empty(updates, tag, generation);
        }
    }

    /// Stop every running request and wait until each has released the client,
    /// so the caller can take sole ownership of it to shut it down.
    pub(super) async fn shutdown(&mut self) {
        self.deferred.clear();
        self.running.clear();
        self.tasks.shutdown().await;
    }
}

/// Report one finished request: charge its outcome to the connection's health,
/// then answer it.
///
/// An [`LspError::Unsupported`] refusal is neither: `note` leaves the tally
/// exactly as it was, and the answer is empty like any other failure.
pub(super) fn deliver(
    answer: HintAnswer,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    key: &SlotKey,
    token: SlotToken,
    generation: u64,
) {
    let hints = match tally.observe(answer.result) {
        Ok(hints) => hints,
        Err(error) => {
            tally.note::<()>(Err(error), dead, updates, key, token);
            Vec::new()
        },
    };
    let HintTag {
        request,
        doc,
        version,
    } = answer.tag;
    let _ = updates.send(LspUpdate::InlayHints {
        generation,
        request,
        doc,
        version,
        hints,
    });
}

fn answer_empty(updates: &mpsc::UnboundedSender<LspUpdate>, tag: HintTag, generation: u64) {
    let _ = updates.send(LspUpdate::InlayHints {
        generation,
        request: tag.request,
        doc: tag.doc,
        version: tag.version,
        hints: Vec::new(),
    });
}
