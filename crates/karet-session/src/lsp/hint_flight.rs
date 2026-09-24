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
//!
//! At most one request per document is ever launched at a time. A newer one
//! for a document whose request is still running stays deferred until that
//! one returns, and only the newest deferred request is then launched -- each
//! one it replaced is reported unanswered. The alternative, cancelling the running
//! request, is not available: `karet-jsonrpc` does not hand the request id to
//! its caller, so `$/cancelRequest` cannot be sent, and dropping the future
//! only stops karet listening -- the server keeps computing. Without the
//! one-per-document rule, an editor re-asking on every scroll or keystroke
//! stacked up a full inference pass per ask on a server that was already the
//! slowest thing in the loop.

use std::collections::HashMap;
use std::future::Future;
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
    /// The superseded one is reported unanswered rather than dropped: every
    /// request is answered. Its asker has already moved on -- it asked again --
    /// so the report is discarded there.
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
            answer_failed(updates, older.tag, generation);
        }
        self.deferred.push(ask);
    }

    /// Launch every deferred request the server can now answer correctly: all
    /// of them but those for `pending`, the one path with an edit not yet sent,
    /// and those for a document whose previous request is still running.
    pub(super) fn launch_ready(&mut self, client: &Arc<LspClient>, pending: Option<&Path>) {
        self.launch_ready_with(pending, |ask| {
            let client = Arc::clone(client);
            async move { client.inlay_hints(&ask.path, ask.range).await }
        });
    }

    /// [`Self::launch_ready`], with how a request is issued left to `start`.
    ///
    /// Separate so the bookkeeping can be tested without a server.
    fn launch_ready_with<F, Fut>(&mut self, pending: Option<&Path>, mut start: F)
    where
        F: FnMut(HintAsk) -> Fut,
        Fut: Future<Output = Result<Vec<InlayHint>, LspError>> + Send + 'static,
    {
        let running = &self.running;
        let (waiting, ready): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deferred)
            .into_iter()
            .partition(|ask| {
                pending == Some(ask.path.as_path())
                    || running.values().any(|tag| tag.doc == ask.tag.doc)
            });
        self.deferred = waiting;
        for ask in ready {
            let tag = ask.tag;
            let request = start(ask);
            let handle = self.tasks.spawn(async move {
                HintAnswer {
                    tag,
                    result: request.await,
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

    /// Report everything accepted as unanswered and stop what is running: the
    /// connection it was headed for is gone.
    pub(super) fn abandon(&mut self, updates: &mpsc::UnboundedSender<LspUpdate>, generation: u64) {
        // Dropping the set aborts its tasks, which releases the client they
        // hold -- and with it, for a spawned server, the process.
        self.tasks = JoinSet::new();
        self.answer_all_failed(updates, generation);
    }

    /// Stop every running request and wait until each has released the client,
    /// so the caller can take sole ownership of it to shut it down -- reporting
    /// everything accepted as unanswered on the way, as [`Self::abandon`] does,
    /// because the slot is retiring and nothing else ever will.
    pub(super) async fn shutdown(
        &mut self,
        updates: &mpsc::UnboundedSender<LspUpdate>,
        generation: u64,
    ) {
        // Stopped before answering, so a task that finished in the meantime is
        // still answered exactly once: `shutdown` discards its output, and its
        // tag is still in `running`.
        self.tasks.shutdown().await;
        self.answer_all_failed(updates, generation);
    }

    /// Report every deferred and running request as unanswered, forgetting
    /// them all.
    fn answer_all_failed(&mut self, updates: &mpsc::UnboundedSender<LspUpdate>, generation: u64) {
        let tags = self
            .deferred
            .drain(..)
            .map(|ask| ask.tag)
            .chain(self.running.drain().map(|(_, tag)| tag));
        for tag in tags {
            answer_failed(updates, tag, generation);
        }
    }
}

/// Report one finished request: charge its outcome to the connection's health,
/// then answer it.
///
/// An [`LspError::Unsupported`] refusal is neither: the tally is left exactly
/// as it was, and the answer is an empty set, because a server that does not
/// offer hints has none to show. Every other failure is reported as
/// unanswered instead ([`LspUpdate::InlayHintsFailed`]): the server had
/// nothing to say about the document, so what the editor already shows for
/// it stands.
///
/// Neither is a timeout. The hang streak exists to catch a server that has
/// stopped answering *what the user is waiting on*, and it is calibrated for
/// requests the server task awaits one at a time: three in a row take at least
/// 90 seconds. Hint requests run beside those, several documents at once, so
/// three of them can time out together inside one 30-second window -- and a
/// server slow to infer types for a large file is slow, not hung. Charging
/// them condemned such a server, killing it mid-analysis for background work
/// nobody was waiting on. A successful answer is neutral too: counting it would
/// let a server that keeps answering hints but has stopped answering hover and
/// completion reset the streak forever, which is the hang the streak is there
/// to catch. It does prove the server has finished starting up, though, so it
/// arms the liveness gate: a server whose only answers so far were hints is
/// still one that can be condemned for going silent afterwards.
pub(super) fn deliver(
    answer: HintAnswer,
    tally: &mut FailureTally,
    dead: &mut bool,
    updates: &mpsc::UnboundedSender<LspUpdate>,
    key: &SlotKey,
    token: SlotToken,
    generation: u64,
) {
    let hints = match answer.result {
        Ok(hints) => {
            tally.note_background_answer();
            Some(hints)
        },
        Err(LspError::Unsupported { .. }) => Some(Vec::new()),
        Err(LspError::Timeout) => {
            tracing::debug!(language = %key, "inlay-hint request timed out; not charged");
            None
        },
        // A closed connection is a fact about the server, not about hints, and
        // must still be noticed.
        Err(LspError::Closed) => {
            tally.note::<()>(Err(LspError::Closed), dead, updates, key, token);
            None
        },
        // Any other error is neutral like an answer: `note` would reset the
        // streak, letting a hint the server cancelled (rust-analyzer's
        // `ContentModified` while typing) excuse hover timeouts around it.
        Err(error) => {
            tracing::debug!(language = %key, %error, "inlay-hint request failed; not charged");
            None
        },
    };
    let Some(hints) = hints else {
        answer_failed(updates, answer.tag, generation);
        return;
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

/// Report the request `tag` echoes as unanswered.
fn answer_failed(updates: &mpsc::UnboundedSender<LspUpdate>, tag: HintTag, generation: u64) {
    let _ = updates.send(LspUpdate::InlayHintsFailed {
        generation,
        request: tag.request,
        doc: tag.doc,
        version: tag.version,
    });
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;

    use tokio::sync::oneshot;

    use super::super::health;
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
    type Reply = Pin<Box<dyn Future<Output = Result<Vec<InlayHint>, LspError>> + Send>>;

    fn ask(request: u64, doc: u64) -> HintAsk {
        HintAsk {
            tag: HintTag {
                request: RequestId(request),
                doc: DocumentId(doc),
                version: 1,
            },
            path: PathBuf::from(format!("/w/{doc}.rs")),
            range: Range::default(),
        }
    }

    /// Every answer sent so far, as `(request, hint count)`; a count of
    /// `None` is a request reported unanswered.
    fn answers(rx: &mut mpsc::UnboundedReceiver<LspUpdate>) -> Vec<(u64, Option<usize>)> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|update| match update {
                LspUpdate::InlayHints { request, hints, .. } => {
                    Some((request.0, Some(hints.len())))
                },
                LspUpdate::InlayHintsFailed { request, .. } => Some((request.0, None)),
                _ => None,
            })
            .collect()
    }

    fn requests<'a>(tags: impl Iterator<Item = &'a HintTag>) -> Vec<u64> {
        let mut out: Vec<u64> = tags.map(|tag| tag.request.0).collect();
        out.sort_unstable();
        out
    }

    /// Launch whatever is ready with a request that never answers.
    fn launch_never(flight: &mut HintFlight) {
        flight.launch_ready_with(None, |_| {
            std::future::pending::<Result<Vec<InlayHint>, LspError>>()
        });
    }

    #[test]
    fn a_replaced_deferred_request_is_reported_unanswered() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut flight = HintFlight::default();
        flight.ask(ask(1, 7), &tx, 0);
        flight.ask(ask(2, 8), &tx, 0);
        assert!(
            answers(&mut rx).is_empty(),
            "another document's ask is not a replacement"
        );
        flight.ask(ask(3, 7), &tx, 0);
        assert_eq!(
            answers(&mut rx),
            vec![(1, None)],
            "the replaced ask went unanswered"
        );
        assert_eq!(requests(flight.deferred.iter().map(|ask| &ask.tag)), [2, 3]);
    }

    #[tokio::test]
    async fn a_document_never_has_two_requests_running() {
        // A newer ask for a document whose request is still running waits for
        // it, and only the newest of those waiting is launched when it returns.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut flight = HintFlight::default();
        let (reply, answer) = oneshot::channel::<Vec<InlayHint>>();
        let mut replies =
            vec![Box::pin(async move { answer.await.map_err(|_| LspError::Closed) }) as Reply];
        flight.ask(ask(1, 7), &tx, 0);
        flight.launch_ready_with(None, |_| {
            replies
                .pop()
                .unwrap_or_else(|| Box::pin(std::future::pending()))
        });
        assert_eq!(requests(flight.running.values()), [1]);

        flight.ask(ask(2, 7), &tx, 0);
        flight.ask(ask(3, 7), &tx, 0);
        flight.ask(ask(4, 9), &tx, 0);
        launch_never(&mut flight);
        assert_eq!(
            requests(flight.running.values()),
            [1, 4],
            "a second request ran for one document, or another document's waited"
        );
        assert_eq!(requests(flight.deferred.iter().map(|ask| &ask.tag)), [3]);
        assert_eq!(
            answers(&mut rx),
            vec![(2, None)],
            "the replaced ask went unanswered"
        );

        let _ = reply.send(Vec::new());
        let finished = flight.next().await.map(|answer| answer.tag.request.0);
        assert_eq!(finished, Some(1));
        launch_never(&mut flight);
        assert!(
            flight.deferred.is_empty(),
            "the newest ask was not launched"
        );
        assert_eq!(requests(flight.running.values()), [3, 4]);
    }

    #[tokio::test]
    async fn a_task_that_ends_without_answering_is_answered_anyway() -> TestResult {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut flight = HintFlight::default();
        flight.ask(ask(5, 7), &tx, 0);
        launch_never(&mut flight);
        // Ended from outside, the way a panic ends it: the task never produces
        // a `HintAnswer` of its own.
        flight.tasks.abort_all();
        let answer = flight.next().await.ok_or("no fallback answer")?;
        assert_eq!(answer.tag.request, RequestId(5));
        assert!(matches!(answer.result, Err(LspError::Protocol(_))));
        assert!(
            flight.running.is_empty(),
            "the ended task was not forgotten"
        );
        Ok(())
    }

    fn key() -> SlotKey {
        SlotKey::new(crate::api::LanguageServerId::new("rust"), "/w")
    }

    /// Deliver one finished hint request with `result`, as the task loop does.
    fn deliver_one(
        tally: &mut FailureTally,
        dead: &mut bool,
        tx: &mpsc::UnboundedSender<LspUpdate>,
        result: Result<Vec<InlayHint>, LspError>,
    ) {
        let answer = HintAnswer {
            tag: ask(1, 7).tag,
            result,
        };
        deliver(answer, tally, dead, tx, &key(), SlotToken::FIRST, 0);
    }

    /// Charge one timed-out request the task awaited in line -- a hover, say.
    fn serial_timeout(
        tally: &mut FailureTally,
        dead: &mut bool,
        tx: &mpsc::UnboundedSender<LspUpdate>,
    ) {
        tally.note::<()>(Err(LspError::Timeout), dead, tx, &key(), SlotToken::FIRST);
    }

    fn died(rx: &mut mpsc::UnboundedReceiver<LspUpdate>) -> bool {
        std::iter::from_fn(|| rx.try_recv().ok())
            .any(|update| matches!(update, LspUpdate::ServerDied { .. }))
    }

    #[test]
    fn hint_timeouts_never_mark_a_server_hung() {
        // Background requests running side by side can time out together
        // inside one window; a server slow to infer is not a server that has
        // stopped answering.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_mul(3) {
            deliver_one(&mut tally, &mut dead, &tx, Err(LspError::Timeout));
        }
        assert!(!dead, "hint timeouts condemned the connection");
        assert!(!tally.hung());
        assert_eq!(
            answers(&mut rx).len(),
            usize::try_from(health::TIMEOUT_DEATH_LIMIT.saturating_mul(3)).unwrap_or_default(),
            "a timed-out hint request went unanswered"
        );

        // They did not advance the streak either: the serial requests still
        // need the whole run to condemn it.
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        assert!(!dead, "hint timeouts were counted toward the streak");
        serial_timeout(&mut tally, &mut dead, &tx);
        assert!(dead, "hover and completion timeouts must still condemn it");
        assert!(tally.hung());
        assert!(died(&mut rx));
    }

    #[test]
    fn an_answered_hint_request_does_not_clear_the_streak() {
        // A server that still answers hints but has stopped answering hover
        // and completion is hung on what the user is waiting for.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        deliver_one(&mut tally, &mut dead, &tx, Ok(Vec::new()));
        assert!(!dead);
        serial_timeout(&mut tally, &mut dead, &tx);
        assert!(dead, "a hint answer reset the streak");
        assert!(died(&mut rx));
    }

    #[test]
    fn a_failed_hint_request_does_not_clear_the_streak_either() {
        // rust-analyzer answers a hint request an edit cancelled with
        // `ContentModified`; that must not excuse hover timeouts around it.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        let _answered = tally.observe(Ok::<(), LspError>(()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT.saturating_sub(1) {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        deliver_one(
            &mut tally,
            &mut dead,
            &tx,
            Err(LspError::Server("content modified".to_owned())),
        );
        assert!(!dead, "a hint error is not a death");
        serial_timeout(&mut tally, &mut dead, &tx);
        assert!(dead, "a hint error reset the streak");
        assert!(died(&mut rx));
    }

    #[test]
    fn an_answered_hint_request_arms_the_liveness_gate() {
        // A server whose only answers so far were hints has finished starting
        // up; silence on what the user waits for afterwards is a hang.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        deliver_one(&mut tally, &mut dead, &tx, Ok(Vec::new()));
        for _ in 0..health::TIMEOUT_DEATH_LIMIT {
            serial_timeout(&mut tally, &mut dead, &tx);
        }
        assert!(dead, "a server that answered hints was never condemned");
        assert!(died(&mut rx));
    }

    #[test]
    fn only_a_server_with_no_hints_to_offer_answers_an_empty_set() {
        // A refusal says the server has no hints; a failure says nothing about
        // the document, so the editor must keep what it shows.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        let outcomes = [
            Ok(vec![InlayHint {
                position: karet_core::LineCol::new(0, 0),
                label: ": i32".to_owned(),
                kind: karet_core::InlayHintKind::Type,
                padding_left: false,
                padding_right: false,
            }]),
            Err(LspError::Unsupported {
                method: "textDocument/inlayHint",
            }),
            Err(LspError::Timeout),
            Err(LspError::Server("content modified".to_owned())),
            Err(LspError::Closed),
        ];
        for result in outcomes {
            deliver_one(&mut tally, &mut dead, &tx, result);
        }
        let counts: Vec<Option<usize>> = answers(&mut rx)
            .into_iter()
            .map(|(_, count)| count)
            .collect();
        assert_eq!(counts, [Some(1), Some(0), None, None, None]);
    }

    #[test]
    fn a_hint_request_on_a_closed_connection_is_still_a_death() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tally = FailureTally::default();
        let mut dead = false;
        deliver_one(&mut tally, &mut dead, &tx, Err(LspError::Closed));
        assert!(dead);
        assert!(died(&mut rx));
    }

    #[tokio::test]
    async fn abandoning_reports_running_and_deferred_requests_unanswered() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut flight = HintFlight::default();
        flight.ask(ask(1, 7), &tx, 0);
        launch_never(&mut flight);
        flight.ask(ask(2, 7), &tx, 0);
        flight.abandon(&tx, 0);
        let mut answered = answers(&mut rx);
        answered.sort_unstable();
        assert_eq!(answered, vec![(1, None), (2, None)]);
        assert!(!flight.is_busy());
    }
}
