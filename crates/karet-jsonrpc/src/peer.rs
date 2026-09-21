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
//! than silence. A peer that is never answered waits forever — JSON-RPC puts
//! no timeout obligation on the requester, and most language servers have none.
//!
//! [`Handler::answer`]: crate::Handler::answer
//! [`Connection::inbound_requests`]: crate::Connection::inbound_requests

use serde_json::Value;
use tokio::sync::mpsc;

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
    outbound: Option<mpsc::Sender<Outbound>>,
    peer: &'static str,
}

impl Responder {
    pub(crate) fn new(
        id: Value,
        method: String,
        outbound: mpsc::Sender<Outbound>,
        peer: &'static str,
    ) -> Self {
        Self {
            id,
            method,
            outbound: Some(outbound),
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
        let Some(outbound) = self.outbound.take() else {
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
        deliver(outbound, frame, self.peer);
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if self.outbound.is_none() {
            return;
        }
        let method = std::mem::take(&mut self.method);
        self.send(Err(ResponseError::method_not_found(&method)));
        self.method = method;
    }
}

/// Hand `frame` to the writer without ever dropping it and without ever
/// blocking the caller.
///
/// The caller is usually the reader task, which must not block on outbound
/// capacity: the peer can be stalled writing to its own stdout precisely
/// because we stopped reading it, and waiting here would close that loop into
/// a deadlock. So a full queue is waited out on a detached task instead, which
/// keeps the reader draining.
///
/// `try_send` alone was the old behaviour, and it *dropped* the reply — which
/// is the one outcome a peer cannot recover from.
pub(crate) fn deliver(outbound: mpsc::Sender<Outbound>, frame: Vec<u8>, peer: &'static str) {
    let full = match outbound.try_send(Outbound::Frame(frame)) {
        Ok(()) => return,
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Nothing to answer to; the connection is already gone.
            tracing::debug!(peer, "connection closed before a reply could be sent");
            return;
        },
        Err(mpsc::error::TrySendError::Full(item)) => item,
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                // Resolves as soon as the writer drains one frame, or errors
                // once the connection dies — either way the reply is not lost
                // silently.
                let _ = outbound.send(full).await;
            });
        },
        Err(_) => {
            // A responder dropped outside any runtime. Nothing can await here.
            tracing::warn!(peer, "no runtime to defer a reply onto; the peer will wait");
        },
    }
}
