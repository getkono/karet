//! Peer-initiated requests: the stream a consumer drains, and the reply token
//! that guarantees every one of them is answered.
//!
//! JSON-RPC is a peer protocol — both sides may issue requests — but answering
//! one can require work the connection itself cannot do: applying an edit,
//! asking the user, reading state behind a lock. [`Handler::answer`] cannot
//! host that, because it runs on the reader task and anything it awaits stalls
//! every other message on the connection.
//!
//! So a consumer takes [`Connection::inbound_requests`] and answers at its own
//! pace through a [`Responder`]. The one invariant this module exists to hold
//! is that **a peer request is always answered**: a responder that is dropped,
//! a stream nobody is draining, and a full queue all produce a reply rather
//! than silence. Replies that meet a full outbound queue wait on one
//! per-connection overflow queue with a single drainer (see `Replies`). A peer that is never answered waits forever — JSON-RPC puts
//! no timeout obligation on the requester, and most language servers have none.
//!
//! [`Handler::answer`]: crate::Handler::answer
//! [`Connection::inbound_requests`]: crate::Connection::inbound_requests

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::connection::Outbound;
use crate::message;
use crate::message::ResponseError;

/// One request the peer issued, waiting to be answered.
#[derive(Debug)]
pub struct PeerRequest {
    /// The method the peer invoked.
    pub method: String,
    /// The request parameters (or `Null`).
    pub params: Value,
    /// The token that answers it.
    pub responder: Responder,
}

/// The right — and the obligation — to answer one peer request.
///
/// Answer with [`respond`](Self::respond), [`ok`](Self::ok) or
/// [`error`](Self::error). Dropping it without answering is not a leak: the
/// `Drop` impl sends [`ResponseError::method_not_found`], because a peer left
/// waiting is a worse outcome than a wrong answer and discipline is not a
/// mechanism. Answering twice is impossible — every method consumes `self`.
#[derive(Debug)]
pub struct Responder {
    id: Value,
    method: String,
    /// Taken when the reply is sent, so `Drop` can tell answered from not.
    replies: Option<Replies>,
    peer: &'static str,
}

impl Responder {
    pub(crate) fn new(id: Value, method: String, replies: Replies, peer: &'static str) -> Self {
        Self {
            id,
            method,
            replies: Some(replies),
            peer,
        }
    }

    /// The peer's request id, echoed verbatim in the reply.
    ///
    /// Exposed so a consumer can correlate a cancellation — LSP's
    /// `$/cancelRequest`, say — with the responder to drop. The id is opaque:
    /// a peer may use a number, a string, or (degenerately) null.
    #[must_use]
    pub fn id(&self) -> &Value {
        &self.id
    }

    /// The method being answered.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Answer the request.
    pub fn respond(mut self, outcome: Result<Value, ResponseError>) {
        self.send(outcome);
    }

    /// Answer the request successfully.
    pub fn ok(self, value: Value) {
        self.respond(Ok(value));
    }

    /// Answer the request with a failure.
    pub fn error(self, error: ResponseError) {
        self.respond(Err(error));
    }

    /// Encode and hand the reply to the writer, marking this responder used.
    ///
    /// Takes `&mut self` rather than `self` so `Drop` can share it.
    fn send(&mut self, outcome: Result<Value, ResponseError>) {
        let Some(replies) = self.replies.take() else {
            return; // already answered
        };
        let response = message::OutgoingResponse::new(self.id.clone(), outcome);
        let frame = match serde_json::to_vec(&response) {
            Ok(frame) => frame,
            Err(e) => {
                // The outcome would not serialize. Still answer: a failure the
                // peer can read beats a silence it cannot.
                tracing::warn!(
                    peer = self.peer,
                    error = %e,
                    method = %self.method,
                    "re-encoding an unserializable reply as an error"
                );
                let fallback = message::OutgoingResponse::new(
                    self.id.clone(),
                    Err(ResponseError::internal_error(&self.method, e)),
                );
                match serde_json::to_vec(&fallback) {
                    Ok(frame) => frame,
                    Err(e) => {
                        tracing::error!(
                            peer = self.peer,
                            error = %e,
                            method = %self.method,
                            "could not encode any reply; the peer will wait"
                        );
                        return;
                    },
                }
            },
        };
        replies.deliver(Outbound::Frame(frame), self.peer);
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if self.replies.is_none() {
            return;
        }
        let method = std::mem::take(&mut self.method);
        self.send(Err(ResponseError::method_not_found(&method)));
        self.method = method;
    }
}

/// The one path every reply to a peer request takes to the writer.
///
/// A reply must never be dropped and must never block the caller — usually the
/// reader task, which must not wait on outbound capacity: the peer can be
/// stalled writing to its own stdout precisely because we stopped reading it,
/// and waiting here would close that loop into a deadlock. (`try_send` alone
/// dropped the reply, the one outcome a peer cannot recover from.)
///
/// So a reply that finds the bounded outbound queue full is parked on a
/// per-connection overflow queue, and **one** drainer task (see
/// [`Replies::start`]) moves parked items onto the outbound queue as capacity
/// frees, in the order they were parked. While anything is parked, later
/// items park behind it rather than overtake it through the fast path.
///
/// # What bounds it
///
/// Tasks are bounded: one drainer per connection, however many replies are
/// deferred. Memory is bounded by the peer, not by a constant — the overflow
/// queue holds at most one reply per request the peer has sent and we have
/// answered but not yet written, so it grows only as fast as we *read* the
/// peer's requests. A hard cap is not available without breaking the
/// invariant: past it a reply would have to be dropped (the peer waits
/// forever) or the reader would have to block (the deadlock above). The
/// per-reply detached task this replaced had the same memory profile plus one
/// task, and one scheduler slot, per deferred reply.
#[derive(Clone, Debug)]
pub(crate) struct Replies {
    outbound: mpsc::Sender<Outbound>,
    overflow: mpsc::UnboundedSender<Outbound>,
    /// Items parked on `overflow` and not yet handed to `outbound`.
    backlog: Arc<AtomicUsize>,
}

impl Replies {
    /// Build the reply path over `outbound` and spawn its single drainer.
    ///
    /// Must be called inside a Tokio runtime, as [`Connection::start`] is.
    ///
    /// [`Connection::start`]: crate::Connection::start
    pub(crate) fn start(outbound: mpsc::Sender<Outbound>) -> (Self, JoinHandle<()>) {
        let (overflow, mut parked) = mpsc::unbounded_channel::<Outbound>();
        let backlog = Arc::new(AtomicUsize::new(0));
        let drainer_outbound = outbound.clone();
        let drainer_backlog = Arc::clone(&backlog);
        let drainer = tokio::spawn(async move {
            while let Some(item) = parked.recv().await {
                // Resolves as soon as the writer drains one frame, or errors
                // once the connection is gone -- nothing is left to answer then.
                let _ = drainer_outbound.send(item).await;
                drainer_backlog.fetch_sub(1, Ordering::SeqCst);
            }
        });
        (
            Self {
                outbound,
                overflow,
                backlog,
            },
            drainer,
        )
    }

    /// Hand `item` to the writer without dropping it and without blocking.
    ///
    /// Synchronous and runtime-free, so a [`Responder`] dropped outside any
    /// runtime still answers.
    pub(crate) fn deliver(&self, item: Outbound, peer: &'static str) {
        // The fast path only while nothing is parked, so an item never
        // overtakes one deferred before it.
        let item = if self.backlog.load(Ordering::SeqCst) == 0 {
            match self.outbound.try_send(item) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    // Nothing to answer to; the connection is already gone.
                    tracing::debug!(peer, "connection closed before a reply could be sent");
                    return;
                },
                Err(mpsc::error::TrySendError::Full(item)) => item,
            }
        } else {
            item
        };
        self.backlog.fetch_add(1, Ordering::SeqCst);
        if self.overflow.send(item).is_err() {
            // The drainer is gone, which only happens once the connection is.
            self.backlog.fetch_sub(1, Ordering::SeqCst);
            tracing::debug!(
                peer,
                "connection closed before a deferred reply could be sent"
            );
        }
    }
}
