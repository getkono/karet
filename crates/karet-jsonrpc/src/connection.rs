//! The JSON-RPC 2.0 connection: the [`Actor`] speaking the JSON-RPC envelope.
//!
//! The actor owns the I/O tasks and the request/response correlation. This
//! module supplies what JSON-RPC means by a message: responses resolve the
//! pending request with the matching id, notifications fan out on a broadcast
//! channel as a [`Handler::Push`] payload, peer→client requests go to the
//! consumer's [`Connection::inbound_requests`] stream or, while nobody holds it,
//! to [`Handler::answer`] inline, and everything else is logged and dropped. When
//! the stream ends, in-flight requests fail with [`RpcError::Closed`].
//!
//! Everything protocol-specific lives on the [`Handler`]: the framing, the
//! broadcast payload, the answers, and the tuning constants.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::actor::Actor;
use crate::actor::Classified;
use crate::actor::Envelope;
use crate::actor::Link;
use crate::framing::Framing;
use crate::message;
use crate::message::Incoming;
use crate::message::RequestId;
use crate::message::ResponseError;
use crate::peer::PeerRequest;
use crate::peer::Responder;

/// The protocol-specific half of a connection: framing, the broadcast payload,
/// peer-request answers, and notification side effects.
///
/// [`Handler::answer`] answers peer→client requests **synchronously** on the
/// reader task, which is all a constant answer needs (`workspace/configuration`
/// with nothing to offer, `client/registerCapability`, …).
///
/// An answer that must *await* — apply an edit, ask the user, read state behind
/// a lock — cannot go here, because the reader task is blocked for its duration
/// and every other message on the connection waits with it. Take
/// [`Connection::inbound_requests`] instead: while a consumer holds that
/// receiver it gets every peer request, and `answer` is not called at all.
pub trait Handler: Send + Sync + 'static {
    /// How message bodies are delimited on the wire.
    type Framing: Framing;
    /// The payload broadcast to [`Connection::subscribe`] listeners.
    type Push: Clone + Send + 'static;

    /// What the peer is called in log messages.
    const PEER: &'static str = "peer";
    /// How long a request may wait for its response before timing out.
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
    /// The deadline for draining the outbound queue in [`Connection::close`].
    const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
    /// Broadcast capacity; slow subscribers drop the oldest payloads.
    ///
    /// [`Connection::start`] clamps this into `1..=usize::MAX / 2`, the range
    /// `tokio::sync::broadcast` accepts — an override outside it degrades the
    /// capacity rather than panicking.
    const PUSH_CHANNEL_CAPACITY: usize = 64;
    /// Frames waiting to be written. A buggy producer cannot grow memory without
    /// bound; requests wait for capacity and notifications fail fast.
    ///
    /// Replies to peer requests are the exception, because none may be dropped
    /// and the reader must not block: one that finds this queue full waits on
    /// a per-connection overflow queue behind a single drainer task, so it
    /// costs memory proportional to the answered-but-unwritten requests the
    /// peer has sent — never a task per reply. That overflow queue has no
    /// constant bound: a peer that floods requests while never reading grows
    /// it until it reads or the connection ends.
    ///
    /// [`Connection::start`] clamps this to at least `1`, the minimum
    /// `tokio::sync::mpsc` accepts — an override of `0` degrades to `1` rather
    /// than panicking.
    const OUTBOUND_CHANNEL_CAPACITY: usize = 256;
    /// Peer requests waiting for the consumer of [`Connection::inbound_requests`].
    ///
    /// Bounded, and deliberately not a broadcast: a lagging broadcast receiver
    /// drops the oldest payloads, and a *dropped request* is one the peer waits
    /// on forever. Overflow answers immediately instead — see
    /// [`Responder`](crate::Responder).
    ///
    /// Clamped to at least `1` for the same reason as the outbound capacity.
    const INBOUND_REQUEST_CAPACITY: usize = 32;

    /// Build the broadcast payload for one peer notification (`None` drops it).
    ///
    /// Called only while at least one subscriber is listening, so an expensive
    /// clone is not paid for when nobody would receive it.
    fn push_payload(&self, method: &str, params: &Value) -> Option<Self::Push>;

    /// React to one peer notification, after the broadcast fan-out.
    fn on_notification(&self, method: &str, params: Value) {
        let _ = (method, params);
    }

    /// Answer a peer→client request, synchronously, on the reader task.
    ///
    /// Not called while a consumer holds [`Connection::inbound_requests`].
    /// Anything that must await belongs there; see the trait docs.
    ///
    /// # Errors
    ///
    /// Returns the [`ResponseError`] to send back; the default answers every
    /// method with [`ResponseError::method_not_found`].
    fn answer(&self, method: &str, params: &Value) -> Result<Value, ResponseError> {
        let _ = params;
        Err(ResponseError::method_not_found(method))
    }
}

/// Errors raised by a [`Connection`] operation.
///
/// Deliberately **not** `#[non_exhaustive]`: every consumer is a lockstep-
/// versioned workspace crate, so the attribute would buy no compatibility —
/// it would only force a catch-all arm in the bridges that translate this
/// enum (`impl From<RpcError> for LspError`), where a newly added variant
/// would then compile clean and surface silently as the wrong error kind.
/// Exhaustive matching turns that into a compile error at the one place that
/// must be taught the new shape.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// The outgoing message could not be serialized.
    #[error("failed to encode {method}: {source}")]
    Encode {
        /// The method whose envelope failed to encode.
        method: String,
        /// The underlying serde failure.
        #[source]
        source: serde_json::Error,
    },
    /// The peer's result did not deserialize into the expected type.
    #[error("malformed {method} response: {source}")]
    Decode {
        /// The method whose response failed to decode.
        method: String,
        /// The underlying serde failure.
        #[source]
        source: serde_json::Error,
    },
    /// The peer answered with a JSON-RPC error object.
    #[error("{method} failed with code {}: {}", .error.code, .error.message)]
    Peer {
        /// The method that failed.
        method: String,
        /// The peer's error object.
        error: ResponseError,
    },
    /// The request was not answered within its deadline.
    #[error("request timed out")]
    Timeout,
    /// The connection to the peer is gone.
    #[error("the connection to the peer closed")]
    Closed,
    /// The bounded outbound queue is full (notifications fail fast).
    #[error("the outbound queue is full")]
    QueueFull,
}

/// The reader's half of the peer-request stream, plus whether anyone is there.
struct Inbound {
    tx: mpsc::Sender<PeerRequest>,
    /// Set once the receiver has been handed out. The reader consults this
    /// rather than `Sender::is_closed`, which cannot tell "nobody has taken the
    /// receiver yet" from "a consumer is listening" — the receiver exists from
    /// the moment the channel is built.
    active: Arc<AtomicBool>,
}

/// The JSON-RPC 2.0 envelope over a [`Handler`]: every destination the reader
/// routes a non-response message to.
struct JsonRpc<H: Handler> {
    handler: H,
    push: broadcast::Sender<H::Push>,
    inbound: Inbound,
}

impl<H: Handler> Envelope for JsonRpc<H> {
    type Framing = H::Framing;
    type Id = RequestId;
    type Outcome = Result<Value, ResponseError>;
    type Message = Incoming;

    const PEER: &'static str = H::PEER;
    const REQUEST_TIMEOUT: Duration = H::REQUEST_TIMEOUT;
    const CLOSE_TIMEOUT: Duration = H::CLOSE_TIMEOUT;
    const OUTBOUND_CHANNEL_CAPACITY: usize = H::OUTBOUND_CHANNEL_CAPACITY;

    fn request_id(seq: i64) -> RequestId {
        RequestId::Number(seq)
    }

    fn encode_request<P: Serialize>(
        &self,
        id: &RequestId,
        method: &str,
        params: P,
    ) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&message::OutgoingRequest::new(id.clone(), method, params))
    }

    fn classify(&self, value: Value) -> Classified<RequestId, Self::Outcome, Incoming> {
        match message::classify(value) {
            Some(Incoming::Response { id, result }) => Classified::Response {
                id,
                outcome: result,
            },
            Some(other) => Classified::Message(other),
            None => Classified::Malformed("a message with no JSON-RPC shape"),
        }
    }

    fn route(&self, message: Incoming, link: &Link) {
        route::<H>(self, message, link);
    }
}

/// A live JSON-RPC connection to one peer.
pub struct Connection<H: Handler> {
    actor: Actor<JsonRpc<H>>,
    /// The peer-request stream, until a consumer takes it.
    inbound_rx: Mutex<Option<mpsc::Receiver<PeerRequest>>>,
}

impl<H: Handler> Connection<H> {
    /// Start the reader/writer tasks over an arbitrary I/O pair.
    pub fn start<R, W>(handler: H, read: R, write: W) -> Self
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        // The capacities are downstream-overridable consts, and the channel
        // constructors *panic* on an out-of-range value (`mpsc` on 0;
        // `broadcast` on 0 or `> usize::MAX / 2`). This crate's lint floor
        // forbids panicking library code, so a bad const degrades the capacity
        // instead of aborting the process (the actor clamps the outbound one).
        let (push, _) = broadcast::channel(H::PUSH_CHANNEL_CAPACITY.clamp(1, usize::MAX / 2));
        let (inbound_tx, inbound_rx) =
            mpsc::channel::<PeerRequest>(H::INBOUND_REQUEST_CAPACITY.max(1));
        let envelope = JsonRpc {
            handler,
            push,
            inbound: Inbound {
                tx: inbound_tx,
                active: Arc::new(AtomicBool::new(false)),
            },
        };
        Self {
            actor: Actor::start(envelope, read, write),
            inbound_rx: Mutex::new(Some(inbound_rx)),
        }
    }

    /// Take the stream of requests the **peer** issues to us.
    ///
    /// Returns the receiver once; every later call returns `None`, because a
    /// request must reach exactly one answerer. While it is held, every peer
    /// request arrives here and [`Handler::answer`] is not called — a consumer
    /// that wants the old constant answers for some methods can delegate to
    /// [`Connection::handler`] itself.
    ///
    /// Every [`PeerRequest`] carries a [`Responder`] that answers it. Dropping
    /// one answers it anyway, with `-32601`, and so does letting this queue
    /// overflow: the peer is never left waiting on our silence. A request the
    /// peer cancels is answered through [`Responder::cancel`], not dropped.
    #[must_use]
    pub fn inbound_requests(&self) -> Option<mpsc::Receiver<PeerRequest>> {
        let taken = self.inbound_rx.lock().ok()?.take();
        if taken.is_some() {
            // Ordered after the take so the reader never routes to a stream
            // whose receiver is not yet in the consumer's hands.
            self.actor
                .envelope()
                .inbound
                .active
                .store(true, Ordering::SeqCst);
        }
        taken
    }

    /// Resolve once this connection is gone, whether the peer hung up, the
    /// stream lost framing, or a write failed.
    ///
    /// Returns immediately for a connection that is already closed, so a caller
    /// cannot miss the transition by racing it. Intended for a `select!` arm
    /// beside whatever else a consumer waits on: without one, a peer's death is
    /// noticed only when something next tries to talk to it.
    pub async fn closed(&self) {
        self.actor.closed().await;
    }

    /// The protocol handler this connection was started with.
    #[must_use]
    pub fn handler(&self) -> &H {
        &self.actor.envelope().handler
    }

    /// Issue `method` and await its typed result, bounded by
    /// [`Handler::REQUEST_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// See [`Connection::request_with`].
    pub async fn request<P, T>(&self, method: &str, params: P) -> Result<T, RpcError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        self.request_with(method, params, H::REQUEST_TIMEOUT).await
    }

    /// Issue `method` and await its typed result, bounded by `timeout`.
    ///
    /// # Errors
    ///
    /// [`RpcError::Encode`] if `params` will not serialize, [`RpcError::Closed`]
    /// if the connection is (or becomes) dead, [`RpcError::Timeout`] if the peer
    /// does not answer in time, [`RpcError::Peer`] for a JSON-RPC error answer,
    /// and [`RpcError::Decode`] if the result is not a `T`.
    pub async fn request_with<P, T>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<T, RpcError>
    where
        P: Serialize,
        T: DeserializeOwned,
    {
        match self.actor.request_with(method, params, timeout).await? {
            Err(error) => Err(RpcError::Peer {
                method: method.to_owned(),
                error,
            }),
            Ok(value) => serde_json::from_value(value).map_err(|source| RpcError::Decode {
                method: method.to_owned(),
                source,
            }),
        }
    }

    /// Send a notification (fire-and-forget).
    ///
    /// # Errors
    ///
    /// [`RpcError::Encode`] if `params` will not serialize, [`RpcError::Closed`]
    /// if the connection is gone, or [`RpcError::QueueFull`] if the bounded
    /// outbound queue has no room.
    pub fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), RpcError> {
        let frame = serde_json::to_vec(&message::OutgoingNotification::new(method, params))
            .map_err(|source| RpcError::Encode {
                method: method.to_owned(),
                source,
            })?;
        self.actor.try_send(frame)
    }

    /// Subscribe to the [`Handler::Push`] payloads built from peer notifications.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<H::Push> {
        self.actor.envelope().push.subscribe()
    }

    /// Drain the outbound queue (every already-enqueued frame is written and
    /// flushed), then stop both I/O tasks. Bounded by [`Handler::CLOSE_TIMEOUT`]
    /// in case the peer stops consuming.
    ///
    /// # Ordering against peer replies
    ///
    /// The close signal travels the same ordered path as replies to peer
    /// requests, not the plain outbound queue:
    ///
    /// - **Replies deferred before `close()` are written first.** A reply that
    ///   met a full outbound queue is parked behind the queue; the close
    ///   signal parks behind *it*, so every reply answered before this call
    ///   reaches the wire ahead of the close (within the timeout above).
    /// - **`close()` never waits for queue capacity** to enqueue the signal: a
    ///   full queue parks it like any deferred reply, and the wait is then the
    ///   bounded one on the writer draining.
    /// - **Replies after the close signal are dropped.** The writer stops at
    ///   the signal, so a reply parked behind it — or sent later by a
    ///   [`Responder`] still held by a consumer — is discarded. It is logged at
    ///   `debug` when it arrives after the queue has closed; one that reaches
    ///   the queue behind the signal before the writer stops is dropped with
    ///   the queue, unlogged. This is the one case in which a peer
    ///   request goes unanswered; answer outstanding responders before calling
    ///   `close()` if the peer must see those replies.
    pub async fn close(&mut self) {
        self.actor.close().await;
    }

    /// How many requests are waiting for a response.
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.actor.in_flight()
    }
}

/// Route one inbound JSON-RPC message that is not a response.
fn route<H: Handler>(envelope: &JsonRpc<H>, message: Incoming, link: &Link) {
    let JsonRpc {
        handler,
        push,
        inbound,
    } = envelope;
    match message {
        Incoming::Response { id, .. } => {
            // `classify` hands responses to the actor, never here.
            tracing::debug!(peer = H::PEER, id = %id, "ignoring a misrouted response");
        },
        Incoming::Request { id, method, params } => {
            let responder = Responder::new(id, method.clone(), link.replies().clone(), H::PEER);
            // A consumer holding the stream owns every peer request, including
            // the ones `answer` used to field: splitting them by method would
            // make which path ran depend on timing.
            // `is_closed` as well as the flag: the flag says the stream was
            // ever taken, and a consumer that has since dropped the receiver
            // would otherwise disable `Handler::answer` permanently, refusing
            // the constant answers it implements correctly.
            if inbound.active.load(Ordering::SeqCst) && !inbound.tx.is_closed() {
                let request = PeerRequest {
                    method: method.clone(),
                    params,
                    responder,
                };
                // Never blocks the reader, and never drops the request: a full
                // queue or a hung-up consumer sends it back inside the error,
                // and dropping that drops the `Responder`, which answers the
                // peer on its way out.
                if let Err(refused) = inbound.tx.try_send(request) {
                    tracing::warn!(
                        peer = H::PEER,
                        method = %method,
                        reason = %refused,
                        "peer request not delivered to the consumer; answering it here"
                    );
                    // Answered explicitly, because `Responder`'s drop says
                    // `-32601` and that is the wrong thing to tell a peer here.
                    // A server reads "method not found" as "this client does
                    // not implement it" and stops offering the feature; this is
                    // transient back-pressure, which `-32603` reports honestly.
                    let responder = match refused {
                        mpsc::error::TrySendError::Full(request)
                        | mpsc::error::TrySendError::Closed(request) => request.responder,
                    };
                    responder.error(ResponseError::internal_error(&method, "the client is busy"));
                }
                return;
            }
            let outcome = handler.answer(&method, &params);
            responder.respond(outcome);
        },
        Incoming::Notification { method, params } => {
            // Every notification fans out first — the escape hatch that lets a
            // consumer handle peer-specific methods the typed surface does not
            // model.
            //
            // Building the payload deep-copies `params`, and a diagnostics
            // payload for a large file is not small, so only pay for it when
            // somebody is listening. A subscriber only ever sees notifications
            // sent after it subscribed, so skipping the send while the count is
            // zero is indistinguishable from sending into no receivers.
            if push.receiver_count() > 0
                && let Some(item) = handler.push_payload(&method, &params)
            {
                let _ = push.send(item); // a receiver that dropped between the check and here is fine
            }
            handler.on_notification(&method, params);
        },
        Incoming::ProtocolError { error } => {
            // The peer rejected something we sent before it could say which
            // request was at fault, so nothing in `pending` can be completed
            // with this. Reporting it is the whole point: the request it refers
            // to will otherwise wait out its full timeout with no explanation,
            // and the fault is ours to fix.
            tracing::warn!(
                peer = H::PEER,
                code = error.code,
                message = %error.message,
                "peer rejected a message we sent"
            );
        },
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
